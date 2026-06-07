use rustc_hash::FxHashMap;

pub const NGRAM_LENGTH: usize = 3;
const INACTIVE_POSITION: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct PostingRange {
    start: u32,
    len: u32,
}

pub struct NgramIndexBuilder<T> {
    postings: FxHashMap<u32, Vec<u32>>,
    values: Vec<T>,
    ngram_scratch: Vec<u32>,
}

impl<T> Default for NgramIndexBuilder<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> NgramIndexBuilder<T> {
    pub fn new() -> Self {
        Self {
            postings: FxHashMap::default(),
            values: Vec::new(),
            ngram_scratch: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn add(&mut self, value: &str, data: T) {
        let index = u32::try_from(self.values.len()).expect("ngram index exceeds u32 capacity");
        self.values.push(data);

        self.ngram_scratch.clear();
        self.ngram_scratch
            .extend(value.as_bytes().windows(NGRAM_LENGTH).map(pack_ngram));
        self.ngram_scratch.sort_unstable();
        self.ngram_scratch.dedup();

        for &ngram in &self.ngram_scratch {
            self.postings.entry(ngram).or_default().push(index);
        }
    }

    pub fn finish(self) -> NgramIndex<T> {
        let posting_count = self.postings.values().map(Vec::len).sum();
        let mut posting_ranges =
            FxHashMap::with_capacity_and_hasher(self.postings.len(), Default::default());
        let mut postings = Vec::with_capacity(posting_count);

        for (ngram, ngram_postings) in self.postings {
            let start = u32::try_from(postings.len()).expect("ngram postings exceed u32 capacity");
            let len = u32::try_from(ngram_postings.len())
                .expect("ngram posting list exceeds u32 capacity");
            postings.extend(ngram_postings);
            posting_ranges.insert(ngram, PostingRange { start, len });
        }

        NgramIndex {
            posting_ranges,
            postings: postings.into_boxed_slice(),
            values: self.values.into_boxed_slice(),
        }
    }
}

pub struct NgramIndex<T> {
    posting_ranges: FxHashMap<u32, PostingRange>,
    postings: Box<[u32]>,
    values: Box<[T]>,
}

impl<T> Default for NgramIndex<T> {
    fn default() -> Self {
        NgramIndexBuilder::new().finish()
    }
}

impl<T> NgramIndex<T> {
    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, index: u32) -> Option<&T> {
        self.values.get(index as usize)
    }

    pub fn ngram_count(&self) -> usize {
        self.posting_ranges.len()
    }

    pub fn posting_count(&self) -> usize {
        self.postings.len()
    }

    pub fn posting_bytes(&self) -> usize {
        std::mem::size_of_val(self.postings.as_ref())
    }

    fn postings(&self, ngram: u32) -> &[u32] {
        let Some(range) = self.posting_ranges.get(&ngram) else {
            return &[];
        };
        let start = range.start as usize;
        &self.postings[start..start + range.len as usize]
    }
}

#[derive(Default)]
pub struct NgramSearchSession {
    query_ngrams: Vec<u32>,
    query_counts: Vec<(u32, u32)>,
    next_query_counts: Vec<(u32, u32)>,
    deltas: Vec<(u32, i32)>,
    scores: Vec<u32>,
    active_positions: Vec<u32>,
    active_ids: Vec<u32>,
    ranked_ids: Vec<u32>,
}

impl NgramSearchSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn search<'a, T>(&'a mut self, index: &NgramIndex<T>, value: &str) -> &'a [u32] {
        self.ensure_index_capacity(index.len());
        self.build_next_query_counts(value);
        if self.query_counts == self.next_query_counts {
            return &self.ranked_ids;
        }

        self.build_deltas();
        let deltas = std::mem::take(&mut self.deltas);
        for &(ngram, delta) in &deltas {
            self.apply_delta(index, ngram, delta);
        }
        self.deltas = deltas;
        std::mem::swap(&mut self.query_counts, &mut self.next_query_counts);

        self.ranked_ids.clear();
        self.ranked_ids.extend_from_slice(&self.active_ids);
        self.ranked_ids.sort_unstable_by(|&a, &b| {
            self.scores[b as usize]
                .cmp(&self.scores[a as usize])
                .then_with(|| a.cmp(&b))
        });
        &self.ranked_ids
    }

    pub fn reset(&mut self) {
        for &id in &self.active_ids {
            self.scores[id as usize] = 0;
            self.active_positions[id as usize] = INACTIVE_POSITION;
        }
        self.query_ngrams.clear();
        self.query_counts.clear();
        self.next_query_counts.clear();
        self.deltas.clear();
        self.active_ids.clear();
        self.ranked_ids.clear();
    }

    fn ensure_index_capacity(&mut self, len: usize) {
        if self.scores.len() == len {
            return;
        }

        self.query_counts.clear();
        self.next_query_counts.clear();
        self.scores.clear();
        self.scores.resize(len, 0);
        self.active_positions.clear();
        self.active_positions.resize(len, INACTIVE_POSITION);
        self.active_ids.clear();
        self.ranked_ids.clear();
    }

    fn build_next_query_counts(&mut self, value: &str) {
        self.query_ngrams.clear();
        self.query_ngrams
            .extend(value.as_bytes().windows(NGRAM_LENGTH).map(pack_ngram));
        self.query_ngrams.sort_unstable();

        self.next_query_counts.clear();
        for &ngram in &self.query_ngrams {
            match self.next_query_counts.last_mut() {
                Some((last_ngram, count)) if *last_ngram == ngram => *count += 1,
                _ => self.next_query_counts.push((ngram, 1)),
            }
        }
    }

    fn build_deltas(&mut self) {
        self.deltas.clear();
        let mut old_index = 0;
        let mut new_index = 0;

        while old_index < self.query_counts.len() || new_index < self.next_query_counts.len() {
            match (
                self.query_counts.get(old_index),
                self.next_query_counts.get(new_index),
            ) {
                (Some(&(old_ngram, old_count)), Some(&(new_ngram, new_count)))
                    if old_ngram == new_ngram =>
                {
                    let delta = new_count as i32 - old_count as i32;
                    if delta != 0 {
                        self.deltas.push((old_ngram, delta));
                    }
                    old_index += 1;
                    new_index += 1;
                }
                (Some(&(old_ngram, old_count)), Some(&(new_ngram, _))) if old_ngram < new_ngram => {
                    self.deltas.push((old_ngram, -(old_count as i32)));
                    old_index += 1;
                }
                (Some(_), Some(&(new_ngram, new_count))) => {
                    self.deltas.push((new_ngram, new_count as i32));
                    new_index += 1;
                }
                (Some(&(old_ngram, old_count)), None) => {
                    self.deltas.push((old_ngram, -(old_count as i32)));
                    old_index += 1;
                }
                (None, Some(&(new_ngram, new_count))) => {
                    self.deltas.push((new_ngram, new_count as i32));
                    new_index += 1;
                }
                (None, None) => break,
            }
        }
    }

    fn apply_delta<T>(&mut self, index: &NgramIndex<T>, ngram: u32, delta: i32) {
        for &id in index.postings(ngram) {
            let score = &mut self.scores[id as usize];
            if delta > 0 {
                if *score == 0 {
                    self.active_positions[id as usize] = self.active_ids.len() as u32;
                    self.active_ids.push(id);
                }
                *score += delta as u32;
            } else {
                *score -= delta.unsigned_abs();
                if *score == 0 {
                    let position = self.active_positions[id as usize] as usize;
                    self.active_ids.swap_remove(position);
                    if let Some(&moved_id) = self.active_ids.get(position) {
                        self.active_positions[moved_id as usize] = position as u32;
                    }
                    self.active_positions[id as usize] = INACTIVE_POSITION;
                }
            }
        }
    }
}

fn pack_ngram(window: &[u8]) -> u32 {
    u32::from(window[0]) << 16 | u32::from(window[1]) << 8 | u32::from(window[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build<'a>(values: &'a [(&'a str, &'a str)]) -> NgramIndex<&'a str> {
        let mut builder = NgramIndexBuilder::new();
        for &(value, data) in values {
            builder.add(value, data);
        }
        builder.finish()
    }

    fn search(index: &NgramIndex<&str>, query: &str) -> Vec<(u32, u32)> {
        let mut session = NgramSearchSession::new();
        let ids = session.search(index, query).to_vec();
        ids.iter()
            .map(|&id| (id, session.scores[id as usize]))
            .collect()
    }

    #[test]
    fn builder_finishes_compact_index_and_reports_statistics() {
        let mut builder = NgramIndexBuilder::new();
        assert!(builder.is_empty());
        builder.add("abcdef", "payload");
        assert_eq!(builder.len(), 1);

        let index = builder.finish();
        assert_eq!(index.len(), 1);
        assert_eq!(index.ngram_count(), 4);
        assert_eq!(index.posting_count(), 4);
        assert_eq!(index.posting_bytes(), 4 * size_of::<u32>());
        assert_eq!(index.get(0), Some(&"payload"));
    }

    #[test]
    fn returns_no_matches_for_queries_shorter_than_trigram_length() {
        let index = build(&[("abcdef", "payload")]);
        assert!(search(&index, "ab").is_empty());
    }

    #[test]
    fn generates_raw_utf8_byte_trigrams() {
        let index = build(&[("éa", "payload")]);
        assert_eq!(search(&index, "éa"), vec![(0, 1)]);
    }

    #[test]
    fn sorts_by_match_weight_then_insertion_order() {
        let index = build(&[
            ("bcdxxx", "weak"),
            ("abcdef", "strong"),
            ("bcdefg", "strong"),
        ]);

        assert_eq!(search(&index, "bcdef"), vec![(1, 3), (2, 3), (0, 1)]);
    }

    #[test]
    fn repeated_query_trigrams_increase_match_score() {
        let index = build(&[("aaa", "payload")]);
        assert_eq!(search(&index, "aaaaa"), vec![(0, 3)]);
    }

    #[test]
    fn repeated_indexed_trigrams_create_one_posting() {
        let index = build(&[("aaaaa", "payload")]);
        assert_eq!(index.ngram_count(), 1);
        assert_eq!(index.posting_count(), 1);
        assert_eq!(search(&index, "aaa"), vec![(0, 1)]);
    }

    #[test]
    fn incremental_append_and_backspace_update_exact_scores() {
        let index = build(&[("abcdef", "first"), ("bcdefg", "second")]);
        let mut session = NgramSearchSession::new();

        assert_eq!(session.search(&index, "bcd"), &[0, 1]);
        assert_eq!(session.search(&index, "bcde"), &[0, 1]);
        assert_eq!(session.scores, vec![2, 2]);
        assert_eq!(session.search(&index, "bcdef"), &[0, 1]);
        assert_eq!(session.scores, vec![3, 3]);
        assert_eq!(session.search(&index, "bcd"), &[0, 1]);
        assert_eq!(session.scores, vec![1, 1]);
    }

    #[test]
    fn switching_to_unmatched_query_removes_active_ids() {
        let index = build(&[("abcdef", "payload")]);
        let mut session = NgramSearchSession::new();

        assert_eq!(session.search(&index, "abc"), &[0]);
        assert!(session.search(&index, "xyz").is_empty());
        assert!(session.active_ids.is_empty());
        assert_eq!(session.scores, vec![0]);
    }

    #[test]
    fn repeated_identical_query_reuses_ranking() {
        let index = build(&[("abcdef", "payload")]);
        let mut session = NgramSearchSession::new();

        assert_eq!(session.search(&index, "abc"), &[0]);
        let ranked_ptr = session.ranked_ids.as_ptr();
        assert_eq!(session.search(&index, "abc"), &[0]);
        assert_eq!(session.ranked_ids.as_ptr(), ranked_ptr);
    }

    #[test]
    fn reset_clears_incremental_state() {
        let index = build(&[("abcdef", "payload")]);
        let mut session = NgramSearchSession::new();
        session.search(&index, "abc");

        session.reset();

        assert!(session.active_ids.is_empty());
        assert_eq!(session.scores, vec![0]);
        assert_eq!(session.search(&index, "abc"), &[0]);
    }
}
