//! Adapters intentionally call the same operations as production handlers.
use super::{
    ExplorerTabs, actions::*, navigation::HistoryMode, sorting::benchmark_entries_are_sorted,
};
use crate::settings::FileSortColumn;
use gpui::{App, Entity, Window, point, px};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct UiSnapshot {
    pub path: PathBuf,
    pub generation: u64,
    pub loading: bool,
    pub entries: usize,
    pub selected: Vec<usize>,
    pub tab: u64,
    pub tabs: usize,
    pub query: String,
    pub searching: bool,
    pub recursive_results: bool,
    pub sort: String,
    pub sorted: bool,
    pub scroll_top: f32,
    pub error: Option<String>,
}

pub(crate) fn snapshot(tabs: &Entity<ExplorerTabs>, cx: &App, validate_sort: bool) -> UiSnapshot {
    let (view, tab, count) = tabs
        .read(cx)
        .benchmark_active()
        .expect("benchmark has an active tab");
    let view = view.read(cx);
    let scroll_top = if view.view_mode == crate::settings::FileViewMode::LargeIcons {
        view.large_icon_layout.as_ref().map_or(0., |layout| {
            layout.scroll_top(view.large_icon_list_state.logical_scroll_top())
        })
    } else {
        -f32::from(view.scroll_handle.0.borrow().base_handle.offset().y)
    };
    UiSnapshot {
        path: view.path.clone(),
        generation: view.directory_load_generation,
        loading: view.loading_path.is_some(),
        entries: view.entries.len(),
        selected: view.selection.selected_indices.iter().copied().collect(),
        tab,
        tabs: count,
        query: view.search_query().into(),
        searching: view.recursive_search_is_working(),
        recursive_results: view.recursive_search_results_active(),
        sort: format!("{:?}/{:?}", view.file_sort.column, view.file_sort.direction),
        sorted: !validate_sort || benchmark_entries_are_sorted(&view.entries, view.file_sort),
        scroll_top,
        error: view.read_error.clone(),
    }
}

pub(crate) fn apply(
    tabs: &Entity<ExplorerTabs>,
    kind: &str,
    path: Option<&Path>,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    if ["new_tab", "switch_tab", "close_tab"].contains(&kind) {
        tabs.update(cx, |tabs, cx| tabs.benchmark_tab_action(kind, window, cx));
        return Ok(());
    }
    let (view, _, _) = tabs.read(cx).benchmark_active().ok_or("no active view")?;
    if kind == "hover" || kind == "clear_hover" {
        let entry = view
            .read(cx)
            .entries
            .first()
            .ok_or("preview fixture has no entry")?
            .path
            .clone();
        let position =
            crate::performance::ui::entry_position(&entry).ok_or("entry has not been painted")?;
        window.dispatch_benchmark_input(
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: gpui::Modifiers {
                    alt: kind == "hover",
                    ..Default::default()
                },
            }),
            cx,
        );
        return Ok(());
    }
    view.update(cx, |view, cx| -> Result<(), String> {
        match kind {
            "idle" => {}
            "navigate" => view.navigate_to_directory_with_watcher(
                path.ok_or("missing navigation path")?.to_path_buf(),
                HistoryMode::Record,
                cx,
            ),
            "open" => {
                view.select_single_path(path.ok_or("missing child path")?);
                view.handle_enter_selected(&EnterSelected, window, cx);
            }
            "back" => view.handle_go_back(&GoBack, window, cx),
            "forward" => view.handle_go_forward(&GoForward, window, cx),
            "up" => view.handle_go_up(&GoUp, window, cx),
            "refresh" => view.handle_refresh(&Refresh, window, cx),
            "filter" => view.benchmark_search("needle".into(), false, cx),
            "recursive_search" => view.benchmark_search("needle".into(), true, cx),
            "single_selection" => view.handle_move_down(&MoveDown, window, cx),
            "range_selection" => {
                view.extend_selection_to_index(9);
            }
            "select_all" => view.handle_select_all(&SelectAll, window, cx),
            "image_viewer" => {
                let entry = view.entries.first().ok_or("viewer fixture has no entry")?;
                crate::image_viewer::open_image_window(entry.path.clone(), cx);
            }
            name if name.starts_with("sort_") => {
                let column = match name.trim_start_matches("sort_").split('_').next().unwrap() {
                    "name" => FileSortColumn::Name,
                    "date" => FileSortColumn::DateModified,
                    "type" => FileSortColumn::Type,
                    "size" => FileSortColumn::Size,
                    _ => return Err("unknown sort column".into()),
                };
                view.sort_entries_from_header(column);
            }
            _ => return Err(format!("unknown UI operation {kind}")),
        }
        cx.notify();
        Ok(())
    })
}

pub(crate) fn media_ready(
    tabs: &Entity<ExplorerTabs>,
    kind: &str,
    cx: &App,
) -> Result<bool, String> {
    let (view, _, _) = tabs.read(cx).benchmark_active().ok_or("no active view")?;
    let view = view.read(cx);
    if view.thumbnail_source_policy == super::image_thumbnails::ThumbnailSourcePolicy::CacheOnly {
        return Err("media fixture is treated as cache-only; generated media requires a local source directory".into());
    }
    if kind.ends_with("thumbnails") {
        let visible = crate::performance::ui::visible_entries();
        let mut count = 0;
        for entry in &view.entries {
            if visible.contains(&entry.path) {
                if !view.benchmark_cached_thumbnail(entry, false, cx)? {
                    return Ok(false);
                }
                count += 1;
            }
        }
        return Ok(count > 0);
    }
    let Some(preview) = &view.image_hover_preview else {
        return Ok(false);
    };
    if !view.image_hover_preview_alt {
        return Ok(false);
    }
    match kind {
        "hover_video" => view.benchmark_video_preview_ready(&preview.entry.path),
        "hover_text" => view.benchmark_text_preview_ready(&preview.entry.path),
        _ => view.benchmark_cached_thumbnail(&preview.entry, true, cx),
    }
}

pub(crate) fn scroll(
    tabs: &Entity<ExplorerTabs>,
    pixels: f32,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    let (view, _, _) = tabs.read(cx).benchmark_active().ok_or("no active view")?;
    let view = view.read(cx);
    let position = point(
        view.view_origin.x + px(view.sidebar_width + 100.),
        view.view_origin.y + px(180.),
    );
    window.dispatch_benchmark_input(
        gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(pixels))),
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
        }),
        cx,
    );
    Ok(())
}
