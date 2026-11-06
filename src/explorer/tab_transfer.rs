use super::*;
use gpui::{AnyWindowHandle, Global, NativeDragResult, WeakEntity, WindowBounds, WindowId};

/// A live workspace, including pane-local work and restore history.
#[derive(Clone)]
pub(crate) struct TabTransfer {
    tab: ExplorerTab,
    history: VecDeque<ClosedTabEntry>,
    background: Vec<Entity<ExplorerView>>,
}

struct TabDragSession {
    serial: Rc<()>,
    source: WeakEntity<ExplorerTabs>,
    tab: TabId,
    window: AnyWindowHandle,
    bounds: WindowBounds,
    offset: Point<Pixels>,
    hovered: Option<WindowId>,
    transferred: bool,
}

impl Global for TabDragSession {}

impl ExplorerTabs {
    pub(super) fn begin_native_tab_drag(
        &mut self,
        tab: TabId,
        serial: Rc<()>,
        offset: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_tab_drag(tab);
        cx.set_global(TabDragSession {
            serial,
            source: cx.weak_entity(),
            tab,
            window: window.window_handle(),
            bounds: window.window_bounds(),
            offset,
            hovered: Some(window.window_handle().window_id()),
            transferred: false,
        });
    }

    pub(super) fn owns_tab_drag(&self, drag: &TabDrag, cx: &Context<Self>) -> bool {
        drag.source
            .as_ref()
            .is_none_or(|source| source.entity_id() == cx.entity_id())
    }

    fn snapshot_transfer(&self, id: TabId) -> Option<TabTransfer> {
        Some(TabTransfer {
            tab: self.tabs.iter().find(|tab| tab.id == id)?.clone(),
            history: self
                .closed_tabs
                .iter()
                .filter(|entry| matches!(entry, ClosedTabEntry::Pane { owner, .. } if *owner == id))
                .cloned()
                .collect(),
            background: if self.tabs.len() == 1 {
                self.background_operation_tabs.clone()
            } else {
                Vec::new()
            },
        })
    }

    fn remove_transferred_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        self.tabs.remove(index);
        self.closed_tabs
            .retain(|entry| !matches!(entry, ClosedTabEntry::Pane { owner, .. } if *owner == id));
        if self.tabs.is_empty() {
            self.background_operation_tabs.clear();
        } else if self.active_tab == id {
            self.active_tab = self.tabs[index.min(self.tabs.len() - 1)].id;
            self.focus_active_tab(window, cx);
            self.scroll_active_tab_into_view();
        }
        self.tab_context_menu = None;
        self.clear_obsolete_layout_state();
        cx.notify();
    }

    fn adopt_transfer(
        &mut self,
        mut transfer: TabTransfer,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TabId {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        let mut panes = HashMap::new();
        let mut splits = HashMap::new();
        transfer.tab.id = id;
        transfer.tab.active_pane =
            remap_pane(transfer.tab.active_pane, &mut panes, &mut self.next_pane_id);
        for pane in &mut transfer.tab.panes {
            pane.id = remap_pane(pane.id, &mut panes, &mut self.next_pane_id);
            observe_tab_view(&pane.view, window, cx);
        }
        remap_layout(
            &mut transfer.tab.layout,
            &mut panes,
            &mut splits,
            &mut self.next_pane_id,
            &mut self.next_split_id,
        );
        for entry in &mut transfer.history {
            if let ClosedTabEntry::Pane {
                owner,
                tab_index,
                pane,
                split,
            } = entry
            {
                *owner = id;
                *tab_index = index;
                pane.id = remap_pane(pane.id, &mut panes, &mut self.next_pane_id);
                split.id = remap_split(split.id, &mut splits, &mut self.next_split_id);
                remap_layout(
                    &mut split.sibling,
                    &mut panes,
                    &mut splits,
                    &mut self.next_pane_id,
                    &mut self.next_split_id,
                );
            }
        }
        for entry in transfer.history {
            self.remember_closed(entry);
        }
        for view in &transfer.background {
            observe_tab_view(view, window, cx);
        }
        self.background_operation_tabs.extend(transfer.background);
        self.cancel_active_tab_thumbnail_extraction(cx);
        self.tabs.insert(index.min(self.tabs.len()), transfer.tab);
        self.active_tab = id;
        self.scroll_active_tab_into_view();
        self.focus_active_tab(window, cx);
        cx.notify();
        id
    }

    pub(crate) fn from_transfer(
        transfer: TabTransfer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        observe_settings(cx);
        observe_window_activation(window, cx);
        start_clipboard_summary_poll(window, cx);
        let mut tabs = Self {
            tabs: Vec::new(),
            closed_tabs: VecDeque::new(),
            active_tab: TabId(0),
            next_tab_id: 1,
            next_pane_id: 1,
            next_split_id: 1,
            background_operation_tabs: Vec::new(),
            dragging_tab: None,
            dock_target: None,
            pane_bounds: HashMap::new(),
            split_bounds: HashMap::new(),
            split_resize_drag: None,
            tab_context_menu: None,
            tab_scroll_handle: ScrollHandle::new(),
            tab_insert_target: None,
            should_move_window: false,
            transfer_panel_collapsed: false,
        };
        tabs.adopt_transfer(transfer, 0, window, cx);
        tabs
    }

    /// Adopt before removing source ownership. Both window handles have been
    /// validated and no asynchronous boundary occurs during this transaction.
    pub(super) fn receive_tab_drag(
        &mut self,
        drag: &TabDrag,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<TabId> {
        if self.owns_tab_drag(drag, cx) {
            return None;
        }
        let source = drag.source.as_ref()?.upgrade()?;
        let session = cx.try_global::<TabDragSession>()?;
        if !Rc::ptr_eq(&session.serial, &drag.session)
            || session.transferred
            || session.source.entity_id() != source.entity_id()
            || session.tab != drag.id
        {
            return None;
        }
        let source_window = session.window;
        let transfer = source.read(cx).snapshot_transfer(drag.id)?;
        // Confirm the native source still exists before rebinding its panes.
        if source_window.update(cx, |_, _, _| ()).is_err() {
            return None;
        }
        let id = self.adopt_transfer(transfer, index, window, cx);
        let _ = source_window.update(cx, |_, window, cx| {
            source.update(cx, |tabs, cx| {
                tabs.remove_transferred_tab(drag.id, window, cx)
            });
        });
        cx.update_global::<TabDragSession, _>(|session, _| session.transferred = true);
        window.activate_window();
        Some(id)
    }

    pub(super) fn update_tab_insertion(
        &mut self,
        drag: &TabDrag,
        target: TabId,
        before: bool,
        cx: &mut Context<Self>,
    ) {
        if self.owns_tab_drag(drag, cx) {
            return;
        }
        let next = Some((target, before));
        if self.tab_insert_target != next {
            self.tab_insert_target = next;
            cx.notify();
        }
    }

    pub(super) fn drop_tab_on_strip(
        &mut self,
        drag: &TabDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.owns_tab_drag(drag, cx) {
            self.clear_tab_drag();
            self.dock_target = None;
            cx.notify();
            return;
        }
        let index = self
            .tab_insert_target
            .and_then(|(target, before)| {
                self.tabs
                    .iter()
                    .position(|tab| tab.id == target)
                    .map(|index| index + usize::from(!before))
            })
            .unwrap_or(self.tabs.len());
        self.receive_tab_drag(drag, index, window, cx);
        self.tab_insert_target = None;
        cx.stop_propagation();
        cx.notify();
    }
}

fn remap_pane(id: PaneId, ids: &mut HashMap<PaneId, PaneId>, next: &mut u64) -> PaneId {
    *ids.entry(id).or_insert_with(|| {
        let id = PaneId(*next);
        *next += 1;
        id
    })
}

fn remap_split(id: u64, ids: &mut HashMap<u64, u64>, next: &mut u64) -> u64 {
    *ids.entry(id).or_insert_with(|| {
        let id = *next;
        *next += 1;
        id
    })
}

fn remap_layout(
    layout: &mut PaneNode,
    panes: &mut HashMap<PaneId, PaneId>,
    splits: &mut HashMap<u64, u64>,
    next_pane: &mut u64,
    next_split: &mut u64,
) {
    match layout {
        PaneNode::Leaf(id) => *id = remap_pane(*id, panes, next_pane),
        PaneNode::Split {
            id, first, second, ..
        } => {
            *id = remap_split(*id, splits, next_split);
            remap_layout(first, panes, splits, next_pane, next_split);
            remap_layout(second, panes, splits, next_pane, next_split);
        }
    }
}

pub(super) fn track_window(event: &FileDropEvent, window: &Window, cx: &mut App) {
    if !cx.has_global::<TabDragSession>() {
        return;
    }
    let id = window.window_handle().window_id();
    cx.update_global::<TabDragSession, _>(|session, _| match event {
        FileDropEvent::Exited if session.hovered == Some(id) => session.hovered = None,
        FileDropEvent::Entered { .. }
        | FileDropEvent::Pending { .. }
        | FileDropEvent::Submit { .. } => session.hovered = Some(id),
        _ => {}
    });
}

pub(super) fn hover_window(window: &Window, cx: &mut App) {
    if cx.has_global::<TabDragSession>() {
        cx.update_global::<TabDragSession, _>(|session, _| {
            session.hovered = Some(window.window_handle().window_id());
        });
    }
}

pub(super) fn finish_drag(
    source: WeakEntity<ExplorerTabs>,
    serial: Rc<()>,
    result: NativeDragResult,
    cx: &mut App,
) {
    if cx
        .try_global::<TabDragSession>()
        .is_none_or(|s| s.source != source || !Rc::ptr_eq(&s.serial, &serial))
    {
        return;
    }
    let session = cx.remove_global::<TabDragSession>();
    clear_transfer_previews(cx);
    let Some(source) = session.source.upgrade() else {
        return;
    };
    if !session.transferred && session.hovered.is_none() {
        if let NativeDragResult::Released { position } = result {
            if let Some(transfer) = source.read(cx).snapshot_transfer(session.tab) {
                match crate::app::open_transferred_explorer_window(
                    transfer,
                    session.bounds,
                    position,
                    session.offset,
                    cx,
                ) {
                    Ok(()) => {
                        let _ = session.window.update(cx, |_, window, cx| {
                            source.update(cx, |tabs, cx| {
                                tabs.remove_transferred_tab(session.tab, window, cx)
                            });
                        });
                    }
                    Err(error) => {
                        source.update(cx, |tabs, cx| {
                            if let Some(view) = tabs.tab_view(session.tab) {
                                view.update(cx, |view, cx| {
                                    view.set_error_notice(format!(
                                        "Could not open window: {error}"
                                    ));
                                    cx.notify();
                                });
                            }
                        });
                    }
                }
            }
        }
    }
    let empty = source.update(cx, |tabs, cx| {
        tabs.clear_tab_drag();
        tabs.dock_target = None;
        tabs.tab_insert_target = None;
        cx.notify();
        tabs.tabs.is_empty()
    });
    // The destination exists and the native drag loop has already completed.
    if empty {
        let _ = session
            .window
            .update(cx, |_, window, _| window.remove_window());
    }
}

fn clear_transfer_previews(cx: &mut App) {
    // Clear destination previews even if the source has already been destroyed.
    for handle in cx.windows() {
        let _ = handle.update(cx, |root, _, cx| {
            let tabs = if let Ok(root) = root.clone().downcast::<crate::app::Explorer>() {
                Some(root.read(cx).explorer.clone())
            } else {
                root.downcast::<ExplorerTabs>().ok()
            };
            if let Some(tabs) = tabs {
                tabs.update(cx, |tabs, cx| {
                    tabs.tab_insert_target = None;
                    tabs.dock_target = None;
                    cx.notify();
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::view::NavigationLocation;
    use crate::settings::{ExplorerSettings, SettingsState};
    use gpui::{AppContext, TestAppContext, WindowHandle, point};

    fn workspace(
        cx: &mut TestAppContext,
        path: &Path,
    ) -> (WindowHandle<ExplorerTabs>, Entity<ExplorerTabs>) {
        let path = path.to_path_buf();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    focus.focus(window);
                    ExplorerTabs::new_for_test(path, focus, cx)
                })
            })
            .unwrap()
        });
        let tabs = window.root(cx).unwrap();
        (window, tabs)
    }

    fn begin(
        cx: &mut TestAppContext,
        window: WindowHandle<ExplorerTabs>,
        tabs: &Entity<ExplorerTabs>,
    ) -> TabDrag {
        let serial = Rc::new(());
        let source = tabs.downgrade();
        window
            .update(cx, |tabs, window, cx| {
                tabs.begin_native_tab_drag(
                    tabs.active_tab,
                    serial.clone(),
                    point(px(20.), px(12.)),
                    window,
                    cx,
                );
            })
            .unwrap();
        cx.read_entity(tabs, |tabs, cx| {
            let tab = tabs.active_tab().unwrap();
            TabDrag {
                session: serial,
                source: Some(source),
                id: tab.id,
                label: "source".into(),
                path: tab.view.read(cx).path().to_path_buf(),
                is_active: true,
                dockable: !tab.is_split(),
            }
        })
    }

    #[gpui::test]
    fn transfer_remaps_colliding_ids_preserves_live_state_and_rebinds_events(
        cx: &mut TestAppContext,
    ) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        let (source_window, source) = workspace(cx, temp.path());
        let (target_window, target) = workspace(cx, temp.path());
        let view = cx.read_entity(&source, |tabs, _| tabs.tabs[0].view.clone());
        // The source may also be keeping work alive for a previously closed tab.
        let background = source_window
            .update(cx, |tabs, window, cx| {
                let view = cx.new(|_| ExplorerView::new(temp.path().to_path_buf()));
                view.update(cx, |view, _| {
                    view.archive_copy_task = Some(gpui::Task::ready(()))
                });
                observe_tab_view(&view, window, cx);
                tabs.background_operation_tabs.push(view.clone());
                view
            })
            .unwrap();
        view.update(cx, |view, _| {
            view.back_stack
                .push(NavigationLocation::Directory(temp.path().join("history")));
            view.download_batch_active = true;
        });
        // Both workspaces initially have TabId(1), PaneId(1).
        let drag = begin(cx, source_window, &source);
        let inserted = target_window
            .update(cx, |tabs, window, cx| {
                tabs.receive_tab_drag(&drag, 0, window, cx).unwrap()
            })
            .unwrap();
        cx.read_entity(&target, |tabs, cx| {
            assert_ne!(inserted, TabId(1));
            assert_eq!(tabs.tabs[0].view, view);
            assert_ne!(tabs.tabs[0].active_pane, tabs.tabs[1].active_pane);
            assert_eq!(view.read(cx).back_stack.len(), 1);
            assert!(view.read(cx).download_batch_active);
            assert!(view.read(cx).hosted_window == Some(*target_window));
            assert_eq!(tabs.background_operation_tabs, vec![background.clone()]);
            assert!(background.read(cx).hosted_window == Some(*target_window));
        });
        // A moved view's workspace events must reach its new owner only.
        view.update(cx, |_, cx| {
            cx.emit(ExplorerViewEvent::OpenDirectoryInNewTab(
                temp.path().to_path_buf(),
            ))
        });
        cx.run_until_parked();
        cx.read_entity(&target, |tabs, _| assert_eq!(tabs.tabs.len(), 3));
        cx.read_entity(&source, |tabs, _| assert!(tabs.tabs.is_empty()));
        // The source stays open until the native drag loop has finished.
        assert!(source_window.root(cx).is_ok());
        cx.update(|cx| {
            finish_drag(
                source.downgrade(),
                drag.session,
                NativeDragResult::Released { position: None },
                cx,
            )
        });
        assert!(source_window.root(cx).is_err());
        assert_eq!(cx.windows().len(), 1);
        background.update(cx, |view, cx| {
            view.archive_copy_task = None;
            cx.notify();
        });
        cx.run_until_parked();
        cx.read_entity(&target, |tabs, _| {
            assert!(tabs.background_operation_tabs.is_empty())
        });
    }

    #[gpui::test]
    fn transfer_docks_at_all_four_edges_with_colliding_tab_ids(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        for direction in [
            SplitDirection::Left,
            SplitDirection::Right,
            SplitDirection::Up,
            SplitDirection::Down,
        ] {
            let (source_window, source) = workspace(cx, temp.path());
            let (target_window, target) = workspace(cx, temp.path());
            let moved = cx.read_entity(&source, |tabs, _| tabs.tabs[0].view.clone());
            let drag = begin(cx, source_window, &source);
            target_window
                .update(cx, |tabs, window, cx| {
                    let destination = tabs.active_tab;
                    let pane = tabs.tabs[0].active_pane;
                    let incoming = tabs.receive_tab_drag(&drag, 1, window, cx).unwrap();
                    assert!(tabs.split_tab_into_pane(
                        incoming,
                        destination,
                        pane,
                        direction,
                        window,
                        cx
                    ));
                    assert_eq!(tabs.tabs.len(), 1);
                    assert_eq!(tabs.tabs[0].layout.pane_count(), 2);
                    assert_eq!(tabs.tabs[0].active_view(), moved);
                    assert_ne!(tabs.tabs[0].panes[0].id, tabs.tabs[0].panes[1].id);
                })
                .unwrap();
            cx.update(|cx| {
                finish_drag(
                    source.downgrade(),
                    drag.session,
                    NativeDragResult::Released { position: None },
                    cx,
                )
            });
            assert!(source_window.root(cx).is_err());
            cx.read_entity(&target, |tabs, _| assert_eq!(tabs.tabs[0].panes.len(), 2));
        }
    }

    #[gpui::test]
    fn composite_transfer_keeps_panes_layout_and_closed_pane_restore_history(
        cx: &mut TestAppContext,
    ) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        let (source_window, source) = workspace(cx, temp.path());
        let (target_window, target) = workspace(cx, temp.path());
        source_window
            .update(cx, |tabs, window, cx| {
                assert!(tabs.self_dock_active_tab(
                    tabs.active_tab,
                    PaneId(1),
                    SplitDirection::Right,
                    window,
                    cx
                ));
                let pane = tabs.tabs[0].panes[1].clone();
                tabs.closed_tabs.push_back(ClosedTabEntry::Pane {
                    owner: tabs.active_tab,
                    tab_index: 0,
                    pane: ClosedPane {
                        id: PaneId(99),
                        path: temp.path().to_path_buf(),
                        sidebar_group: None,
                    },
                    split: ClosedSplit {
                        id: 99,
                        axis: SplitAxis::Horizontal,
                        ratio: 0.5,
                        closed_first: false,
                        sibling: PaneNode::Leaf(pane.id),
                    },
                });
            })
            .unwrap();
        let original_views = cx.read_entity(&source, |tabs, _| {
            tabs.tabs[0]
                .panes
                .iter()
                .map(|pane| pane.view.clone())
                .collect::<Vec<_>>()
        });
        let drag = begin(cx, source_window, &source);
        assert!(!drag.dockable);
        target_window.update(cx, |tabs, window, cx| {
            let id = tabs.receive_tab_drag(&drag, 1, window, cx).unwrap();
            let tab = tabs.tabs.iter().find(|tab| tab.id == id).unwrap();
            assert_eq!(tab.layout.pane_count(), 2);
            assert_eq!(tab.panes.iter().map(|pane| pane.view.clone()).collect::<Vec<_>>(), original_views);
            assert!(matches!(tabs.closed_tabs.back(), Some(ClosedTabEntry::Pane { owner, split, .. }) if *owner == id && tab.layout.contains(match split.sibling { PaneNode::Leaf(id) => id, _ => panic!("leaf sibling") })));
            assert!(!tabs.closed_tabs.iter().any(|entry| matches!(entry, ClosedTabEntry::Tab { .. })));
        }).unwrap();
        cx.read_entity(&source, |tabs, _| assert!(tabs.closed_tabs.is_empty()));
        cx.read_entity(&target, |tabs, _| assert_eq!(tabs.tabs.len(), 2));
    }

    #[gpui::test]
    fn cancellation_invalid_drop_and_stale_completion_never_detach(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        let (window, tabs) = workspace(cx, temp.path());
        let first = begin(cx, window, &tabs);
        let current = begin(cx, window, &tabs);
        cx.update(|cx| {
            finish_drag(
                tabs.downgrade(),
                first.session,
                NativeDragResult::Released { position: None },
                cx,
            )
        });
        cx.update(|cx| assert!(cx.has_global::<TabDragSession>()));
        // Releasing inside an Explorer window without a target does nothing.
        cx.update(|cx| {
            finish_drag(
                tabs.downgrade(),
                current.session,
                NativeDragResult::Released { position: None },
                cx,
            )
        });
        assert_eq!(cx.windows().len(), 1);
        for result in [NativeDragResult::Cancelled, NativeDragResult::Failed] {
            let drag = begin(cx, window, &tabs);
            cx.update(|cx| {
                cx.update_global::<TabDragSession, _>(|session, _| session.hovered = None);
                finish_drag(tabs.downgrade(), drag.session.clone(), result, cx);
                finish_drag(
                    tabs.downgrade(),
                    drag.session,
                    NativeDragResult::Released { position: None },
                    cx,
                );
            });
            assert_eq!(cx.windows().len(), 1);
            cx.read_entity(&tabs, |tabs, _| {
                assert_eq!(tabs.tabs.len(), 1);
                assert!(tabs.dragging_tab.is_none());
            });
        }
    }

    #[gpui::test]
    fn repeated_or_expired_transfer_is_rejected(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        let (source_window, source) = workspace(cx, temp.path());
        let (target_window, _) = workspace(cx, temp.path());
        let mut drag = begin(cx, source_window, &source);
        let real_serial = drag.session.clone();
        drag.session = Rc::new(());
        target_window
            .update(cx, |tabs, window, cx| {
                assert!(tabs.receive_tab_drag(&drag, 0, window, cx).is_none())
            })
            .unwrap();
        drag.session = real_serial;
        target_window
            .update(cx, |tabs, window, cx| {
                assert!(tabs.receive_tab_drag(&drag, 0, window, cx).is_some());
                assert!(tabs.receive_tab_drag(&drag, 0, window, cx).is_none());
                assert_eq!(tabs.tabs.len(), 2);
            })
            .unwrap();
    }

    #[gpui::test]
    fn outside_release_opens_one_window_with_the_live_tab_and_closes_empty_source(
        cx: &mut TestAppContext,
    ) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        let (window, tabs) = workspace(cx, temp.path());
        let original = cx.read_entity(&tabs, |tabs, _| tabs.tabs[0].view.clone());
        let drag = begin(cx, window, &tabs);
        cx.update(|cx| {
            cx.update_global::<TabDragSession, _>(|session, _| session.hovered = None);
            finish_drag(
                tabs.downgrade(),
                drag.session.clone(),
                NativeDragResult::Released {
                    position: Some(point(px(400.), px(100.))),
                },
                cx,
            );
        });
        assert!(window.root(cx).is_err());
        let windows = cx.windows();
        assert_eq!(windows.len(), 1);
        windows[0]
            .update(cx, |root, window, cx| {
                let root = root.downcast::<crate::app::Explorer>().unwrap();
                let target = root.read(cx).explorer.clone();
                assert_eq!(target.read(cx).tabs.len(), 1);
                assert_eq!(target.read(cx).tabs[0].view, original);
                assert_eq!(window.bounds().origin, point(px(380.), px(88.)));
            })
            .unwrap();
        cx.update(|cx| {
            finish_drag(
                tabs.downgrade(),
                drag.session,
                NativeDragResult::Released { position: None },
                cx,
            )
        });
        assert_eq!(cx.windows().len(), 1);
    }

    #[gpui::test]
    fn failed_detach_keeps_source_tab_and_reports_error(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        let (window, tabs) = workspace(cx, temp.path());
        let original = cx.read_entity(&tabs, |tabs, _| tabs.tabs[0].view.clone());
        let drag = begin(cx, window, &tabs);
        cx.fail_next_window_open();
        cx.update(|cx| {
            cx.update_global::<TabDragSession, _>(|session, _| session.hovered = None);
            finish_drag(
                tabs.downgrade(),
                drag.session,
                NativeDragResult::Released { position: None },
                cx,
            );
        });
        assert_eq!(cx.windows().len(), 1);
        assert!(window.root(cx).is_ok());
        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), 1);
            assert_eq!(tabs.tabs[0].view, original);
            assert!(tabs.dragging_tab.is_none());
            assert!(original.read(cx).operation_notice.is_some());
        });
    }

    #[gpui::test]
    fn moved_editors_receive_focus_events_only_in_destination(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        let path = temp.path().join("item.txt");
        std::fs::write(&path, "contents").unwrap();
        for editor in 0..3 {
            let (source_window, source) = workspace(cx, temp.path());
            let (target_window, _) = workspace(cx, temp.path());
            let view = cx.read_entity(&source, |tabs, _| tabs.tabs[0].view.clone());
            let focus = source_window
                .update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| match editor {
                        0 => {
                            assert!(view.start_rename_for_path(&path, window, cx));
                            view.active_rename
                                .as_ref()
                                .unwrap()
                                .focus_handle
                                .clone()
                                .unwrap()
                        }
                        1 => {
                            assert!(view.start_address_bar_edit(window, cx));
                            view.active_address_bar
                                .as_ref()
                                .unwrap()
                                .focus_handle
                                .clone()
                                .unwrap()
                        }
                        _ => {
                            assert!(view.start_search_edit(window, cx));
                            view.search.focus_handle.clone().unwrap()
                        }
                    })
                })
                .unwrap();
            let drag = begin(cx, source_window, &source);
            target_window
                .update(cx, |tabs, window, cx| {
                    tabs.receive_tab_drag(&drag, 1, window, cx).unwrap();
                })
                .unwrap();
            source_window
                .update(cx, |_, window, cx| cx.focus_handle().focus(window))
                .unwrap();
            cx.run_until_parked();
            cx.read_entity(&view, |view, _| match editor {
                0 => assert!(view.active_rename.is_some()),
                1 => assert!(view.active_address_bar.is_some()),
                _ => assert!(view.search.focus_handle.is_some()),
            });
            target_window
                .update(cx, |_, window, _| focus.focus(window))
                .unwrap();
            cx.run_until_parked();
            target_window
                .update(cx, |_, window, cx| cx.focus_handle().focus(window))
                .unwrap();
            cx.run_until_parked();
            cx.read_entity(&view, |view, _| match editor {
                0 => assert!(view.active_rename.is_none()),
                1 => assert!(view.active_address_bar.is_none()),
                _ => assert!(view.search.focus_handle.is_none()),
            });
            cx.update(|cx| {
                finish_drag(
                    source.downgrade(),
                    drag.session,
                    NativeDragResult::Released { position: None },
                    cx,
                )
            });
        }
    }

    #[gpui::test]
    fn pending_directory_load_finishes_in_destination_after_source_closes(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        let (source_window, source) = workspace(cx, temp.path());
        let (target_window, _) = workspace(cx, temp.path());
        let view = cx.read_entity(&source, |tabs, _| tabs.tabs[0].view.clone());
        let created = temp.path().join("created.txt");
        std::fs::write(&created, "contents").unwrap();
        source_window
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.reload_async_with_options_and_focused_rename(
                        crate::explorer::view::ReloadMode {
                            cache_policy:
                                crate::explorer::remote_directory_cache::DirectoryLoadPolicy::Fresh,
                            preserve_selection: true,
                            rebuild_sidebar: false,
                            preserve_context_menu: false,
                        },
                        vec![created.clone()],
                        created.clone(),
                        false,
                        false,
                        false,
                        window,
                        cx,
                    );
                });
            })
            .unwrap();
        let drag = begin(cx, source_window, &source);
        target_window
            .update(cx, |tabs, window, cx| {
                tabs.receive_tab_drag(&drag, 1, window, cx).unwrap();
            })
            .unwrap();
        cx.update(|cx| {
            finish_drag(
                source.downgrade(),
                drag.session,
                NativeDragResult::Released { position: None },
                cx,
            )
        });
        assert!(source_window.root(cx).is_err());
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.loading_path.is_none());
            assert!(view.hosted_window == Some(*target_window));
            assert_eq!(
                view.active_rename
                    .as_ref()
                    .map(|rename| &rename.original_path),
                Some(&created)
            );
        });
    }

    #[gpui::test]
    fn destination_closure_during_drag_preserves_source(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        let (source_window, source) = workspace(cx, temp.path());
        let (target_window, _) = workspace(cx, temp.path());
        let drag = begin(cx, source_window, &source);
        target_window
            .update(cx, |_, window, cx| {
                hover_window(window, cx);
                window.remove_window();
            })
            .unwrap();
        cx.update(|cx| {
            finish_drag(
                source.downgrade(),
                drag.session,
                NativeDragResult::Released { position: None },
                cx,
            )
        });
        assert_eq!(cx.windows().len(), 1);
        cx.read_entity(&source, |tabs, _| assert_eq!(tabs.tabs.len(), 1));
    }

    #[gpui::test]
    fn transfer_retains_source_neighbor_and_inserts_at_requested_position(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = crate::explorer::test_support::TempDir::new();
        let (source_window, source) = workspace(cx, temp.path());
        let (target_window, target) = workspace(cx, temp.path());
        source_window
            .update(cx, |tabs, window, cx| {
                tabs.add_foreground_tab(temp.path().to_path_buf(), window, cx)
            })
            .unwrap();
        target_window
            .update(cx, |tabs, window, cx| {
                tabs.add_foreground_tab(temp.path().to_path_buf(), window, cx)
            })
            .unwrap();
        let target_views = cx.read_entity(&target, |tabs, _| {
            tabs.tabs
                .iter()
                .map(|tab| tab.view.clone())
                .collect::<Vec<_>>()
        });
        let moved = cx.read_entity(&source, |tabs, _| tabs.active_tab().unwrap().view.clone());
        let drag = begin(cx, source_window, &source);
        target_window
            .update(cx, |tabs, window, cx| {
                tabs.tab_insert_target = Some((tabs.tabs[1].id, true));
                tabs.drop_tab_on_strip(&drag, window, cx);
                assert_eq!(
                    tabs.tabs
                        .iter()
                        .map(|tab| tab.view.clone())
                        .collect::<Vec<_>>(),
                    vec![target_views[0].clone(), moved, target_views[1].clone()]
                );
            })
            .unwrap();
        cx.update(|cx| {
            finish_drag(
                source.downgrade(),
                drag.session,
                NativeDragResult::Released { position: None },
                cx,
            )
        });
        assert!(source_window.root(cx).is_ok());
        cx.read_entity(&source, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 1);
            assert_eq!(tabs.active_tab, tabs.tabs[0].id);
            assert!(tabs.closed_tabs.is_empty());
        });
    }
}
