use std::path::Path;

use gpui::{Context, Window, actions};

use crate::explorer::{
    selection::{SelectionDirection, SelectionEdge},
    view::ExplorerView,
};

actions!(
    explorer,
    [
        MoveUp,
        MoveDown,
        ExtendUp,
        ExtendDown,
        MoveHome,
        MoveEnd,
        ExtendHome,
        ExtendEnd,
        GoBack,
        GoForward,
        GoUp,
        CancelDrag,
        OpenSelected,
        OpenSelectedInNewTab,
        OpenProperties,
        OpenSettings,
        EnterSelected,
        EnterSelectedInNewTab,
        Refresh,
        SelectAll,
        CopySelected,
        CutSelected,
        PasteClipboard,
        UndoFileOperation,
        TrashSelected,
        PermanentlyDeleteSelected,
        CreateNewFolder,
        CreateNewFile,
        RenameSelected,
        RenameCommit,
        RenameCancel,
        RenameBackspace,
        RenameBackspaceWord,
        RenameDelete,
        RenameLeft,
        RenameRight,
        RenameSelectLeft,
        RenameSelectRight,
        RenameWordLeft,
        RenameWordRight,
        RenameSelectWordLeft,
        RenameSelectWordRight,
        RenameHome,
        RenameEnd,
        RenameSelectHome,
        RenameSelectEnd,
        RenameSelectAll,
        RenameCopy,
        RenameCut,
        RenamePaste,
        RenameNoop,
        AddressEdit,
        AddressCommit,
        AddressCancel,
        AddressBackspace,
        AddressBackspaceWord,
        AddressDelete,
        AddressLeft,
        AddressRight,
        AddressSelectLeft,
        AddressSelectRight,
        AddressWordLeft,
        AddressWordRight,
        AddressSelectWordLeft,
        AddressSelectWordRight,
        AddressHome,
        AddressEnd,
        AddressSelectHome,
        AddressSelectEnd,
        AddressSelectAll,
        AddressCopy,
        AddressCut,
        AddressPaste,
        AddressSuggestionUp,
        AddressSuggestionDown,
        AddressAcceptSuggestion,
        SearchEdit,
        RecursiveSearchEdit,
        SearchCommit,
        SearchCancel,
        SearchBackspace,
        SearchBackspaceWord,
        SearchDelete,
        SearchLeft,
        SearchRight,
        SearchSelectLeft,
        SearchSelectRight,
        SearchWordLeft,
        SearchWordRight,
        SearchSelectWordLeft,
        SearchSelectWordRight,
        SearchHome,
        SearchEnd,
        SearchSelectHome,
        SearchSelectEnd,
        SearchSelectAll,
        SearchCopy,
        SearchCut,
        SearchPaste,
        NewWindow,
        NewTab,
        CloseTab,
        SelectNextTab,
        SelectPreviousTab
    ]
);

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = explorer, no_json)]
pub struct SelectTabByIndex {
    pub index: usize,
}

impl ExplorerView {
    pub(super) fn handle_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(SelectionDirection::Up);
        cx.notify();
    }

    pub(super) fn handle_move_down(
        &mut self,
        _: &MoveDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selection(SelectionDirection::Down);
        cx.notify();
    }

    pub(super) fn handle_extend_up(
        &mut self,
        _: &ExtendUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection(SelectionDirection::Up);
        cx.notify();
    }

    pub(super) fn handle_extend_down(
        &mut self,
        _: &ExtendDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection(SelectionDirection::Down);
        cx.notify();
    }

    pub(super) fn handle_move_home(
        &mut self,
        _: &MoveHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_edge(SelectionEdge::Home);
        cx.notify();
    }

    pub(super) fn handle_move_end(&mut self, _: &MoveEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_edge(SelectionEdge::End);
        cx.notify();
    }

    pub(super) fn handle_extend_home(
        &mut self,
        _: &ExtendHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection_to_edge(SelectionEdge::Home);
        cx.notify();
    }

    pub(super) fn handle_extend_end(
        &mut self,
        _: &ExtendEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection_to_edge(SelectionEdge::End);
        cx.notify();
    }

    pub(super) fn handle_go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate_back_with_watcher(cx);
        cx.notify();
    }

    pub(super) fn handle_go_forward(
        &mut self,
        _: &GoForward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_forward_with_watcher(cx);
        cx.notify();
    }

    pub(super) fn handle_go_up(&mut self, _: &GoUp, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate_up_with_watcher(cx);
        cx.notify();
    }

    pub(super) fn handle_cancel_drag(
        &mut self,
        _: &CancelDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stopped_drag = cx.stop_active_drag(window);
        let stopped_mouse_selection_drag = self.cancel_mouse_selection_drag();
        let cleared_drop_indicator = self.clear_drop_indicator();
        let cleared_sidebar_drag = self.dragging_sidebar_item.take().is_some();

        if stopped_drag
            || stopped_mouse_selection_drag
            || cleared_drop_indicator
            || cleared_sidebar_drag
        {
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn handle_open_selected(
        &mut self,
        _: &OpenSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(action) = self.activate_focused_entry_with_watcher(false, cx) {
            self.perform_entry_action(action, window, cx);
        }
        cx.notify();
    }

    pub(super) fn handle_open_selected_in_new_tab(
        &mut self,
        _: &OpenSelectedInNewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(action) = self.activate_focused_entry_in_new_tab_with_watcher(false, cx) {
            self.perform_entry_action(action, window, cx);
        }
        cx.notify();
    }

    pub(super) fn handle_open_settings(
        &mut self,
        _: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = cx
            .global::<crate::settings::SettingsState>()
            .settings_path()
            .map(Path::to_path_buf);
        match path {
            Some(path) => self.open_file_with_default_app(&path, window, cx),
            None => {
                self.open_error = Some(
                    "Could not open settings.json: settings file path is unavailable".to_owned(),
                )
            }
        }
        cx.notify();
    }

    pub(super) fn handle_enter_selected(
        &mut self,
        _: &EnterSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(action) = self.activate_focused_entry_with_watcher(true, cx) {
            self.perform_entry_action(action, window, cx);
        }
        cx.notify();
    }

    pub(super) fn handle_enter_selected_in_new_tab(
        &mut self,
        _: &EnterSelectedInNewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(action) = self.activate_focused_entry_in_new_tab_with_watcher(true, cx) {
            self.perform_entry_action(action, window, cx);
        }
        cx.notify();
    }

    pub(super) fn handle_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.refresh_with_entry_metadata_and_search_resolution(cx);
        cx.notify();
    }

    pub(super) fn handle_select_all(
        &mut self,
        _: &SelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_all_entries();
        cx.notify();
    }

    pub(super) fn handle_copy_selected(
        &mut self,
        _: &CopySelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_to_clipboard(cx);
        cx.notify();
    }

    pub(super) fn handle_cut_selected(
        &mut self,
        _: &CutSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cut_selected_to_clipboard(cx);
        cx.notify();
    }

    pub(super) fn handle_paste_clipboard(
        &mut self,
        _: &PasteClipboard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste_clipboard(window, cx);
        cx.notify();
    }

    pub(super) fn handle_undo_file_operation(
        &mut self,
        _: &UndoFileOperation,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.undo_file_operation(cx);
        cx.notify();
    }

    pub(super) fn handle_trash_selected(
        &mut self,
        _: &TrashSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.trash_selected_paths(cx);
        cx.notify();
    }

    pub(super) fn handle_permanently_delete_selected(
        &mut self,
        _: &PermanentlyDeleteSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_permanent_delete_selected(cx);
        cx.notify();
    }

    pub(super) fn handle_create_new_folder(
        &mut self,
        _: &CreateNewFolder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_new_folder(window, cx);
        cx.notify();
    }

    pub(super) fn handle_create_new_file(
        &mut self,
        _: &CreateNewFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_new_file(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        clipboard::{FileClipboardOperation, file_clipboard_from_item},
        test_support::{TempDir, selected_names, test_view_entity, test_view_entity_at_path},
    };
    use gpui::{AppContext, ClipboardItem, Image, ImageFormat, TestAppContext};
    use std::fs;

    #[gpui::test]
    fn selection_action_handlers_move_extend_and_select_all(cx: &mut TestAppContext) {
        let (_temp, view, cx) = test_view_entity(cx, &["a.txt", "b.txt", "c.txt"]);

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_move_down(&MoveDown, window, cx);
                assert_eq!(selected_names(view), vec!["a.txt"]);

                view.handle_move_down(&MoveDown, window, cx);
                assert_eq!(selected_names(view), vec!["b.txt"]);

                view.handle_extend_down(&ExtendDown, window, cx);
                assert_eq!(selected_names(view), vec!["b.txt", "c.txt"]);

                view.handle_move_home(&MoveHome, window, cx);
                assert_eq!(selected_names(view), vec!["a.txt"]);

                view.handle_extend_end(&ExtendEnd, window, cx);
                assert_eq!(selected_names(view), vec!["a.txt", "b.txt", "c.txt"]);

                view.handle_select_all(&SelectAll, window, cx);
                assert_eq!(selected_names(view), vec!["a.txt", "b.txt", "c.txt"]);
            });
        });
    }

    #[gpui::test]
    fn clipboard_action_handlers_copy_and_mark_cut_selection(cx: &mut TestAppContext) {
        let (temp, view, cx) = test_view_entity(cx, &["a.txt", "b.txt"]);
        let cut_path = temp.path().join("b.txt");

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.select_single_index(1);
                view.handle_copy_selected(&CopySelected, window, cx);
                assert!(view.cut_paths.is_empty());

                view.handle_cut_selected(&CutSelected, window, cx);
                assert!(view.entry_is_cut(&cut_path));
            });
        });

        let item = cx.read_from_clipboard().expect("clipboard item");
        let clipboard = file_clipboard_from_item(&item).expect("file clipboard");
        assert_eq!(clipboard.operation, FileClipboardOperation::Cut);
        assert_eq!(clipboard.paths, vec![cut_path]);
    }

    #[gpui::test]
    fn create_item_action_handlers_create_and_select_new_entries(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let root = temp.path().to_path_buf();
        let (view, cx) = test_view_entity_at_path(cx, root.clone());

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_create_new_folder(&CreateNewFolder, window, cx);
                assert!(root.join("New folder").is_dir());
                assert_eq!(selected_names(view), vec!["New folder"]);

                view.handle_create_new_file(&CreateNewFile, window, cx);
                assert!(root.join("New file").is_file());
                assert_eq!(selected_names(view), vec!["New file"]);
            });
        });
    }

    #[gpui::test]
    fn navigation_action_handlers_update_history_and_refresh_entries(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("inside.txt"), b"inside").unwrap();
        let root = temp.path().to_path_buf();
        let (view, cx) = test_view_entity_at_path(cx, child.clone());

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_go_up(&GoUp, window, cx);
                assert_eq!(view.path, root);
            });
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["child"]);
        });

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_go_back(&GoBack, window, cx);
                assert_eq!(view.path, child);
            });
        });
        cx.run_until_parked();

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_go_forward(&GoForward, window, cx);
                assert_eq!(view.path, root);
            });
        });
        cx.run_until_parked();

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                fs::write(root.join("new.txt"), b"new").unwrap();
                view.handle_refresh(&Refresh, window, cx);
            });
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.entries.iter().any(|entry| entry.name == "new.txt"));
        });
    }

    #[gpui::test]
    fn open_settings_action_reports_unavailable_settings_path(cx: &mut TestAppContext) {
        let (_temp, view, cx) = test_view_entity(cx, &["a.txt"]);

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_open_settings(&OpenSettings, window, cx);
                assert_eq!(
                    view.open_error.as_deref(),
                    Some("Could not open settings.json: settings file path is unavailable")
                );
            });
        });
    }

    #[gpui::test]
    fn permanent_delete_action_stages_selected_paths_for_confirmation(cx: &mut TestAppContext) {
        let (temp, view, cx) = test_view_entity(cx, &["a.txt", "b.txt"]);
        let selected = temp.path().join("b.txt");

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.select_single_path(&selected);
                view.handle_permanently_delete_selected(&PermanentlyDeleteSelected, window, cx);
                assert_eq!(
                    view.pending_permanent_delete
                        .as_ref()
                        .map(|pending| pending.paths.as_slice()),
                    Some([selected.clone()].as_slice())
                );
                assert!(view.open_error.is_none());
            });
        });
    }

    #[gpui::test]
    fn paste_clipboard_action_saves_clipboard_image(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let root = temp.path().to_path_buf();
        let (view, cx) = test_view_entity_at_path(cx, root.clone());
        let image = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3, 4]);
        cx.update(|_, app| app.write_to_clipboard(ClipboardItem::new_image(&image)));

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_paste_clipboard(&PasteClipboard, window, cx);
                assert_eq!(fs::read(root.join("image.png")).unwrap(), vec![1, 2, 3, 4]);
                assert_eq!(selected_names(view), vec!["image.png"]);
            });
        });
    }
}
