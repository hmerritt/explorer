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
        OpenSelected,
        EnterSelected,
        Refresh,
        SelectAll
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
}
