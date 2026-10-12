use super::book::{Block, Book, ContentPoint, check_cancel};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::AtomicBool;

pub(super) struct IndexedChapter {
    pub href: String,
    pub title: String,
    pub blocks: Vec<String>,
    pub weight: usize,
    pub linear: bool,
    block_weights: Vec<usize>,
    checkpoints: Vec<Vec<usize>>,
}

impl IndexedChapter {
    fn new(href: String, title: String, blocks: Vec<String>, linear: bool) -> Self {
        let mut block_weights = vec![0];
        let mut checkpoints = Vec::new();
        for text in &blocks {
            let mut count = 0;
            let mut offsets = Vec::new();
            for (index, (offset, _)) in text.char_indices().enumerate() {
                if index % 256 == 0 {
                    offsets.push(offset);
                }
                count = index + 1;
            }
            block_weights.push(block_weights.last().copied().unwrap_or(0) + count);
            checkpoints.push(offsets);
        }
        let weight = block_weights.last().copied().unwrap_or(0).max(1);
        Self {
            href,
            title,
            blocks,
            weight,
            linear,
            block_weights,
            checkpoints,
        }
    }

    fn position_weight(&self, point: ContentPoint) -> usize {
        let before = self
            .block_weights
            .get(point.block)
            .copied()
            .unwrap_or(self.weight);
        let Some(text) = self.blocks.get(point.block) else {
            return before;
        };
        let offset = point.offset.min(text.len());
        let Some(checkpoints) = self.checkpoints.get(point.block) else {
            return before;
        };
        if checkpoints.is_empty() {
            return before;
        }
        let checkpoint = checkpoints
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        // Count at most 256 characters on the UI thread, even in a huge paragraph.
        before
            + checkpoint * 256
            + text[checkpoints[checkpoint]..]
                .char_indices()
                .take_while(|(start, _)| *start + checkpoints[checkpoint] < offset)
                .count()
    }
}

#[derive(Default)]
pub(super) struct BookIndex {
    pub chapters: Vec<IndexedChapter>,
    pub total: usize,
    pub incomplete: bool,
}

#[derive(Clone, Debug)]
pub(super) struct SearchMatch {
    pub href: String,
    pub title: String,
    pub excerpt: String,
    pub start: ContentPoint,
    pub end: ContentPoint,
}

impl BookIndex {
    pub fn build(book: &Book, cancel: &AtomicBool) -> Result<Self, String> {
        let mut index = Self::default();
        let mut queue: VecDeque<_> = book
            .sections
            .iter()
            .filter(|section| !book.is_cover(&section.href))
            .map(|section| (section.href.clone(), section.title.clone(), true))
            .collect();
        let mut seen: HashSet<String> = book
            .sections
            .iter()
            .map(|section| section.href.clone())
            .collect();
        while let Some((href, title, linear)) = queue.pop_front() {
            check_cancel(cancel)?;
            match book.chapter(&href, cancel) {
                Ok(chapter) => {
                    for block in &chapter.blocks {
                        if let Block::Text { spans, .. } = block {
                            for span in spans {
                                if let Some(target) = &span.style.link {
                                    let target = target.split('#').next().unwrap_or(target);
                                    if !book.is_cover(target)
                                        && book.readable_location(target)
                                        && seen.insert(target.to_owned())
                                    {
                                        let title = book
                                            .toc
                                            .iter()
                                            .find(|item| {
                                                item.target.as_deref().is_some_and(|href| {
                                                    href.split('#').next() == Some(target)
                                                })
                                            })
                                            .map(|item| item.title.clone())
                                            .unwrap_or_else(|| "Supplement".into());
                                        queue.push_back((target.to_owned(), title, false));
                                    }
                                }
                            }
                        }
                    }
                    let blocks: Vec<_> = chapter
                        .blocks
                        .iter()
                        .map(|block| match block {
                            Block::Text { text, .. } => text.clone(),
                            _ => String::new(),
                        })
                        .collect();
                    let chapter = IndexedChapter::new(href, title, blocks, linear);
                    if linear {
                        index.total += chapter.weight;
                    }
                    index.chapters.push(chapter);
                }
                Err(_) => {
                    check_cancel(cancel)?;
                    index.incomplete = true;
                }
            }
        }
        Ok(index)
    }

    pub fn progress(&self, href: &str, point: ContentPoint) -> Option<u8> {
        let mut before = 0;
        for chapter in &self.chapters {
            if !chapter.linear {
                continue;
            }
            if chapter.href == href {
                let within = chapter.position_weight(point);
                return Some(
                    ((before + within.min(chapter.weight)) as f64 * 100.0
                        / self.total.max(1) as f64)
                        .round()
                        .clamp(0.0, 100.0) as u8,
                );
            }
            before += chapter.weight;
        }
        None
    }

    pub fn search(&self, query: &str, cancel: &AtomicBool) -> Result<Vec<SearchMatch>, String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(vec![]);
        }
        let mut results = Vec::new();
        for chapter in &self.chapters {
            for (block, text) in chapter.blocks.iter().enumerate() {
                check_cancel(cancel)?;
                // Case conversion can change UTF-8 length (e.g. dotted I).
                // Map every folded byte back to its original character range.
                let mut folded = String::new();
                let mut offsets = Vec::new();
                for (index, (start, ch)) in text.char_indices().enumerate() {
                    if index % 4096 == 0 {
                        check_cancel(cancel)?;
                    }
                    let lower: String = ch.to_lowercase().collect();
                    offsets.extend(std::iter::repeat_n(
                        (start, start + ch.len_utf8()),
                        lower.len(),
                    ));
                    folded.push_str(&lower);
                }
                for (offset, _) in folded.match_indices(&query) {
                    let start = offsets[offset].0;
                    let end = offsets[offset + query.len() - 1].1;
                    let prefix: String = text[..start]
                        .chars()
                        .rev()
                        .take(32)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                    let suffix: String = text[end..].chars().take(48).collect();
                    results.push(SearchMatch {
                        href: chapter.href.clone(),
                        title: chapter.title.clone(),
                        excerpt: format!("{prefix}{}{suffix}", &text[start..end]),
                        start: ContentPoint {
                            block,
                            offset: start,
                        },
                        end: ContentPoint { block, offset: end },
                    });
                    if results.len() >= 200 {
                        return Ok(results);
                    }
                }
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_matches_keep_original_offsets_and_progress_is_content_based() {
        let index = BookIndex {
            chapters: vec![IndexedChapter::new(
                "c".into(),
                "Chapter".into(),
                vec!["café İ 字 café".into()],
                true,
            )],
            total: 13,
            incomplete: false,
        };
        let cancel = AtomicBool::new(false);
        let matches = index.search("CAFÉ", &cancel).unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].end.offset, 5);
        let matches = index.search("i", &cancel).unwrap();
        assert_eq!(
            &index.chapters[0].blocks[0][matches[0].start.offset..matches[0].end.offset],
            "İ"
        );
        assert_eq!(index.progress("c", ContentPoint::default()), Some(0));
        let long = "café 字 İ ".repeat(1000);
        let chapter = IndexedChapter::new(
            "long".into(),
            "Long".into(),
            vec!["Before".into(), long.clone()],
            true,
        );
        for (offset, _) in long.char_indices().step_by(137) {
            assert_eq!(
                chapter.position_weight(ContentPoint { block: 1, offset }),
                6 + long[..offset].chars().count()
            );
        }
        assert_eq!(
            chapter.position_weight(ContentPoint {
                block: 2,
                offset: 0
            }),
            chapter.weight
        );
        assert!(index.search(" ", &cancel).unwrap().is_empty());
    }
}
