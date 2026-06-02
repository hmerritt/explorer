pub(super) const COLUMN_NAME_MIN_WIDTH: f32 = 250.0;
pub(super) const COLUMN_DATE_WIDTH: f32 = 180.0;
pub(super) const COLUMN_TYPE_WIDTH: f32 = 180.0;
pub(super) const COLUMN_SIZE_WIDTH: f32 = 120.0;
pub(super) const NAVBAR_HEIGHT: f32 = 52.0;
pub(super) const NAV_ICON_SIZE_PHYSICAL: f32 = 18.0;
pub(super) const NAV_ICON_ENABLED_COLOR: u32 = 0x1f1f1f;
pub(super) const NAV_ICON_DISABLED_COLOR: u32 = 0x9a9a9a;
pub(super) const NAV_BUTTON_HOVER_BG: u32 = 0xefefef;
pub(super) const NAV_BUTTON_ACTIVE_OPACITY: f32 = 0.7;
pub(super) const NAVBAR_HORIZONTAL_PADDING: f32 = 10.0;
pub(super) const NAVBAR_ITEM_GAP: f32 = 10.0;
pub(super) const NAV_BUTTON_SIZE: f32 = 34.0;
pub(super) const DIRECTORY_BAR_HEIGHT: f32 = 34.0;
pub(super) const DIRECTORY_BAR_RADIUS: f32 = 6.0;
pub(super) const DIRECTORY_BAR_HORIZONTAL_PADDING: f32 = 16.0;
pub(super) const DIRECTORY_BAR_SEGMENT_HORIZONTAL_PADDING: f32 = 4.0;
pub(super) const DIRECTORY_BAR_TEXT_SIZE: f32 = 15.0;
pub(super) const DIRECTORY_BAR_SEPARATOR: &str = " / ";
pub(super) const DIRECTORY_BAR_ELLIPSIS: &str = "...";
pub(super) const HEADER_HEIGHT: f32 = 32.0;
pub(super) const ROW_HEIGHT: f32 = 28.0;
pub(super) const FILE_ICON_SLOT_WIDTH_PHYSICAL: f32 = 22.0;
pub(super) const FILE_ICON_SLOT_HEIGHT_PHYSICAL: f32 = 20.0;
pub(super) const FILE_ICON_PAGE_WIDTH_PHYSICAL: f32 = 16.0;
pub(super) const FILE_ICON_PAGE_HEIGHT_PHYSICAL: f32 = 20.0;
pub(super) const FILE_ICON_PAGE_LEFT_PHYSICAL: f32 =
    (FILE_ICON_SLOT_WIDTH_PHYSICAL - FILE_ICON_PAGE_WIDTH_PHYSICAL) / 2.0;
pub(super) const FILE_ICON_FOLD_SIZE_PHYSICAL: f32 = 5.0;
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
pub(super) const SIDEBAR_WIDTH: f32 = 220.0;
pub(super) const SIDEBAR_ROW_HEIGHT: f32 = 30.0;
pub(super) const SIDEBAR_ICON_TEXT_GAP_PHYSICAL: f32 = 10.0;
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
pub(super) const KB_BYTES: u64 = 1024;
pub(super) const MB_BYTES: u64 = KB_BYTES * 1024;
pub(super) const GB_BYTES: u64 = MB_BYTES * 1024;
pub(super) const TB_BYTES: u64 = GB_BYTES * 1024;

pub(super) fn effective_name_column_width(viewport_width: f32) -> f32 {
    let fixed_columns_width =
        COLUMN_DATE_WIDTH + COLUMN_TYPE_WIDTH + COLUMN_SIZE_WIDTH + SCROLLBAR_GUTTER_WIDTH;

    (viewport_width - fixed_columns_width).max(COLUMN_NAME_MIN_WIDTH)
}
