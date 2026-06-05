use std::ops::Range;

use gpui::{
    Bounds, Pixels, Point, ShapedLine, TextRun, UTF16Selection, UnderlineStyle, point, px, rgb,
};

pub(super) const EDITABLE_TEXT_SELECTION_BACKGROUND: u32 = 0x0078d7;
pub(super) const EDITABLE_TEXT_SELECTION_FOREGROUND: u32 = 0xffffff;

#[derive(Clone)]
pub(super) struct EditableTextState {
    pub(super) content: String,
    pub(super) selected_range: Range<usize>,
    pub(super) selection_reversed: bool,
    pub(super) marked_range: Option<Range<usize>>,
    pub(super) last_layout: Option<ShapedLine>,
    pub(super) last_bounds: Option<Bounds<Pixels>>,
    pub(super) scroll_offset: Pixels,
    pub(super) is_selecting: bool,
}

impl EditableTextState {
    pub(super) fn new(content: String) -> Self {
        let selected_range = 0..content.len();
        Self::with_selection(content, selected_range)
    }

    pub(super) fn with_selection(content: String, selected_range: Range<usize>) -> Self {
        Self {
            content,
            selected_range,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            scroll_offset: px(0.0),
            is_selecting: false,
        }
    }

    pub(super) fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    pub(super) fn move_to(&mut self, offset: usize) {
        let offset = self.clamp_to_boundary(offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.scroll_cursor_into_view();
    }

    pub(super) fn select_to(&mut self, offset: usize) {
        let offset = self.clamp_to_boundary(offset);
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }

        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.scroll_cursor_into_view();
    }

    pub(super) fn select_all(&mut self) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        self.scroll_cursor_into_view();
    }

    pub(super) fn select_word_at(&mut self, offset: usize) {
        let offset = self.clamp_to_boundary(offset);
        let Some(word_offset) = self.word_offset_near(offset) else {
            self.move_to(offset);
            return;
        };
        let start = self.word_start(word_offset);
        let end = self.word_end(word_offset);
        self.selected_range = start..end;
        self.selection_reversed = false;
        self.scroll_cursor_into_view();
    }

    fn word_offset_near(&self, offset: usize) -> Option<usize> {
        let offset = self.clamp_to_boundary(offset);

        if let Some((_, ch)) = self.next_char(offset)
            && ch.is_alphanumeric()
        {
            return Some(offset);
        }

        let previous =
            self.content
                .get(..offset)?
                .char_indices()
                .rev()
                .find_map(|(previous_offset, ch)| {
                    ch.is_alphanumeric().then_some((
                        previous_offset,
                        offset.saturating_sub(previous_offset + ch.len_utf8()),
                    ))
                });
        let next = self
            .content
            .get(offset..)?
            .char_indices()
            .find_map(|(relative_offset, ch)| {
                ch.is_alphanumeric()
                    .then_some((offset + relative_offset, relative_offset))
            });

        match (previous, next) {
            (Some((previous_offset, previous_distance)), Some((next_offset, next_distance))) => {
                if previous_distance <= next_distance {
                    Some(previous_offset)
                } else {
                    Some(next_offset)
                }
            }
            (Some((previous_offset, _)), None) => Some(previous_offset),
            (None, Some((next_offset, _))) => Some(next_offset),
            (None, None) => None,
        }
    }

    fn word_start(&self, mut offset: usize) -> usize {
        while let Some((previous_offset, ch)) = self.previous_char(offset) {
            if !ch.is_alphanumeric() {
                break;
            }
            offset = previous_offset;
        }
        offset
    }

    fn word_end(&self, mut offset: usize) -> usize {
        while let Some((next_offset, ch)) = self.next_char(offset) {
            if !ch.is_alphanumeric() {
                break;
            }
            offset = next_offset;
        }
        offset
    }

    pub(super) fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .rev()
            .find_map(|(ix, _)| (ix < offset).then_some(ix))
            .unwrap_or(0)
    }

    pub(super) fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .find_map(|(ix, _)| (ix > offset).then_some(ix))
            .unwrap_or(self.content.len())
    }

    pub(super) fn previous_word_boundary(&self, offset: usize) -> usize {
        let mut offset = self.clamp_to_boundary(offset);

        while let Some((previous_offset, ch)) = self.previous_char(offset) {
            if ch.is_alphanumeric() {
                break;
            }
            offset = previous_offset;
        }

        while let Some((previous_offset, ch)) = self.previous_char(offset) {
            if !ch.is_alphanumeric() {
                break;
            }
            offset = previous_offset;
        }

        offset
    }

    pub(super) fn next_word_boundary(&self, offset: usize) -> usize {
        let mut offset = self.clamp_to_boundary(offset);

        while let Some((next_offset, ch)) = self.next_char(offset) {
            if !ch.is_alphanumeric() {
                break;
            }
            offset = next_offset;
        }

        while let Some((next_offset, ch)) = self.next_char(offset) {
            if ch.is_alphanumeric() {
                break;
            }
            offset = next_offset;
        }

        offset
    }

    fn previous_char(&self, offset: usize) -> Option<(usize, char)> {
        self.content
            .get(..offset)?
            .char_indices()
            .next_back()
            .map(|(ix, ch)| (ix, ch))
    }

    fn next_char(&self, offset: usize) -> Option<(usize, char)> {
        let ch = self.content.get(offset..)?.chars().next()?;
        Some((offset + ch.len_utf8(), ch))
    }

    pub(super) fn clamp_to_boundary(&self, offset: usize) -> usize {
        if offset >= self.content.len() {
            return self.content.len();
        }

        if self.content.is_char_boundary(offset) {
            offset
        } else {
            self.previous_boundary(offset)
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    pub(super) fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    pub(super) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub(super) fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    pub(super) fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };

        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }

        self.clamp_to_boundary(
            line.closest_index_for_x(position.x - bounds.left() + self.scroll_offset),
        )
    }

    pub(super) fn update_layout(&mut self, line: ShapedLine, bounds: Bounds<Pixels>) {
        self.last_layout = Some(line);
        self.last_bounds = Some(bounds);
        self.scroll_cursor_into_view();
    }

    pub(super) fn scroll_cursor_into_view(&mut self) {
        let (Some(line), Some(bounds)) = (self.last_layout.as_ref(), self.last_bounds.as_ref())
        else {
            return;
        };

        self.scroll_offset = scroll_offset_for_cursor(
            self.scroll_offset,
            line.x_for_index(self.cursor_offset()),
            line.width,
            bounds.right() - bounds.left(),
        );
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        (!self.selected_range.is_empty())
            .then(|| self.content[self.selected_range.clone()].to_owned())
    }

    pub(super) fn replace_text(&mut self, range: Option<Range<usize>>, new_text: &str) {
        let range = range
            .or(self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.content.replace_range(range.clone(), new_text);
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_cursor_into_view();
    }

    pub(super) fn replace_text_in_range_utf16(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone());
        self.replace_text(range, new_text);
    }

    pub(super) fn replace_and_mark_text_in_range_utf16(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.content.replace_range(range.clone(), new_text);
        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        self.scroll_cursor_into_view();
    }

    pub(super) fn selected_text_range_utf16(&self) -> UTF16Selection {
        UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        }
    }

    pub(super) fn marked_text_range_utf16(&self) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    pub(super) fn unmark_text(&mut self) {
        self.marked_range = None;
    }

    pub(super) fn bounds_for_range(
        &self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + line.x_for_index(range.start) - self.scroll_offset,
                bounds.top(),
            ),
            point(
                bounds.left() + line.x_for_index(range.end) - self.scroll_offset,
                bounds.bottom(),
            ),
        ))
    }

    pub(super) fn character_index_for_point(&self, point: Point<Pixels>) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let line = self.last_layout.as_ref()?;
        let utf8_index = line.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(self.clamp_to_boundary(utf8_index)))
    }
}

pub(super) fn scroll_offset_for_cursor(
    current_offset: Pixels,
    cursor_x: Pixels,
    content_width: Pixels,
    viewport_width: Pixels,
) -> Pixels {
    let margin = px(4.0);
    if content_width <= viewport_width {
        return px(0.0);
    }

    let max_offset = (content_width - viewport_width + margin).max(px(0.0));
    let left_edge = current_offset + margin;
    let right_edge = current_offset + viewport_width - margin;

    if cursor_x < left_edge {
        (cursor_x - margin).max(px(0.0))
    } else if cursor_x > right_edge {
        (cursor_x - viewport_width + margin).clamp(px(0.0), max_offset)
    } else {
        current_offset.clamp(px(0.0), max_offset)
    }
}

pub(super) fn editable_text_runs(
    content_len: usize,
    base_run: TextRun,
    selected_range: &Range<usize>,
    marked_range: Option<&Range<usize>>,
) -> Vec<TextRun> {
    if content_len == 0 {
        return vec![base_run];
    }

    let selected_range = clamp_range(selected_range, content_len);
    let marked_range = marked_range.map(|range| clamp_range(range, content_len));

    let mut boundaries = vec![0, content_len];
    push_range_boundaries(&mut boundaries, &selected_range);
    if let Some(marked_range) = marked_range.as_ref() {
        push_range_boundaries(&mut boundaries, marked_range);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    boundaries
        .windows(2)
        .filter_map(|window| {
            let start = window[0];
            let end = window[1];
            (end > start).then(|| {
                let mut run = base_run.clone();
                run.len = end - start;
                if range_contains_segment(&selected_range, start, end) {
                    run.color = rgb(EDITABLE_TEXT_SELECTION_FOREGROUND).into();
                }
                if marked_range
                    .as_ref()
                    .is_some_and(|range| range_contains_segment(range, start, end))
                {
                    run.underline = Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    });
                }
                run
            })
        })
        .collect()
}

fn clamp_range(range: &Range<usize>, len: usize) -> Range<usize> {
    range.start.min(len)..range.end.min(len)
}

fn push_range_boundaries(boundaries: &mut Vec<usize>, range: &Range<usize>) {
    if !range.is_empty() {
        boundaries.push(range.start);
        boundaries.push(range.end);
    }
}

fn range_contains_segment(range: &Range<usize>, start: usize, end: usize) -> bool {
    !range.is_empty() && range.start <= start && end <= range.end
}

#[cfg(test)]
pub(super) fn text_x_for_mouse_x(
    mouse_x: Pixels,
    bounds_left: Pixels,
    scroll_offset: Pixels,
) -> Pixels {
    mouse_x - bounds_left + scroll_offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::font;

    fn test_run(len: usize) -> TextRun {
        TextRun {
            len,
            font: font(".SystemUIFont"),
            color: rgb(0x1f1f1f).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    fn run_colors(runs: &[TextRun]) -> Vec<u32> {
        runs.iter()
            .map(|run| {
                if run.color == rgb(EDITABLE_TEXT_SELECTION_FOREGROUND).into() {
                    EDITABLE_TEXT_SELECTION_FOREGROUND
                } else if run.color == rgb(0x1f1f1f).into() {
                    0x1f1f1f
                } else {
                    panic!("unexpected run color: {:?}", run.color);
                }
            })
            .collect()
    }

    fn run_lengths(runs: &[TextRun]) -> Vec<usize> {
        runs.iter().map(|run| run.len).collect()
    }

    fn run_underlines(runs: &[TextRun]) -> Vec<bool> {
        runs.iter().map(|run| run.underline.is_some()).collect()
    }

    #[test]
    fn editable_selection_colors_are_exact() {
        assert_eq!(EDITABLE_TEXT_SELECTION_FOREGROUND, 0xffffff);
        assert_eq!(EDITABLE_TEXT_SELECTION_BACKGROUND, 0x0078d7);
    }

    #[test]
    fn editable_text_runs_without_selection_keep_base_color() {
        let runs = editable_text_runs("alpha".len(), test_run("alpha".len()), &(2..2), None);

        assert_eq!(run_lengths(&runs), vec!["alpha".len()]);
        assert_eq!(run_colors(&runs), vec![0x1f1f1f]);
        assert_eq!(run_underlines(&runs), vec![false]);
    }

    #[test]
    fn editable_text_runs_color_full_selection_white() {
        let runs = editable_text_runs("alpha".len(), test_run("alpha".len()), &(0..5), None);

        assert_eq!(run_lengths(&runs), vec!["alpha".len()]);
        assert_eq!(run_colors(&runs), vec![EDITABLE_TEXT_SELECTION_FOREGROUND]);
        assert_eq!(run_underlines(&runs), vec![false]);
    }

    #[test]
    fn editable_text_runs_split_partial_selection() {
        let runs = editable_text_runs("abcdef".len(), test_run("abcdef".len()), &(2..5), None);

        assert_eq!(run_lengths(&runs), vec![2, 3, 1]);
        assert_eq!(
            run_colors(&runs),
            vec![0x1f1f1f, EDITABLE_TEXT_SELECTION_FOREGROUND, 0x1f1f1f]
        );
        assert_eq!(run_underlines(&runs), vec![false, false, false]);
    }

    #[test]
    fn editable_text_runs_preserve_marked_underline_without_selection() {
        let runs = editable_text_runs(
            "abcdef".len(),
            test_run("abcdef".len()),
            &(2..2),
            Some(&(1..4)),
        );

        assert_eq!(run_lengths(&runs), vec![1, 3, 2]);
        assert_eq!(run_colors(&runs), vec![0x1f1f1f, 0x1f1f1f, 0x1f1f1f]);
        assert_eq!(run_underlines(&runs), vec![false, true, false]);
    }

    #[test]
    fn editable_text_runs_preserve_marked_underline_across_selection_overlap() {
        let runs = editable_text_runs(
            "abcdef".len(),
            test_run("abcdef".len()),
            &(2..5),
            Some(&(1..4)),
        );

        assert_eq!(run_lengths(&runs), vec![1, 1, 2, 1, 1]);
        assert_eq!(
            run_colors(&runs),
            vec![
                0x1f1f1f,
                0x1f1f1f,
                EDITABLE_TEXT_SELECTION_FOREGROUND,
                EDITABLE_TEXT_SELECTION_FOREGROUND,
                0x1f1f1f
            ]
        );
        assert_eq!(run_underlines(&runs), vec![false, true, true, false, false]);
    }

    #[test]
    fn editable_text_runs_filter_zero_length_boundaries() {
        let runs = editable_text_runs(
            "alpha".len(),
            test_run("alpha".len()),
            &(2..2),
            Some(&(3..3)),
        );

        assert_eq!(run_lengths(&runs), vec!["alpha".len()]);
        assert_eq!(run_colors(&runs), vec![0x1f1f1f]);
        assert_eq!(run_underlines(&runs), vec![false]);
    }

    #[test]
    fn word_selection_selects_alphanumeric_runs() {
        let mut input = EditableTextState::new("alpha/beta gamma".to_owned());

        input.select_word_at("al".len());
        assert_eq!(input.selected_range, 0.."alpha".len());

        input.select_word_at("alpha/".len());
        assert_eq!(input.selected_range, "alpha/".len().."alpha/beta".len());
    }

    #[test]
    fn word_selection_uses_nearest_word_across_separators() {
        let mut input = EditableTextState::new("alpha--beta".to_owned());

        input.select_word_at("alpha-".len());
        assert_eq!(input.selected_range, 0.."alpha".len());

        input.select_word_at("alpha--".len());
        assert_eq!(input.selected_range, "alpha--".len().."alpha--beta".len());
    }

    #[test]
    fn selection_clamps_to_utf8_boundaries() {
        let mut input = EditableTextState::new("aé beta".to_owned());

        input.move_to(2);
        assert_eq!(input.selected_range, 1..1);

        input.select_word_at(2);
        assert_eq!(input.selected_range, 0.."aé".len());
    }

    #[test]
    fn select_to_tracks_reversed_selection() {
        let mut input = EditableTextState::new("alpha beta gamma".to_owned());
        input.move_to("alpha beta".len());

        input.select_to("alpha ".len());
        assert_eq!(input.selected_range, "alpha ".len().."alpha beta".len());
        assert!(input.selection_reversed);

        input.select_to("alpha beta gamma".len());
        assert_eq!(
            input.selected_range,
            "alpha beta".len().."alpha beta gamma".len()
        );
        assert!(!input.selection_reversed);
    }

    #[test]
    fn utf16_ranges_round_trip_surrogate_pairs() {
        let input = EditableTextState::new("a😀b".to_owned());
        let range = "a".len().."a😀".len();

        let utf16 = input.range_to_utf16(&range);
        assert_eq!(utf16, 1..3);
        assert_eq!(input.range_from_utf16(&utf16), range);
    }

    #[test]
    fn replace_and_mark_text_updates_mark_and_selection() {
        let mut input = EditableTextState::new("alpha beta".to_owned());
        input.selected_range = "alpha ".len().."alpha beta".len();

        input.replace_and_mark_text_in_range_utf16(None, "delta", Some(1..3));

        assert_eq!(input.content, "alpha delta");
        assert_eq!(
            input.marked_range,
            Some("alpha ".len().."alpha delta".len())
        );
        assert_eq!(input.selected_range, "alpha d".len().."alpha del".len());
    }

    #[test]
    fn scroll_offset_keeps_cursor_visible() {
        assert_eq!(
            scroll_offset_for_cursor(px(12.0), px(40.0), px(90.0), px(100.0)),
            px(0.0)
        );
        assert_eq!(
            scroll_offset_for_cursor(px(0.0), px(140.0), px(200.0), px(100.0)),
            px(44.0)
        );
        assert_eq!(
            scroll_offset_for_cursor(px(80.0), px(20.0), px(200.0), px(100.0)),
            px(16.0)
        );
    }

    #[test]
    fn mouse_text_x_includes_scroll_offset() {
        assert_eq!(text_x_for_mouse_x(px(60.0), px(20.0), px(80.0)), px(120.0));
    }
}
