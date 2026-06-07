use std::path::Path;

use gpui::{Context, Window, actions};

use crate::explorer::{
    navigation::EntryAction,
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
        OpenSettings,
        EnterSelected,
        Refresh,
        SelectAll,
        CopySelected,
        CutSelected,
        PasteClipboard,
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
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.activate_focused_entry_with_watcher(false, cx);
        cx.notify();
    }

    pub(super) fn handle_open_settings(
        &mut self,
        _: &OpenSettings,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = cx
            .global::<crate::settings::SettingsState>()
            .settings_path()
            .map(Path::to_path_buf);
        self.open_settings_file(path.as_deref());
        cx.notify();
    }

    pub(super) fn handle_enter_selected(
        &mut self,
        _: &EnterSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(EntryAction::OpenFile(path)) =
            self.activate_focused_entry_with_watcher(true, cx)
        {
            self.open_file_with_default_app(&path);
        }
        cx.notify();
    }

    pub(super) fn handle_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.reload();
        self.refresh_search_after_external_change(cx);
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
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste_clipboard_files(cx);
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
