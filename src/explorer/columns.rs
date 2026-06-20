use crate::{
    explorer::constants::{COLUMN_NAME_MIN_WIDTH, SCROLLBAR_GUTTER_WIDTH},
    settings::{FileColumnKind, FileColumnSettings, default_file_column_width},
};

pub(super) fn file_column_label(kind: FileColumnKind) -> &'static str {
    match kind {
        FileColumnKind::DateModified => "Date modified",
        FileColumnKind::Type => "Type",
        FileColumnKind::Size => "Size",
    }
}

pub(super) fn file_column_width(settings: &FileColumnSettings, kind: FileColumnKind) -> f32 {
    settings
        .widths
        .get(&kind)
        .copied()
        .unwrap_or_else(|| default_file_column_width(kind)) as f32
}

pub(super) fn file_column_width_total(settings: &FileColumnSettings) -> f32 {
    settings
        .order
        .iter()
        .map(|kind| file_column_width(settings, *kind))
        .sum()
}

pub(super) fn minimum_file_columns_width(settings: &FileColumnSettings) -> f32 {
    settings.name_width.unwrap_or(COLUMN_NAME_MIN_WIDTH as u32) as f32
        + file_column_width_total(settings)
}

pub(super) fn effective_name_column_width(
    viewport_width: f32,
    settings: &FileColumnSettings,
) -> f32 {
    if let Some(width) = settings.name_width {
        return width as f32;
    }

    let fixed_columns_width = file_column_width_total(settings) + SCROLLBAR_GUTTER_WIDTH;

    (viewport_width - fixed_columns_width).max(COLUMN_NAME_MIN_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::FileColumnKind;

    #[test]
    fn minimum_width_uses_configured_non_name_widths() {
        let mut settings = FileColumnSettings::default();
        settings.widths.insert(FileColumnKind::Type, 300);

        assert_eq!(
            minimum_file_columns_width(&settings),
            250.0 + 150.0 + 300.0 + 120.0
        );
    }

    #[test]
    fn name_width_uses_remaining_viewport_and_minimum() {
        let mut settings = FileColumnSettings::default();
        settings.widths.insert(FileColumnKind::Type, 300);

        assert_eq!(effective_name_column_width(900.0, &settings), 312.0);
        assert_eq!(effective_name_column_width(500.0, &settings), 250.0);
    }

    #[test]
    fn manual_name_width_overrides_auto_width_and_minimum() {
        let mut settings = FileColumnSettings::default();
        settings.name_width = Some(400);

        assert_eq!(effective_name_column_width(900.0, &settings), 400.0);
        assert_eq!(
            minimum_file_columns_width(&settings),
            400.0 + 150.0 + 150.0 + 120.0
        );
    }
}
