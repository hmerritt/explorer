use std::{fs, path::Path, time::SystemTime};

use gpui::{Context, Pixels, Point, Window};

use crate::explorer::{formatting::format_modified, view::ExplorerView};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ContextMenuState {
    pub(super) origin: Point<Pixels>,
    pub(super) items: Vec<ContextMenuItem>,
    pub(super) hovered_path: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ContextMenuItem {
    Action {
        id: &'static str,
        icon: Option<ContextMenuIcon>,
        label: &'static str,
        command: ContextMenuCommand,
        enabled: bool,
    },
    Submenu {
        id: &'static str,
        icon: Option<ContextMenuIcon>,
        label: &'static str,
        children: Vec<ContextMenuItem>,
    },
    Separator,
    Detail {
        label: &'static str,
        value: String,
        icon_slot: ContextMenuIconSlot,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContextMenuIconSlot {
    Reserve,
    Collapse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContextMenuIcon {
    Paste,
    New,
    File,
    Folder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContextMenuCommand {
    Paste,
    NewFile,
    NewFolder,
}

impl ContextMenuState {
    pub(super) fn new(origin: Point<Pixels>, items: Vec<ContextMenuItem>) -> Self {
        Self {
            origin,
            items,
            hovered_path: Vec::new(),
        }
    }
}

impl ExplorerView {
    pub(super) fn open_folder_context_menu(
        &mut self,
        origin: Point<Pixels>,
        can_paste: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.commit_active_rename_before_interaction(window, cx) {
            return false;
        }

        self.finish_search_edit();
        self.cancel_address_bar_edit();
        self.cancel_pending_click_rename();
        self.open_utility_menu = None;
        self.context_menu = Some(ContextMenuState::new(
            origin,
            folder_context_menu_items(&self.path, can_paste),
        ));
        true
    }

    pub(super) fn close_context_menu(&mut self) -> bool {
        self.context_menu.take().is_some()
    }

    pub(super) fn set_context_menu_hovered_path(&mut self, path: Vec<usize>) {
        if let Some(menu) = self.context_menu.as_mut() {
            menu.hovered_path = path;
        }
    }

    pub(super) fn execute_context_menu_command(
        &mut self,
        command: ContextMenuCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_context_menu();
        self.open_utility_menu = None;

        if !self.commit_active_rename_before_interaction(window, cx) {
            return;
        }

        match command {
            ContextMenuCommand::Paste => self.paste_clipboard_files(cx),
            ContextMenuCommand::NewFile => self.create_new_file(window, cx),
            ContextMenuCommand::NewFolder => self.create_new_folder(window, cx),
        }
    }
}

pub(super) fn folder_context_menu_items(path: &Path, can_paste: bool) -> Vec<ContextMenuItem> {
    let (created, modified) = fs::metadata(path)
        .map(|metadata| (metadata.created().ok(), metadata.modified().ok()))
        .unwrap_or((None, None));

    folder_context_menu_items_from_times(can_paste, created, modified)
}

pub(super) fn folder_context_menu_items_from_times(
    can_paste: bool,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
) -> Vec<ContextMenuItem> {
    vec![
        ContextMenuItem::Action {
            id: "context-menu-paste",
            icon: Some(ContextMenuIcon::Paste),
            label: "Paste",
            command: ContextMenuCommand::Paste,
            enabled: can_paste,
        },
        ContextMenuItem::Submenu {
            id: "context-menu-new",
            icon: Some(ContextMenuIcon::New),
            label: "New",
            children: vec![
                ContextMenuItem::Action {
                    id: "context-menu-new-file",
                    icon: Some(ContextMenuIcon::File),
                    label: "File",
                    command: ContextMenuCommand::NewFile,
                    enabled: true,
                },
                ContextMenuItem::Action {
                    id: "context-menu-new-folder",
                    icon: Some(ContextMenuIcon::Folder),
                    label: "Folder",
                    command: ContextMenuCommand::NewFolder,
                    enabled: true,
                },
            ],
        },
        ContextMenuItem::Separator,
        ContextMenuItem::Detail {
            label: "Created",
            value: format_modified(created),
            icon_slot: ContextMenuIconSlot::Collapse,
        },
        ContextMenuItem::Detail {
            label: "Modified",
            value: format_modified(modified),
            icon_slot: ContextMenuIconSlot::Collapse,
        },
    ]
}

pub(super) fn context_menu_path_is_active(hovered_path: &[usize], path: &[usize]) -> bool {
    hovered_path.len() >= path.len() && hovered_path[..path.len()] == *path
}

pub(super) fn context_menu_height(
    items: &[ContextMenuItem],
    row_height: f32,
    separator_height: f32,
) -> f32 {
    let content_height: f32 = items
        .iter()
        .map(|item| match item {
            ContextMenuItem::Separator => separator_height,
            ContextMenuItem::Action { .. }
            | ContextMenuItem::Submenu { .. }
            | ContextMenuItem::Detail { .. } => row_height,
        })
        .sum();

    content_height + 8.0
}

pub(super) fn context_menu_item_top(
    items: &[ContextMenuItem],
    index: usize,
    row_height: f32,
    separator_height: f32,
) -> f32 {
    4.0 + items[..index]
        .iter()
        .map(|item| match item {
            ContextMenuItem::Separator => separator_height,
            ContextMenuItem::Action { .. }
            | ContextMenuItem::Submenu { .. }
            | ContextMenuItem::Detail { .. } => row_height,
        })
        .sum::<f32>()
}

pub(super) fn clamped_context_menu_origin(
    origin: (f32, f32),
    menu_size: (f32, f32),
    window_size: (f32, f32),
) -> (f32, f32) {
    let max_x = (window_size.0 - menu_size.0).max(0.0);
    let max_y = (window_size.1 - menu_size.1).max(0.0);

    (origin.0.clamp(0.0, max_x), origin.1.clamp(0.0, max_y))
}

pub(super) fn context_menu_pointer_tip_origin(
    pointer: (f32, f32),
    menu_size: (f32, f32),
    window_size: (f32, f32),
) -> (f32, f32) {
    clamped_context_menu_origin(pointer, menu_size, window_size)
}

pub(super) fn context_submenu_left(
    parent_left: f32,
    menu_width: f32,
    overlap: f32,
    window_width: f32,
) -> f32 {
    let right_left = parent_left + menu_width - overlap;
    if right_left + menu_width <= window_width {
        right_left
    } else {
        parent_left - menu_width + overlap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    #[test]
    fn hovered_path_matches_active_branch() {
        let hovered = vec![1, 0, 2];

        assert!(context_menu_path_is_active(&hovered, &[1]));
        assert!(context_menu_path_is_active(&hovered, &[1, 0]));
        assert!(context_menu_path_is_active(&hovered, &[1, 0, 2]));
        assert!(!context_menu_path_is_active(&hovered, &[0]));
        assert!(!context_menu_path_is_active(&hovered, &[1, 1]));
        assert!(!context_menu_path_is_active(&hovered, &[1, 0, 2, 0]));
    }

    #[test]
    fn root_menu_position_clamps_inside_window() {
        assert_eq!(
            context_menu_pointer_tip_origin((120.0, 90.0), (220.0, 180.0), (800.0, 600.0)),
            (120.0, 90.0)
        );
        assert_eq!(
            context_menu_pointer_tip_origin((780.0, 580.0), (220.0, 180.0), (800.0, 600.0)),
            (580.0, 420.0)
        );
        assert_eq!(
            context_menu_pointer_tip_origin((-20.0, -10.0), (220.0, 180.0), (800.0, 600.0)),
            (0.0, 0.0)
        );
    }

    #[test]
    fn submenu_position_overlaps_parent_border() {
        assert_eq!(context_submenu_left(100.0, 250.0, 1.0, 800.0), 349.0);
        assert_eq!(context_submenu_left(500.0, 250.0, 1.0, 800.0), 251.0);
    }

    #[test]
    fn new_state_replaces_origin_for_reopen() {
        let first = ContextMenuState::new(
            Point {
                x: gpui::px(10.0),
                y: gpui::px(20.0),
            },
            Vec::new(),
        );
        let second = ContextMenuState::new(
            Point {
                x: gpui::px(70.0),
                y: gpui::px(80.0),
            },
            Vec::new(),
        );

        assert_ne!(first.origin, second.origin);
        assert_eq!(second.hovered_path, Vec::<usize>::new());
    }

    #[test]
    fn folder_menu_contains_expected_items_and_icons() {
        let items = folder_context_menu_items_from_times(false, None, None);

        assert_eq!(items.len(), 5);
        assert_eq!(
            items[0],
            ContextMenuItem::Action {
                id: "context-menu-paste",
                icon: Some(ContextMenuIcon::Paste),
                label: "Paste",
                command: ContextMenuCommand::Paste,
                enabled: false,
            }
        );

        let ContextMenuItem::Submenu {
            icon,
            label,
            children,
            ..
        } = &items[1]
        else {
            panic!("expected New submenu");
        };
        assert_eq!(*icon, Some(ContextMenuIcon::New));
        assert_eq!(*label, "New");
        assert_eq!(children.len(), 2);
        assert!(matches!(items[2], ContextMenuItem::Separator));
    }

    #[test]
    fn detail_rows_have_no_icons_and_blank_unsupported_dates() {
        let items = folder_context_menu_items_from_times(true, None, None);

        assert_eq!(
            items[3],
            ContextMenuItem::Detail {
                label: "Created",
                value: String::new(),
                icon_slot: ContextMenuIconSlot::Collapse,
            }
        );
        assert_eq!(
            items[4],
            ContextMenuItem::Detail {
                label: "Modified",
                value: String::new(),
                icon_slot: ContextMenuIconSlot::Collapse,
            }
        );
    }

    #[test]
    fn detail_rows_format_supported_dates() {
        let created = Local.with_ymd_and_hms(2026, 6, 1, 9, 15, 0).unwrap();
        let modified = Local.with_ymd_and_hms(2026, 6, 2, 10, 30, 0).unwrap();
        let items =
            folder_context_menu_items_from_times(true, Some(created.into()), Some(modified.into()));

        assert_eq!(
            items[3],
            ContextMenuItem::Detail {
                label: "Created",
                value: "01/06/2026 09:15".to_owned(),
                icon_slot: ContextMenuIconSlot::Collapse,
            }
        );
        assert_eq!(
            items[4],
            ContextMenuItem::Detail {
                label: "Modified",
                value: "02/06/2026 10:30".to_owned(),
                icon_slot: ContextMenuIconSlot::Collapse,
            }
        );
    }
}
