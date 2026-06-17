use gpui::{App, Font, LineFragment, px};

use crate::explorer::{
    constants::{
        LARGE_ICON_ROW_GAP, LARGE_ICON_SIZE, LARGE_ICON_TEXT_BOTTOM_PADDING,
        LARGE_ICON_TEXT_LINE_HEIGHT, LARGE_ICON_TEXT_ROWS, LARGE_ICON_TEXT_SIZE,
        LARGE_ICON_TEXT_TOP_GAP, LARGE_ICON_TILE_WIDTH,
    },
    entry::FileEntry,
    mouse_selection::large_icon_grid_columns,
};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LargeIconLayoutKey {
    columns: usize,
    row_count: usize,
    tile_heights: Vec<f32>,
}

#[derive(Clone, Debug)]
pub(super) struct LargeIconLayout {
    pub(super) columns: usize,
    pub(super) column_gap: f32,
    rows: Vec<LargeIconRowLayout>,
    tile_heights: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LargeIconRowLayout {
    pub(super) top: f32,
    pub(super) height: f32,
    pub(super) tile_height: f32,
}

impl LargeIconLayout {
    pub(super) fn new(
        entries: &[FileEntry],
        viewport_width: f32,
        show_file_name_extensions: bool,
        font: &Font,
        cx: &App,
    ) -> Self {
        let columns = large_icon_grid_columns(viewport_width);
        let column_gap = large_icon_column_gap(viewport_width, columns);
        let tile_heights = entries
            .iter()
            .map(|entry| {
                let display_name = entry.display_name_with_extensions(show_file_name_extensions);
                large_icon_tile_height_for_text(display_name, font, cx)
            })
            .collect::<Vec<_>>();

        Self::from_tile_heights(columns, column_gap, tile_heights)
    }

    pub(super) fn from_tile_heights(
        columns: usize,
        column_gap: f32,
        tile_heights: Vec<f32>,
    ) -> Self {
        let columns = columns.max(1);
        let rows = large_icon_rows_from_tile_heights(columns, &tile_heights);
        Self {
            columns,
            column_gap,
            rows,
            tile_heights,
        }
    }

    pub(super) fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub(super) fn row_for_index(&self, ix: usize) -> usize {
        ix / self.columns
    }

    pub(super) fn row_bounds(&self, row: usize) -> Option<LargeIconRowLayout> {
        self.rows.get(row).copied()
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
            .iter()
            .position(|row| content_y >= row.top && content_y < row.top + row.tile_height)?;
        let column = self.column_at_x(content_x)?;
        let ix = row * self.columns + column;
        (ix < entry_count).then_some(ix)
    }

    pub(super) fn key(&self) -> LargeIconLayoutKey {
        LargeIconLayoutKey {
            columns: self.columns,
            row_count: self.row_count(),
            tile_heights: self.tile_heights.clone(),
        }
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

pub(super) fn large_icon_tile_height_for_text(text: &str, font: &Font, cx: &App) -> f32 {
    large_icon_tile_height_for_rows(large_icon_text_row_count(text, font, cx))
}

fn large_icon_text_row_count(text: &str, font: &Font, cx: &App) -> usize {
    if text.is_empty() {
        return 1;
    }

    let mut line_wrapper = cx
        .text_system()
        .line_wrapper(font.clone(), px(LARGE_ICON_TEXT_SIZE));
    let fragments = [LineFragment::text(text)];
    (line_wrapper
        .wrap_line(&fragments, px(large_icon_filename_text_width()))
        .count()
        + 1)
    .clamp(1, LARGE_ICON_TEXT_ROWS)
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
}
