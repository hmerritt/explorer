pub(crate) const COLUMN_NAME_MIN_WIDTH: f32 = 250.0;
pub(crate) const COLUMN_DATE_WIDTH: f32 = 150.0;
pub(crate) const COLUMN_TYPE_WIDTH: f32 = 150.0;
pub(crate) const COLUMN_SIZE_WIDTH: f32 = 120.0;
pub(super) const NAVBAR_HEIGHT: f32 = 52.0;
pub(super) const NAV_ICON_TEXT_SIZE: f32 = 12.0;
pub(super) const NAV_ICON_ENABLED_COLOR: u32 = 0x1f1f1f;
pub(super) const NAV_ICON_DISABLED_COLOR: u32 = 0x9a9a9a;
pub(super) const NAV_BUTTON_HOVER_BG: u32 = 0xefefef;
pub(super) const NAV_BUTTON_ACTIVE_OPACITY: f32 = 0.7;
pub(super) const NAVBAR_HORIZONTAL_PADDING: f32 = 10.0;
pub(super) const NAVBAR_ITEM_GAP: f32 = 12.0;
pub(super) const NAV_BUTTON_SIZE: f32 = 34.0;
pub(super) const UTILITY_BAR_HEIGHT: f32 = 44.0;
pub(super) const UTILITY_BAR_HORIZONTAL_PADDING: f32 = 10.0;
pub(super) const UTILITY_BAR_ITEM_GAP: f32 = 14.0;
pub(super) const UTILITY_BUTTON_HEIGHT: f32 = 32.0;
pub(super) const UTILITY_ICON_BUTTON_SIZE: f32 = 32.0;
pub(super) const UTILITY_MENU_WIDTH: f32 = 190.0;
pub(super) const UTILITY_MENU_ROW_HEIGHT: f32 = 32.0;
pub(super) const DIRECTORY_BAR_HEIGHT: f32 = 34.0;
pub(super) const DIRECTORY_BAR_RADIUS: f32 = 6.0;
pub(super) const DIRECTORY_BAR_HORIZONTAL_PADDING: f32 = 16.0;
pub(super) const DIRECTORY_BAR_SEGMENT_HORIZONTAL_PADDING: f32 = 4.0;
pub(super) const DIRECTORY_BAR_TEXT_SIZE: f32 = 15.0;
pub(super) const DIRECTORY_BAR_SEPARATOR: &str = " / ";
pub(super) const DIRECTORY_BAR_ELLIPSIS: &str = "...";
pub(super) const SEARCH_BAR_MAX_WIDTH: f32 = 260.0;
pub(super) const SEARCH_BAR_MIN_WIDTH: f32 = 150.0;
pub(super) const SEARCH_BAR_RESERVED_WIDTH: f32 = SEARCH_BAR_MAX_WIDTH + NAVBAR_ITEM_GAP;
pub(super) const SEARCH_NO_MATCHES_MESSAGE: &str = "No items match your search.";
pub(super) const SEARCH_WORKING_MESSAGE: &str = "Searching...";
pub(super) const HEADER_HEIGHT: f32 = 32.0;
pub(super) const ROW_HEIGHT: f32 = 28.0;
pub(super) const RECURSIVE_SEARCH_ROW_HEIGHT: f32 = 42.0;
pub(super) const FILE_ICON_SIZE: f32 = 16.0;
pub(super) const FILE_ICON_SLOT_WIDTH: f32 = 16.0;
pub(super) const FILE_ICON_SLOT_HEIGHT: f32 = 16.0;
pub(super) const LARGE_ICON_TILE_WIDTH: f32 = 105.0;
pub(super) const LARGE_ICON_TILE_HEIGHT: f32 = 120.0;
pub(super) const LARGE_ICON_SIZE: f32 = 85.0;
pub(super) const LARGE_ICON_TEXT_TOP_GAP: f32 = 4.0;
pub(super) const LARGE_ICON_TEXT_SIZE: f32 = 12.0;
pub(super) const LARGE_ICON_TEXT_LINE_HEIGHT: f32 = 16.0;
pub(super) const LARGE_ICON_TEXT_ROWS: usize = 3;
pub(super) const LARGE_ICON_TEXT_BOTTOM_PADDING: f32 = 6.0;
pub(super) const LARGE_ICON_ROW_GAP: f32 = 2.0;
pub(super) const EMPTY_FOLDER_TEXT_SIZE: f32 = 12.0;
pub(super) const EMPTY_FOLDER_TOP_MARGIN: f32 = 20.0;
pub(super) const EMPTY_FOLDER_MESSAGE: &str = "This folder is empty.";
pub(super) const OPEN_ERROR_VERTICAL_PADDING: f32 = 8.0;
pub(super) const OPEN_ERROR_HORIZONTAL_PADDING: f32 = 16.0;
pub(super) const STATUS_BAR_HEIGHT: f32 = 24.0;
pub(super) const STATUS_BAR_HORIZONTAL_PADDING: f32 = 16.0;
pub(super) const STATUS_BAR_TEXT_SIZE: f32 = 12.0;
pub(super) const STATUS_BAR_TEXT_COLOR: u32 = 0x595959;
pub(super) const STATUS_BAR_SEPARATOR_COLOR: u32 = 0xe5e5e5;
pub(crate) const EXPLORER_COPY_GREEN: u32 = 0x36a646; // 0x0f7b0f | 0x36a646 | 0x06b025
pub(super) const SIDEBAR_ROW_HEIGHT: f32 = 30.0;
pub(super) const SIDEBAR_ICON_SIZE: f32 = 16.0;
pub(super) const SIDEBAR_ICON_TEXT_GAP: f32 = 10.0;
pub(super) const SIDEBAR_HORIZONTAL_PADDING: f32 = 12.0;
pub(super) const SIDEBAR_TEXT_SIZE: f32 = 12.0;
pub(super) const SCROLLBAR_GUTTER_WIDTH: f32 = 18.0;
pub(super) const SCROLLBAR_THUMB_WIDTH: f32 = 4.0;
pub(super) const SCROLLBAR_THUMB_HOVER_WIDTH: f32 = 6.0;
pub(super) const SCROLLBAR_ARROW_HEIGHT: f32 = 16.0;
pub(super) const SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 32.0;
pub(super) const SCROLLBAR_TRACK_BG: u32 = 0xf8f8f8;
pub(super) const SCROLLBAR_THUMB_BG: u32 = 0x8a8a8a;
pub(super) const SCROLLBAR_THUMB_HOVER_BG: u32 = 0x707070;
pub(super) const SCROLLBAR_THUMB_ACTIVE_BG: u32 = 0x5f5f5f;
pub(super) const SCROLLBAR_ARROW_COLOR: u32 = 0x606060;
pub(super) const SCROLLBAR_ARROW_HOVER_BG: u32 = 0xe8e8e8;
pub(super) const HORIZONTAL_SCROLLBAR_LINE_DELTA: f32 = 40.0;
pub(super) const KB_BYTES: u64 = 1024;
pub(super) const MB_BYTES: u64 = KB_BYTES * 1024;
pub(super) const GB_BYTES: u64 = MB_BYTES * 1024;
pub(super) const TB_BYTES: u64 = GB_BYTES * 1024;

#[cfg(test)]
pub(super) fn effective_name_column_width(viewport_width: f32) -> f32 {
    let fixed_columns_width =
        COLUMN_DATE_WIDTH + COLUMN_TYPE_WIDTH + COLUMN_SIZE_WIDTH + SCROLLBAR_GUTTER_WIDTH;

    (viewport_width - fixed_columns_width).max(COLUMN_NAME_MIN_WIDTH)
}

#[cfg(test)]
pub(super) fn minimum_file_columns_width() -> f32 {
    COLUMN_NAME_MIN_WIDTH + COLUMN_DATE_WIDTH + COLUMN_TYPE_WIDTH + COLUMN_SIZE_WIDTH
}
