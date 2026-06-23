use std::path::{Component, Path, PathBuf};

use gpui::{Font, TextRun, Window, px, rgb};

use crate::explorer::constants::{
    DIRECTORY_BAR_COPY_BUTTON_GAP, DIRECTORY_BAR_COPY_BUTTON_SIZE, DIRECTORY_BAR_ELLIPSIS,
    DIRECTORY_BAR_HORIZONTAL_PADDING, DIRECTORY_BAR_SEGMENT_HORIZONTAL_PADDING,
    DIRECTORY_BAR_SEPARATOR, DIRECTORY_BAR_TEXT_SIZE, NAV_BUTTON_SIZE, NAVBAR_HORIZONTAL_PADDING,
    NAVBAR_ITEM_GAP, SEARCH_BAR_RESERVED_WIDTH,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct VisibleBreadcrumb {
    pub(super) show_ellipsis: bool,
    pub(super) segments: Vec<BreadcrumbSegment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct BreadcrumbSegment {
    pub(super) label: String,
    pub(super) target: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BreadcrumbVisibility {
    pub(super) start_index: usize,
    pub(super) show_ellipsis: bool,
}

pub(super) fn path_breadcrumb_segments(path: &Path) -> Vec<BreadcrumbSegment> {
    let mut segments = Vec::new();
    let mut saw_prefix = false;
    let mut target = PathBuf::new();
    let components = path.components().collect::<Vec<_>>();

    for (index, component) in components.iter().copied().enumerate() {
        match component {
            Component::Prefix(prefix) => {
                saw_prefix = true;
                target.push(prefix.as_os_str());
                let label = prefix_breadcrumb_label(prefix);
                if !label.is_empty() {
                    let mut segment_target = target.clone();
                    if matches!(components.get(index + 1), Some(Component::RootDir)) {
                        segment_target.push(Component::RootDir.as_os_str());
                    }
                    segments.push(BreadcrumbSegment {
                        label,
                        target: segment_target,
                    });
                }
            }
            Component::RootDir => {
                target.push(component.as_os_str());
                if !saw_prefix {
                    segments.push(BreadcrumbSegment {
                        label: root_breadcrumb_label().to_owned(),
                        target: target.clone(),
                    });
                }
            }
            Component::CurDir => {
                target.push(component.as_os_str());
                segments.push(BreadcrumbSegment {
                    label: ".".to_owned(),
                    target: target.clone(),
                });
            }
            Component::ParentDir => {
                target.push(component.as_os_str());
                segments.push(BreadcrumbSegment {
                    label: "..".to_owned(),
                    target: target.clone(),
                });
            }
            Component::Normal(component) => {
                target.push(component);
                segments.push(BreadcrumbSegment {
                    label: component.to_string_lossy().into_owned(),
                    target: target.clone(),
                });
            }
        }
    }

    if segments.is_empty() {
        let fallback = path.display().to_string();
        let target = if path.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            path.to_path_buf()
        };
        segments.push(BreadcrumbSegment {
            label: if fallback.is_empty() {
                ".".to_owned()
            } else {
                fallback
            },
            target,
        });
    }

    segments
}

fn prefix_breadcrumb_label(prefix: std::path::PrefixComponent<'_>) -> String {
    rclone_prefix_breadcrumb_label(prefix)
        .unwrap_or_else(|| prefix.as_os_str().to_string_lossy().into_owned())
}

#[cfg(all(feature = "rclone", target_os = "windows"))]
fn rclone_prefix_breadcrumb_label(prefix: std::path::PrefixComponent<'_>) -> Option<String> {
    use std::path::Prefix;

    let (server, share) = match prefix.kind() {
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => (server, share),
        _ => return None,
    };
    server
        .to_string_lossy()
        .eq_ignore_ascii_case("rclone")
        .then(|| share.to_string_lossy().into_owned())
}

#[cfg(not(all(feature = "rclone", target_os = "windows")))]
fn rclone_prefix_breadcrumb_label(_: std::path::PrefixComponent<'_>) -> Option<String> {
    None
}

fn root_breadcrumb_label() -> &'static str {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        "Filesystem"
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "/"
    }
}

#[cfg(test)]
pub(super) fn breadcrumb_labels(segments: &[BreadcrumbSegment]) -> Vec<String> {
    segments
        .iter()
        .map(|segment| segment.label.clone())
        .collect()
}

pub(super) fn directory_bar_available_width(window_width: f32) -> f32 {
    let navbar_content_width =
        window_width - (NAVBAR_HORIZONTAL_PADDING * 2.0) - (NAV_BUTTON_SIZE * 4.0);
    let directory_bar_width =
        navbar_content_width - (NAVBAR_ITEM_GAP * 4.0) - SEARCH_BAR_RESERVED_WIDTH;
    let copy_button_width = DIRECTORY_BAR_COPY_BUTTON_SIZE + DIRECTORY_BAR_COPY_BUTTON_GAP;
    (directory_bar_width - (DIRECTORY_BAR_HORIZONTAL_PADDING * 2.0) - copy_button_width).max(0.0)
}

pub(super) fn visible_breadcrumb_for_path(
    path: &Path,
    available_width: f32,
    font: &Font,
    window: &Window,
) -> VisibleBreadcrumb {
    let segments = path_breadcrumb_segments(path);
    let segment_widths = segments
        .iter()
        .map(|segment| {
            measure_directory_bar_text(&segment.label, font, window)
                + DIRECTORY_BAR_SEGMENT_HORIZONTAL_PADDING * 2.0
        })
        .collect::<Vec<_>>();
    let separator_width = measure_directory_bar_text(DIRECTORY_BAR_SEPARATOR, font, window);
    let ellipsis_width = measure_directory_bar_text(DIRECTORY_BAR_ELLIPSIS, font, window);
    let visibility = choose_visible_breadcrumb(
        &segment_widths,
        separator_width,
        ellipsis_width,
        available_width,
    );

    VisibleBreadcrumb {
        show_ellipsis: visibility.show_ellipsis,
        segments: segments[visibility.start_index..].to_vec(),
    }
}

pub(super) fn measure_directory_bar_text(text: &str, font: &Font, window: &Window) -> f32 {
    if text.is_empty() {
        return 0.0;
    }

    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: rgb(0x1f1f1f).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };

    f32::from(
        window
            .text_system()
            .layout_line(text, px(DIRECTORY_BAR_TEXT_SIZE), &[run], None)
            .width,
    )
}

pub(super) fn choose_visible_breadcrumb(
    segment_widths: &[f32],
    separator_width: f32,
    ellipsis_width: f32,
    available_width: f32,
) -> BreadcrumbVisibility {
    if segment_widths.is_empty() {
        return BreadcrumbVisibility {
            start_index: 0,
            show_ellipsis: false,
        };
    }

    if breadcrumb_width(segment_widths, separator_width) <= available_width {
        return BreadcrumbVisibility {
            start_index: 0,
            show_ellipsis: false,
        };
    }

    for start_index in 1..segment_widths.len() {
        let width = ellipsis_width
            + separator_width
            + breadcrumb_width(&segment_widths[start_index..], separator_width);
        if width <= available_width {
            return BreadcrumbVisibility {
                start_index,
                show_ellipsis: true,
            };
        }
    }

    BreadcrumbVisibility {
        start_index: segment_widths.len() - 1,
        show_ellipsis: segment_widths.len() > 1,
    }
}

pub(super) fn breadcrumb_width(segment_widths: &[f32], separator_width: f32) -> f32 {
    if segment_widths.is_empty() {
        return 0.0;
    }

    segment_widths.iter().sum::<f32>() + separator_width * (segment_widths.len() - 1) as f32
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::path::{Path, PathBuf};

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_paths_render_drive_as_first_breadcrumb_segment() {
        let segments = path_breadcrumb_segments(Path::new(r"C:\Users\Ada\Documents"));

        assert_eq!(
            breadcrumb_labels(&segments),
            vec!["C:", "Users", "Ada", "Documents"]
        );
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.target.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from("C:\\"),
                PathBuf::from(r"C:\Users"),
                PathBuf::from(r"C:\Users\Ada"),
                PathBuf::from(r"C:\Users\Ada\Documents"),
            ]
        );
    }

    #[cfg(all(target_os = "windows", feature = "rclone"))]
    #[test]
    fn windows_rclone_unc_paths_render_remote_as_first_breadcrumb_segment() {
        let segments = path_breadcrumb_segments(Path::new(r"\\rclone\gdrive\Folder\File.txt"));

        assert_eq!(
            breadcrumb_labels(&segments),
            vec!["gdrive", "Folder", "File.txt"]
        );
        assert_eq!(segments[0].target, PathBuf::from(r"\\rclone\gdrive\"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_non_rclone_unc_paths_keep_full_prefix_label() {
        let segments = path_breadcrumb_segments(Path::new(r"\\server\share\Folder"));

        assert_eq!(
            breadcrumb_labels(&segments),
            vec![r"\\server\share", "Folder"]
        );
        assert_eq!(segments[0].target, PathBuf::from(r"\\server\share\"));
    }

    #[test]
    fn absolute_paths_render_root_as_breadcrumb_segment() {
        let segments = path_breadcrumb_segments(Path::new("/usr/local/bin"));

        assert_eq!(
            breadcrumb_labels(&segments),
            vec![root_breadcrumb_label(), "usr", "local", "bin"]
        );
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.target.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from("/"),
                PathBuf::from("/usr"),
                PathBuf::from("/usr/local"),
                PathBuf::from("/usr/local/bin"),
            ]
        );
    }

    #[test]
    fn relative_paths_keep_relative_breadcrumb_components() {
        let segments = path_breadcrumb_segments(Path::new("../project/src"));

        assert_eq!(breadcrumb_labels(&segments), vec!["..", "project", "src"]);
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.target.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from(".."),
                PathBuf::from("../project"),
                PathBuf::from("../project/src"),
            ]
        );

        let current_dir_segments = path_breadcrumb_segments(Path::new("."));
        assert_eq!(breadcrumb_labels(&current_dir_segments), vec!["."]);
        assert_eq!(current_dir_segments[0].target, PathBuf::from("."));
    }

    #[test]
    fn empty_paths_fall_back_to_current_directory_breadcrumb() {
        let segments = path_breadcrumb_segments(Path::new(""));

        assert_eq!(breadcrumb_labels(&segments), vec!["."]);
        assert_eq!(segments[0].target, PathBuf::from("."));
    }

    #[test]
    fn breadcrumb_visibility_keeps_full_path_when_it_fits() {
        assert_eq!(
            choose_visible_breadcrumb(&[10.0, 10.0, 10.0], 2.0, 3.0, 34.0),
            BreadcrumbVisibility {
                start_index: 0,
                show_ellipsis: false
            }
        );
    }

    #[test]
    fn directory_bar_available_width_reserves_copy_button_slot() {
        let window_width = 800.0;
        let width_without_copy_button = window_width
            - (NAVBAR_HORIZONTAL_PADDING * 2.0)
            - (NAV_BUTTON_SIZE * 4.0)
            - (NAVBAR_ITEM_GAP * 4.0)
            - SEARCH_BAR_RESERVED_WIDTH
            - (DIRECTORY_BAR_HORIZONTAL_PADDING * 2.0);

        assert_eq!(
            directory_bar_available_width(window_width),
            width_without_copy_button
                - DIRECTORY_BAR_COPY_BUTTON_SIZE
                - DIRECTORY_BAR_COPY_BUTTON_GAP
        );
    }

    #[test]
    fn breadcrumb_visibility_removes_leading_items_until_tail_fits() {
        assert_eq!(
            choose_visible_breadcrumb(&[20.0, 20.0, 20.0, 20.0], 2.0, 3.0, 47.0),
            BreadcrumbVisibility {
                start_index: 2,
                show_ellipsis: true
            }
        );
    }

    #[test]
    fn breadcrumb_visibility_preserves_final_segment_when_nothing_fits() {
        assert_eq!(
            choose_visible_breadcrumb(&[50.0, 50.0, 50.0], 5.0, 10.0, 1.0),
            BreadcrumbVisibility {
                start_index: 2,
                show_ellipsis: true
            }
        );
    }
}
