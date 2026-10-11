use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    rc::Rc,
};

use gpui::{Font, TextRun, WindowTextSystem, px, rgb};

use crate::explorer::{
    constants::{
        LARGE_ICON_ROW_GAP, LARGE_ICON_SIZE, LARGE_ICON_TEXT_BOTTOM_PADDING,
        LARGE_ICON_TEXT_LINE_HEIGHT, LARGE_ICON_TEXT_ROWS, LARGE_ICON_TEXT_SIZE,
        LARGE_ICON_TEXT_TOP_GAP, LARGE_ICON_TILE_WIDTH,
    },
    entry::FileEntry,
    mouse_selection::large_icon_grid_columns,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LargeIconLayoutCacheKey {
    entries_revision: u64,
    entry_count: usize,
    viewport_width_px: u32,
    show_file_name_extensions: bool,
    font: Font,
}

const FILENAME_MEASUREMENT_CAPACITY: usize = 10_000;

/// Shared filename metrics for this view's current and recently visited folders.
/// The current layout owns its heights independently of this bounded reuse cache.
#[derive(Default)]
pub(super) struct LargeIconFilenameCache {
    font: Option<Font>,
    heights: HashMap<Rc<str>, (f32, u64)>,
    recency: BTreeMap<u64, Rc<str>>,
    clock: u64,
    #[cfg(test)]
    measurements: usize,
}

impl LargeIconFilenameCache {
    #[cfg(test)]
    pub(super) fn measurement_count(&self) -> usize {
        self.measurements
    }

    pub(super) fn tile_heights(
        &mut self,
        entries: &[FileEntry],
        extensions: bool,
        font: &Font,
        text_system: &WindowTextSystem,
    ) -> Rc<[f32]> {
        if self.font.as_ref() != Some(font) {
            self.heights.clear();
            self.recency.clear();
            self.font = Some(font.clone());
        }
        // Capture hits before inserting misses. Otherwise visiting a small child
        // folder and returning to a capacity-sized parent causes sequential LRU
        // eviction to discard filenames that this same batch still needs.
        let mut heights = entries
            .iter()
            .map(|entry| {
                self.cached_height(entry.display_name_with_extensions(extensions))
                    .unwrap_or(f32::NAN)
            })
            .collect::<Vec<_>>();
        if heights.iter().all(|height| height.is_finite()) {
            return heights.into();
        }
        for (entry, height) in entries.iter().zip(&mut heights) {
            if height.is_finite() {
                continue;
            }
            let text = entry.display_name_with_extensions(extensions);
            // Repeated displayed names in one directory share one measurement.
            if let Some(cached) = self.cached_height(text) {
                *height = cached;
                continue;
            }
            let rows = large_icon_filename_lines(text, font, text_system).len();
            *height = large_icon_tile_height_for_rows(rows);
            #[cfg(test)]
            {
                self.measurements += 1;
            }
            if self.heights.len() == FILENAME_MEASUREMENT_CAPACITY {
                let (_, oldest) = self.recency.pop_first().expect("nonempty filename cache");
                self.heights.remove(oldest.as_ref());
            }
            let key: Rc<str> = Rc::from(text);
            self.heights.insert(key.clone(), (*height, self.clock));
            self.recency.insert(self.clock, key);
        }
        heights.into()
    }

    fn cached_height(&mut self, text: &str) -> Option<f32> {
        if self.clock == u64::MAX {
            self.heights.clear();
            self.recency.clear();
            self.clock = 0;
        }
        self.clock += 1;
        let (key, &(height, old_tick)) = self.heights.get_key_value(text)?;
        let key = key.clone();
        self.recency.remove(&old_tick);
        self.heights.get_mut(text).expect("cached filename").1 = self.clock;
        self.recency.insert(self.clock, key);
        Some(height)
    }
}

#[derive(Clone, Debug)]
pub(super) struct LargeIconLayout {
    pub(super) columns: usize,
    pub(super) column_gap: f32,
    rows: Rc<[LargeIconRowLayout]>,
    tile_heights: Rc<[f32]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LargeIconRowLayout {
    pub(super) top: f32,
    pub(super) height: f32,
    pub(super) tile_height: f32,
}

impl LargeIconLayout {
    pub(super) fn from_measured_heights(tile_heights: Rc<[f32]>, viewport_width: f32) -> Self {
        let columns = large_icon_grid_columns(viewport_width);
        Self::from_shared_tile_heights(
            columns,
            large_icon_column_gap(viewport_width, columns),
            tile_heights,
        )
    }

    pub(super) fn with_viewport_width(&self, viewport_width: f32) -> Self {
        let columns = large_icon_grid_columns(viewport_width);
        if columns == self.columns {
            let mut layout = self.clone();
            layout.column_gap = large_icon_column_gap(viewport_width, columns);
            layout
        } else {
            Self::from_measured_heights(self.tile_heights.clone(), viewport_width)
        }
    }

    pub(super) fn from_tile_heights(
        columns: usize,
        column_gap: f32,
        tile_heights: Vec<f32>,
    ) -> Self {
        Self::from_shared_tile_heights(columns, column_gap, tile_heights.into())
    }

    fn from_shared_tile_heights(columns: usize, column_gap: f32, tile_heights: Rc<[f32]>) -> Self {
        let columns = columns.max(1);
        let rows = large_icon_rows_from_tile_heights(columns, &tile_heights);
        Self {
            columns,
            column_gap,
            rows: rows.into(),
            tile_heights,
        }
    }

    #[cfg(test)]
    pub(super) fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub(super) fn row_sizes(
        &self,
        width: f32,
    ) -> impl Iterator<Item = gpui::Size<gpui::Pixels>> + '_ {
        self.rows
            .iter()
            .map(move |row| gpui::size(px(width), px(row.height)))
    }

    pub(super) fn row_sizes_with_tail(
        &self,
        width: f32,
        tail: f32,
    ) -> Vec<gpui::Size<gpui::Pixels>> {
        let mut sizes = self.row_sizes(width).collect::<Vec<_>>();
        if let Some(last) = sizes.last_mut() {
            last.height += px(tail);
        }
        sizes
    }

    pub(super) fn scroll_offset_with_tail(&self, scroll_top: f32) -> gpui::ListOffset {
        if let Some(last) = self.rows.last()
            && scroll_top >= last.top
        {
            gpui::ListOffset {
                item_ix: self.rows.len() - 1,
                offset_in_item: px(scroll_top - last.top),
            }
        } else {
            self.scroll_offset(scroll_top)
        }
    }

    pub(super) fn row_for_index(&self, ix: usize) -> usize {
        ix / self.columns
    }

    pub(super) fn row_bounds(&self, row: usize) -> Option<LargeIconRowLayout> {
        self.rows.get(row).copied()
    }

    pub(super) fn content_height(&self) -> f32 {
        self.rows.last().map_or(0.0, |row| row.top + row.height)
    }

    pub(super) fn scroll_top(&self, offset: gpui::ListOffset) -> f32 {
        self.row_bounds(offset.item_ix)
            .map_or(self.content_height(), |row| row.top)
            + f32::from(offset.offset_in_item)
    }

    pub(super) fn scroll_offset(&self, scroll_top: f32) -> gpui::ListOffset {
        let scroll_top = scroll_top.max(0.0);
        let row_ix = self
            .rows
            .partition_point(|row| row.top + row.height <= scroll_top);
        let row_top = self
            .row_bounds(row_ix)
            .map_or(self.content_height(), |row| row.top);
        gpui::ListOffset {
            item_ix: row_ix,
            offset_in_item: px(scroll_top - row_top),
        }
    }

    pub(super) fn scroll_top_revealing_index(
        &self,
        scroll_top: f32,
        viewport_height: f32,
        ix: usize,
    ) -> f32 {
        let Some((_, top, _, height)) = self.index_bounds(ix) else {
            return scroll_top;
        };
        if top < scroll_top {
            top
        } else if top + height > scroll_top + viewport_height {
            (top + height - viewport_height).max(0.0)
        } else {
            scroll_top
        }
    }

    pub(super) fn tile_height(&self, ix: usize) -> Option<f32> {
        self.tile_heights.get(ix).copied()
    }

    pub(super) fn index_bounds(&self, ix: usize) -> Option<(f32, f32, f32, f32)> {
        let row = self.row_for_index(ix);
        let row_layout = self.row_bounds(row)?;
        let column = ix % self.columns;
        let stride = LARGE_ICON_TILE_WIDTH + self.column_gap;

        Some((
            column as f32 * stride,
            row_layout.top,
            LARGE_ICON_TILE_WIDTH,
            self.tile_height(ix)?,
        ))
    }

    pub(super) fn index_at_content_point(
        &self,
        content_x: f32,
        content_y: f32,
        entry_count: usize,
    ) -> Option<usize> {
        if content_x < 0.0 || content_y < 0.0 || entry_count == 0 {
            return None;
        }

        let row = self
            .rows
            .partition_point(|row| row.top + row.height <= content_y);
        let bounds = self.rows.get(row)?;
        if !(content_y >= bounds.top && content_y < bounds.top + bounds.tile_height) {
            return None;
        }
        let column = self.column_at_x(content_x)?;
        let ix = row * self.columns + column;
        (ix < entry_count).then_some(ix)
    }

    fn column_at_x(&self, content_x: f32) -> Option<usize> {
        for column in 0..self.columns {
            let left = column as f32 * (LARGE_ICON_TILE_WIDTH + self.column_gap);
            let right = left + LARGE_ICON_TILE_WIDTH;
            if content_x >= left && content_x < right {
                return Some(column);
            }
        }

        None
    }
}

impl LargeIconLayoutCacheKey {
    pub(super) fn new(
        entries_revision: u64,
        entry_count: usize,
        viewport_width: f32,
        show_file_name_extensions: bool,
        font: &Font,
    ) -> Self {
        Self {
            entries_revision,
            entry_count,
            viewport_width_px: viewport_width_key(viewport_width),
            show_file_name_extensions,
            font: font.clone(),
        }
    }

    pub(super) fn same_filename_metrics(&self, other: &Self) -> bool {
        self.entries_revision == other.entries_revision
            && self.entry_count == other.entry_count
            && self.show_file_name_extensions == other.show_file_name_extensions
            && self.font == other.font
    }
}

fn viewport_width_key(viewport_width: f32) -> u32 {
    viewport_width.max(0.0).to_bits()
}

pub(super) fn large_icon_filename_text_width() -> f32 {
    (LARGE_ICON_TILE_WIDTH - 8.0).max(0.0)
}

pub(super) fn large_icon_max_tile_height() -> f32 {
    large_icon_tile_height_for_rows(LARGE_ICON_TEXT_ROWS)
}

pub(super) fn large_icon_tile_height_for_rows(rows: usize) -> f32 {
    let rows = rows.clamp(1, LARGE_ICON_TEXT_ROWS);
    LARGE_ICON_SIZE
        + LARGE_ICON_TEXT_TOP_GAP
        + LARGE_ICON_TEXT_LINE_HEIGHT * rows as f32
        + LARGE_ICON_TEXT_BOTTOM_PADDING
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FilenameRow {
    /// Characters drawn on this row, excluding whitespace at a soft wrap.
    pub(super) display: Range<usize>,
    /// All source characters owned by this row, including wrap whitespace.
    pub(super) source: Range<usize>,
}

fn filename_width(text: &str, font: &Font, text_system: &WindowTextSystem) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: rgb(0).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        text_system
            .layout_line(text, px(LARGE_ICON_TEXT_SIZE), &[run], None)
            .width
            .ceil(),
    )
}

pub(super) fn large_icon_filename_rows(
    text: &str,
    font: &Font,
    text_system: &WindowTextSystem,
) -> Vec<FilenameRow> {
    let normalized = text.replace(['\r', '\n'], " ");
    layout_filename_rows(&normalized, large_icon_filename_text_width(), |line| {
        filename_width(line, font, text_system)
    })
}

/// Labels and the editor share soft-wrap boundaries, but only labels truncate.
pub(super) fn large_icon_filename_lines(
    text: &str,
    font: &Font,
    text_system: &WindowTextSystem,
) -> Vec<String> {
    layout_filename_lines(text, large_icon_filename_text_width(), |line| {
        filename_width(line, font, text_system)
    })
}

fn layout_filename_rows(
    text: &str,
    width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<FilenameRow> {
    let mut rows = Vec::new();
    let mut start = 0;
    loop {
        let remaining = &text[start..];
        let mut end = 0;
        for (ix, ch) in remaining.char_indices() {
            let next = ix + ch.len_utf8();
            if measure(&remaining[..next]) > width {
                break;
            }
            end = next;
        }
        if end == remaining.len() {
            rows.push(FilenameRow {
                display: start..text.len(),
                source: start..text.len(),
            });
            break;
        }
        // Even an unusually wide glyph must remain editable, and make progress.
        if end == 0 {
            end = remaining.chars().next().unwrap().len_utf8();
        }
        if !remaining[end..].starts_with(char::is_whitespace)
            && let Some((ix, _)) = remaining[..end]
                .char_indices()
                .rev()
                .find(|(ix, ch)| *ix > 0 && ch.is_whitespace())
        {
            end = ix;
        }
        let display_end = start + remaining[..end].trim_end().len();
        let next = text.len() - remaining[end..].trim_start().len();
        rows.push(FilenameRow {
            display: start..display_end,
            source: start..next,
        });
        start = next;
        if start == text.len() {
            break;
        }
    }
    rows
}

fn layout_filename_lines(
    text: &str,
    width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<String> {
    let text = text.replace(['\r', '\n'], " ");
    let rows = layout_filename_rows(&text, width, &mut measure);
    let mut lines = Vec::new();
    for (ix, row) in rows.iter().take(LARGE_ICON_TEXT_ROWS).enumerate() {
        let displayed = &text[row.display.clone()];
        if (ix == LARGE_ICON_TEXT_ROWS - 1 && rows.len() > LARGE_ICON_TEXT_ROWS)
            || measure(displayed) > width
        {
            let remaining = &text[row.source.start..];
            let mut last = "\u{2026}".to_owned();
            for (offset, ch) in remaining.char_indices() {
                let candidate =
                    format!("{}\u{2026}", remaining[..offset + ch.len_utf8()].trim_end());
                if measure(&candidate) > width {
                    break;
                }
                last = candidate;
            }
            lines.push(last);
            break;
        }
        lines.push(displayed.to_owned());
    }
    lines
}

fn large_icon_column_gap(viewport_width: f32, columns: usize) -> f32 {
    if columns > 1 {
        ((viewport_width - LARGE_ICON_TILE_WIDTH * columns as f32) / (columns - 1) as f32).max(0.0)
    } else {
        0.0
    }
}

fn large_icon_rows_from_tile_heights(
    columns: usize,
    tile_heights: &[f32],
) -> Vec<LargeIconRowLayout> {
    let mut rows = Vec::new();
    let mut top = 0.0;

    for row_tiles in tile_heights.chunks(columns.max(1)) {
        let tile_height = row_tiles.iter().copied().fold(0.0, f32::max);
        let height = tile_height + LARGE_ICON_ROW_GAP;
        rows.push(LargeIconRowLayout {
            top,
            height,
            tile_height,
        });
        top += height;
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monospace_lines(text: &str, width: usize) -> Vec<String> {
        layout_filename_lines(text, width as f32, |line| line.chars().count() as f32)
    }

    #[test]
    fn editable_rows_match_fitting_labels_and_preserve_wrap_whitespace() {
        for text in [
            "",
            "abcde",
            "abcde fghij",
            "abcde fghij klmno",
            "abcdefghijklmno",
            "a.b-c.txt",
            "abc  def   ghi  ",
            "  abc def",
            "\u{6587}\u{4ef6}\u{1f642}abc",
        ] {
            let rows = layout_filename_rows(text, 5.0, |line| line.chars().count() as f32);
            assert_eq!(
                rows.iter()
                    .map(|row| &text[row.source.clone()])
                    .collect::<String>(),
                text
            );
            let labels = monospace_lines(text, 5);
            for (ix, row) in rows.iter().take(3).enumerate() {
                assert!(text[row.display.clone()].chars().count() <= 5);
                if rows.len() <= 3 || ix < 2 {
                    assert_eq!(&text[row.display.clone()], labels[ix]);
                }
            }
        }
    }

    #[test]
    fn editable_rows_preserve_every_source_character_and_label_boundaries() {
        let text = "alpha  beta gamma delta epsilon zeta \u{6587}\u{4ef6}\u{1f642}longtoken";
        let measure = |line: &str| line.chars().count() as f32;
        let rows = layout_filename_rows(text, 10.0, measure);
        assert!(rows.len() > LARGE_ICON_TEXT_ROWS);
        assert_eq!(
            rows.iter()
                .map(|row| &text[row.source.clone()])
                .collect::<String>(),
            text
        );
        let labels = layout_filename_lines(text, 10.0, measure);
        for (row, label) in rows
            .iter()
            .zip(labels.iter())
            .take(LARGE_ICON_TEXT_ROWS - 1)
        {
            assert_eq!(&text[row.display.clone()], label);
        }
        for row in rows {
            assert!(text.is_char_boundary(row.display.start));
            assert!(text.is_char_boundary(row.display.end));
            assert!(measure(&text[row.display]) <= 10.0);
        }
    }

    #[test]
    fn rename_scroll_tail_keeps_tile_positions_and_supports_offsets_below_last_tile() {
        let layout = LargeIconLayout::from_tile_heights(1, 0.0, vec![100.0, 100.0]);
        let sizes = layout.row_sizes_with_tail(200.0, 80.0);
        assert_eq!(sizes[0].height, px(102.0));
        assert_eq!(sizes[1].height, px(182.0));
        assert_eq!(layout.index_bounds(1).unwrap().1, 102.0);
        let offset = layout.scroll_offset_with_tail(250.0);
        assert_eq!(offset.item_ix, 1);
        assert_eq!(layout.scroll_top(offset), 250.0);
    }

    #[test]
    fn long_spaced_filename_ellipsizes_the_third_row() {
        let lines = monospace_lines(
            "M. R. James - A Warning to the Curious and other Ghost Stories",
            15,
        );
        assert_eq!(
            lines,
            [
                "M. R. James - A",
                "Warning to the",
                "Curious and ot\u{2026}"
            ]
        );
        assert!(lines.iter().all(|line| line.chars().count() <= 15));
    }

    #[test]
    fn fitting_names_keep_all_text_without_an_ellipsis() {
        for (text, expected) in [
            ("", vec![""]),
            ("abcde", vec!["abcde"]),
            ("abcde fghij", vec!["abcde", "fghij"]),
            ("abcde fghij klmno", vec!["abcde", "fghij", "klmno"]),
            ("abcdefghijklmno", vec!["abcde", "fghij", "klmno"]),
            ("a (1944).mkv z", vec!["a", "(1944).mkv", "z"]),
        ] {
            let width = if text.contains("1944") { 10 } else { 5 };
            assert_eq!(monospace_lines(text, width), expected, "{text}");
        }
    }

    #[test]
    fn oversized_unicode_tokens_fit_and_keep_character_boundaries() {
        let text = "\u{6587}\u{4ef6}\u{1f642}".repeat(12);
        let lines = monospace_lines(&text, 5);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|line| line.chars().count() <= 5));
        assert!(lines[2].ends_with('\u{2026}'));
        assert_eq!(lines[0], text.chars().take(5).collect::<String>());
    }

    #[test]
    fn suffix_is_measured_together_with_the_final_prefix() {
        let measure = |line: &str| {
            line.chars()
                .map(|ch| if ch == '\u{2026}' { 3.0 } else { 1.0 })
                .sum::<f32>()
        };
        let lines = layout_filename_lines("abcdefghijklmnop", 5.0, measure);
        assert_eq!(lines, ["abcde", "fghij", "kl\u{2026}"]);
        assert!(lines.iter().all(|line| measure(line) <= 5.0));
    }

    #[test]
    fn filename_newlines_do_not_bypass_the_row_limit() {
        assert_eq!(
            monospace_lines("abc\ndef\nghi\njkl", 5),
            ["abc", "def", "ghi\u{2026}"]
        );
    }

    #[gpui::test]
    fn displayed_rows_fit_the_shaped_label_width(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| FilenameTestView);
        cx.update(|window, _| {
            let font = test_font(".SystemUIFont");
            for name in [
                "M. R. James - A Warning to the Curious and other Ghost Stories".to_owned(),
                "abcdefghij".repeat(20),
                "\u{6587}\u{4ef6}\u{1f642}".repeat(30),
                "a (1944).mkv brackets-and-punctuation.txt ".repeat(5),
            ] {
                let lines = large_icon_filename_lines(&name, &font, window.text_system());
                assert_eq!(lines.len(), LARGE_ICON_TEXT_ROWS);
                assert!(lines.last().unwrap().ends_with('\u{2026}'));
                for line in lines {
                    let run = TextRun {
                        len: line.len(),
                        font: font.clone(),
                        color: rgb(0).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let width = window
                        .text_system()
                        .layout_line(&line, px(LARGE_ICON_TEXT_SIZE), &[run], None)
                        .width;
                    assert!(
                        f32::from(width.ceil()) <= large_icon_filename_text_width(),
                        "{line}: {width:?}"
                    );
                }
            }
        });
    }

    #[test]
    fn cloned_layout_shares_geometry_and_supplies_exact_row_sizes() {
        let layout = LargeIconLayout::from_tile_heights(2, 3., vec![100., 140., 120.]);
        let cloned = layout.clone();
        assert!(Rc::ptr_eq(&layout.rows, &cloned.rows));
        assert!(Rc::ptr_eq(&layout.tile_heights, &cloned.tile_heights));
        assert_eq!(
            layout.row_sizes(213.).collect::<Vec<_>>(),
            vec![
                gpui::size(px(213.), px(142.)),
                gpui::size(px(213.), px(122.))
            ]
        );
        assert_eq!(layout.content_height(), 264.);
    }

    struct FilenameTestView;

    impl gpui::Render for FilenameTestView {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    fn test_font(name: &'static str) -> Font {
        gpui::font(name)
    }

    #[test]
    fn layout_cache_key_changes_when_viewport_width_changes() {
        let font = test_font(".SystemUIFont");

        let narrow = LargeIconLayoutCacheKey::new(1, 1, 300.0, true, &font);
        let wide = LargeIconLayoutCacheKey::new(1, 1, 301.0, true, &font);

        assert_ne!(narrow, wide);
        assert!(narrow.same_filename_metrics(&wide));
    }

    #[test]
    fn layout_cache_key_changes_when_font_changes() {
        let system = LargeIconLayoutCacheKey::new(1, 1, 300.0, true, &test_font(".SystemUIFont"));
        let custom = LargeIconLayoutCacheKey::new(1, 1, 300.0, true, &test_font("Segoe UI"));

        assert_ne!(system, custom);
    }

    #[test]
    fn layout_cache_key_changes_when_extension_visibility_changes() {
        let font = test_font(".SystemUIFont");

        let hidden = LargeIconLayoutCacheKey::new(1, 1, 300.0, false, &font);
        let visible = LargeIconLayoutCacheKey::new(1, 1, 300.0, true, &font);

        assert_ne!(hidden, visible);
    }

    #[test]
    fn layout_cache_key_changes_when_entries_are_revised() {
        let font = test_font(".SystemUIFont");

        let first_key = LargeIconLayoutCacheKey::new(1, 1, 300.0, true, &font);
        let second_key = LargeIconLayoutCacheKey::new(2, 1, 300.0, true, &font);

        assert_ne!(first_key, second_key);
        assert!(!first_key.same_filename_metrics(&second_key));
    }

    #[test]
    fn resizing_reuses_tile_metrics_and_repacking_reuses_heights() {
        let layout =
            LargeIconLayout::from_measured_heights(vec![100., 140., 120., 110., 140.].into(), 320.);
        let wider = layout.with_viewport_width(325.);
        assert!(Rc::ptr_eq(&layout.rows, &wider.rows));
        assert!(Rc::ptr_eq(&layout.tile_heights, &wider.tile_heights));
        assert_ne!(layout.column_gap, wider.column_gap);
        let narrower = layout.with_viewport_width(220.);
        assert!(!Rc::ptr_eq(&layout.rows, &narrower.rows));
        assert!(Rc::ptr_eq(&layout.tile_heights, &narrower.tile_heights));
        assert_eq!(narrower.row_count(), 3);
        assert_eq!(narrower.row_bounds(1).unwrap().top, 142.);
    }

    #[gpui::test]
    fn filename_measurements_match_wrapping_and_reuse_names(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| FilenameTestView);
        cx.update(|window, _| {
            let font = test_font(".SystemUIFont");
            let names = [
                "short.txt".to_string(),
                "résumé-文件 with several words.txt".into(),
                "very long words ".repeat(100),
            ];
            let mut entries = names
                .iter()
                .map(|name| FileEntry::test(name, false, None, None))
                .collect::<Vec<_>>();
            let mut cache = LargeIconFilenameCache::default();
            let heights = cache.tile_heights(&entries, true, &font, window.text_system());
            for (entry, &height) in entries.iter().zip(heights.iter()) {
                assert_eq!(
                    height,
                    large_icon_tile_height_for_rows(
                        large_icon_filename_lines(
                            entry.display_name_with_extensions(true),
                            &font,
                            window.text_system()
                        )
                        .len()
                    )
                );
            }
            assert_eq!(cache.measurements, 3);
            entries.reverse();
            let reordered = cache.tile_heights(&entries, true, &font, window.text_system());
            assert_eq!(
                reordered.iter().copied().collect::<Vec<_>>(),
                heights.iter().rev().copied().collect::<Vec<_>>()
            );
            cache.tile_heights(&entries[..1], true, &font, window.text_system());
            cache.tile_heights(&entries, true, &font, window.text_system());
            assert_eq!(cache.measurements, 3);
            cache.tile_heights(&entries, false, &font, window.text_system());
            assert_eq!(cache.measurements, 5); // The long name has no extension.
            cache.tile_heights(
                &entries,
                false,
                &test_font("Segoe UI"),
                window.text_system(),
            );
            assert_eq!(cache.measurements, 8);
        });
    }

    #[gpui::test]
    fn filename_measurement_cache_evicts_least_recently_used_names(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| FilenameTestView);
        cx.update(|window, _| {
            let font = test_font(".SystemUIFont");
            let entries = (0..FILENAME_MEASUREMENT_CAPACITY)
                .map(|ix| FileEntry::test(&format!("item-{ix}"), false, None, None))
                .collect::<Vec<_>>();
            let mut cache = LargeIconFilenameCache::default();
            let current_heights = cache.tile_heights(&entries, true, &font, window.text_system());
            cache.tile_heights(&entries[..1], true, &font, window.text_system());
            cache.tile_heights(
                &[FileEntry::test("new-name", false, None, None)],
                true,
                &font,
                window.text_system(),
            );
            assert_eq!(cache.heights.len(), FILENAME_MEASUREMENT_CAPACITY);
            assert_eq!(cache.recency.len(), FILENAME_MEASUREMENT_CAPACITY);
            assert!(cache.heights.contains_key("item-0"));
            assert!(!cache.heights.contains_key("item-1"));
            assert_eq!(current_heights.len(), FILENAME_MEASUREMENT_CAPACITY);
            assert!(
                current_heights
                    .iter()
                    .all(|height| *height >= large_icon_tile_height_for_rows(1))
            );
            let before_return = cache.measurements;
            cache.tile_heights(&entries, true, &font, window.text_system());
            assert_eq!(
                cache.measurements,
                before_return + 1,
                "returning to a full folder must not evict future hits in the same batch"
            );
        });
    }

    #[test]
    fn tile_height_grows_for_one_two_and_three_filename_rows() {
        let one = large_icon_tile_height_for_rows(1);
        let two = large_icon_tile_height_for_rows(2);
        let three = large_icon_tile_height_for_rows(3);

        assert!(one < two);
        assert!(two < three);
    }

    #[test]
    fn tile_height_includes_filename_bottom_padding() {
        assert_eq!(
            large_icon_tile_height_for_rows(1),
            LARGE_ICON_SIZE
                + LARGE_ICON_TEXT_TOP_GAP
                + LARGE_ICON_TEXT_LINE_HEIGHT
                + LARGE_ICON_TEXT_BOTTOM_PADDING
        );
    }

    #[test]
    fn tile_height_clamps_to_three_filename_rows() {
        assert_eq!(
            large_icon_tile_height_for_rows(4),
            large_icon_tile_height_for_rows(3)
        );
        assert_eq!(
            large_icon_max_tile_height(),
            large_icon_tile_height_for_rows(3)
        );
    }

    #[test]
    fn row_height_uses_tallest_tile_plus_gap() {
        let layout = LargeIconLayout::from_tile_heights(
            3,
            10.0,
            vec![
                large_icon_tile_height_for_rows(1),
                large_icon_tile_height_for_rows(3),
                large_icon_tile_height_for_rows(2),
            ],
        );

        let row = layout.row_bounds(0).expect("row");
        assert_eq!(row.tile_height, large_icon_tile_height_for_rows(3));
        assert_eq!(
            row.height,
            large_icon_tile_height_for_rows(3) + LARGE_ICON_ROW_GAP
        );
    }

    #[test]
    fn mixed_name_tiles_keep_individual_heights_inside_shared_row() {
        let layout = LargeIconLayout::from_tile_heights(
            2,
            10.0,
            vec![
                large_icon_tile_height_for_rows(1),
                large_icon_tile_height_for_rows(3),
            ],
        );

        let (_, short_top, _, short_height) = layout.index_bounds(0).expect("short bounds");
        let (_, tall_top, _, tall_height) = layout.index_bounds(1).expect("tall bounds");

        assert_eq!(short_top, tall_top);
        assert!(short_height < tall_height);
        assert_eq!(layout.row_bounds(0).unwrap().tile_height, tall_height);
    }

    #[test]
    fn scroll_offsets_round_trip_across_variable_height_rows() {
        let layout = LargeIconLayout::from_tile_heights(2, 0.0, vec![100.0, 160.0, 120.0, 100.0]);
        let second_row_top = layout.row_bounds(1).unwrap().top;
        for scroll_top in [
            0.0,
            19.0,
            second_row_top,
            second_row_top + 37.0,
            layout.content_height(),
        ] {
            assert_eq!(
                layout.scroll_top(layout.scroll_offset(scroll_top)),
                scroll_top
            );
        }
        assert_eq!(layout.scroll_offset(second_row_top).item_ix, 1);
        let empty = LargeIconLayout::from_tile_heights(1, 0.0, vec![]);
        assert_eq!(empty.scroll_top(empty.scroll_offset(0.0)), 0.0);
    }

    #[test]
    fn reveal_preserves_visible_position_and_moves_only_to_item_edge() {
        let layout = LargeIconLayout::from_tile_heights(1, 0.0, vec![100.0; 10]);
        let scroll_top = layout.row_bounds(3).unwrap().top + 17.0;
        assert_eq!(
            layout.scroll_top_revealing_index(scroll_top, 400.0, 4),
            scroll_top
        );
        assert_eq!(
            layout.scroll_top_revealing_index(scroll_top, 400.0, 2),
            layout.row_bounds(2).unwrap().top
        );
        assert_eq!(
            layout.scroll_top_revealing_index(scroll_top, 400.0, 8),
            layout.row_bounds(8).unwrap().top + 100.0 - 400.0
        );
    }
}
