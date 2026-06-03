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
        EnterSelected,
        Refresh,
        SelectAll,
        CopySelected,
        CutSelected,
        PasteClipboard,
        TrashSelected,
        PermanentlyDeleteSelected
    ]
);

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
        self.navigate_back();
        cx.notify();
    }

    pub(super) fn handle_go_forward(
        &mut self,
        _: &GoForward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_forward();
        cx.notify();
    }

    pub(super) fn handle_go_up(&mut self, _: &GoUp, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate_up();
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

        if stopped_drag || stopped_mouse_selection_drag || cleared_drop_indicator {
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
        let _ = self.activate_focused_entry(false);
        cx.notify();
    }

    pub(super) fn handle_enter_selected(
        &mut self,
        _: &EnterSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(EntryAction::OpenFile(path)) = self.activate_focused_entry(true) {
            self.open_file_with_default_app(&path);
        }
        cx.notify();
    }

    pub(super) fn handle_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.reload();
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
        self.trash_selected_paths();
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
}
