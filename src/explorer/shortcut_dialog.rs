use std::path::PathBuf;

use gpui::{
    AnyWindowHandle, ClickEvent, Context, Div, Entity, Focusable, KeyDownEvent, PathPromptOptions,
    Render, Stateful, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowDecorations,
    WindowKind, WindowOptions, div, prelude::*, px, rgb, size,
};

use super::{
    RenameCancel, RenameCommit,
    dialog_placement::DialogPlacement,
    remote_dialog::DialogTextInput,
    remote_directory_cache::DirectoryLoadPolicy,
    shortcut::{ShortcutTarget, is_shortcut_directory, validate_shortcut_name},
    view::{ExplorerView, ReloadMode},
};

struct ShortcutDialog {
    explorer: WeakEntity<ExplorerView>,
    directory: PathBuf,
    destination: Entity<DialogTextInput>,
    name: Entity<DialogTextInput>,
    browsed_path: Option<PathBuf>,
    target: Option<ShortcutTarget>,
    pending: bool,
    error: Option<String>,
}

impl ExplorerView {
    pub(super) fn can_create_shortcut(&self) -> bool {
        !self.is_sidebar_group_view()
            && self.search_query().is_empty()
            && is_shortcut_directory(&self.path)
    }

    pub(super) fn open_create_shortcut_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_create_shortcut() || self.active_dialog_window.is_some() {
            return;
        }

        let dialog_size = size(px(500.0), px(265.0));
        let mut options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(dialog_size, cx)),
            window_min_size: Some(dialog_size),
            titlebar: Some(TitlebarOptions {
                title: Some("Create Shortcut".into()),
                ..Default::default()
            }),
            kind: WindowKind::Floating,
            is_resizable: false,
            is_minimizable: false,
            window_decorations: Some(WindowDecorations::Server),
            ..Default::default()
        };
        DialogPlacement::from_window(window, cx).apply(&mut options);
        let directory = self.path.clone();
        let explorer = cx.weak_entity();
        match cx.open_window(options, move |window, cx| {
            let destination = cx.new(|cx| {
                DialogTextInput::new(
                    String::new(),
                    "",
                    false,
                    cx.focus_handle().tab_index(0).tab_stop(true),
                )
            });
            let name = cx.new(|cx| {
                DialogTextInput::new(
                    String::new(),
                    "",
                    false,
                    cx.focus_handle().tab_index(0).tab_stop(true),
                )
            });
            destination.read(cx).focus_handle(cx).focus(window);
            let handle = window.window_handle();
            let dialog = cx.new(|cx| {
                cx.observe(&destination, |_, _, cx| cx.notify()).detach();
                cx.observe(&name, |_, _, cx| cx.notify()).detach();
                cx.on_release(move |dialog: &mut ShortcutDialog, cx| {
                    let _ = dialog.explorer.update(cx, |view, cx| {
                        if view.active_dialog_window == Some(handle) {
                            view.active_dialog_window = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
                ShortcutDialog {
                    explorer,
                    directory,
                    destination,
                    name,
                    browsed_path: None,
                    target: None,
                    pending: false,
                    error: None,
                }
            });
            let weak = dialog.downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                weak.upgrade()
                    .is_none_or(|dialog| !dialog.read(cx).saving())
            });
            dialog
        }) {
            Ok(handle) => self.active_dialog_window = Some(handle.into()),
            Err(error) => self.set_error_notice(format!("Could not open Create Shortcut: {error}")),
        }
    }
}

impl ShortcutDialog {
    fn input(&self) -> &Entity<DialogTextInput> {
        if self.target.is_some() {
            &self.name
        } else {
            &self.destination
        }
    }

    fn saving(&self) -> bool {
        self.pending && self.target.is_some()
    }

    fn cancel(&self, window: &mut Window) {
        if !self.saving() {
            window.remove_window();
        }
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.pending {
            self.target = None;
            self.error = None;
            self.destination.read(cx).focus_handle(cx).focus(window);
            cx.notify();
        }
    }

    fn advance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let input = self.input().read(cx).content().to_owned();
        if input.trim().is_empty() {
            return;
        }
        if let Some(target) = self.target.clone() {
            self.finish(target, input, window.window_handle(), cx);
            return;
        }

        let directory = self.directory.clone();
        let browsed_path = self
            .browsed_path
            .as_ref()
            .filter(|path| path.to_string_lossy() == input)
            .cloned();
        self.pending = true;
        self.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let original_input = input.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(path) = browsed_path {
                        ShortcutTarget::from_path(path)
                    } else {
                        ShortcutTarget::parse(&input, &directory)
                    }
                })
                .await;
            let _ = this.update_in(cx, |dialog, window, cx| {
                dialog.pending = false;
                if dialog.destination.read(cx).content() == original_input {
                    match result {
                        Ok(target) => {
                            dialog.name.update(cx, |input, cx| {
                                input.set_content(target.suggested_name(), true);
                                cx.notify();
                            });
                            dialog.target = Some(target);
                            dialog.name.read(cx).focus_handle(cx).focus(window);
                        }
                        Err(error) => {
                            dialog.error = Some(error);
                            dialog.destination.read(cx).focus_handle(cx).focus(window);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn finish(
        &mut self,
        target: ShortcutTarget,
        name: String,
        handle: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = validate_shortcut_name(&name) {
            self.error = Some(error);
            cx.notify();
            return;
        }

        self.pending = true;
        self.error = None;
        let directory = self.directory.clone();
        let explorer = self.explorer.clone();
        cx.spawn(async move |this, cx| {
            let save_directory = directory.clone();
            let result = cx
                .background_executor()
                .spawn(async move { target.create_in(&save_directory, &name) })
                .await;
            if let Ok(path) = &result {
                let _ = explorer.update(cx, |view, cx| {
                    if view.path == directory && view.can_create_shortcut() {
                        view.clear_operation_notice();
                        view.reload_async_with_options_preserving_live_selection(
                            ReloadMode {
                                cache_policy: DirectoryLoadPolicy::Fresh,
                                preserve_selection: true,
                                rebuild_sidebar: true,
                                preserve_context_menu: false,
                            },
                            vec![path.clone()],
                            true,
                            false,
                            false,
                            cx,
                        );
                    }
                    view.emit_filesystem_changed(cx);
                    cx.notify();
                });
            }
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |dialog, cx| {
                    dialog.pending = false;
                    match result {
                        Ok(_) => window.remove_window(),
                        Err(error) => {
                            dialog.error = Some(error);
                            dialog.name.read(cx).focus_handle(cx).focus(window);
                            cx.notify();
                        }
                    }
                });
            });
        })
        .detach();
        cx.notify();
    }

    fn browse(
        &mut self,
        files: bool,
        directories: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending || self.target.is_some() {
            return;
        }
        self.pending = true;
        self.error = None;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files,
            directories,
            multiple: false,
            prompt: Some("Browse for a shortcut destination".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |dialog, window, cx| {
                dialog.pending = false;
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            let text = path.to_string_lossy().into_owned();
                            dialog.destination.update(cx, |input, cx| {
                                input.set_content(text, false);
                                cx.notify();
                            });
                            dialog.browsed_path = Some(path);
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => dialog.error = Some(format!("Could not browse: {error}")),
                    Err(_) => dialog.error = Some("Could not open the file picker.".into()),
                }
                dialog.destination.read(cx).focus_handle(cx).focus(window);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for ShortcutDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let naming = self.target.is_some();
        let enabled = !self.pending;
        let input = self.input().clone();
        let can_advance = enabled && !input.read(cx).content().trim().is_empty();
        let mixed_picker = cx.can_select_mixed_files_and_dirs();
        div()
            .key_context("ExplorerRenameInput")
            .font(crate::settings::current_app_font(cx))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(20.0))
            .bg(rgb(0xffffff))
            .text_size(px(12.0))
            .text_color(rgb(0x202020))
            .on_key_down(|event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev();
                    } else {
                        window.focus_next();
                    }
                    cx.stop_propagation();
                }
            })
            .on_action(cx.listener(|dialog, _: &RenameCommit, window, cx| {
                if dialog.input().read(cx).focus_handle(cx).is_focused(window) {
                    dialog.advance(window, cx);
                    cx.stop_propagation();
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|dialog, _: &RenameCancel, window, cx| {
                dialog.cancel(window);
                cx.stop_propagation();
            }))
            .child(div().text_size(px(16.0)).child(if naming {
                "What would you like to name the shortcut?"
            } else {
                "What item would you like to create a shortcut for?"
            }))
            .child(if naming {
                "Type a name for this shortcut."
            } else {
                "Type the location of a file, folder, or Internet address."
            })
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(div().flex_1().min_w(px(0.0)).child(input))
                    .when(!naming, |row| {
                        row.child(
                            dialog_button(
                                "shortcut-browse",
                                if mixed_picker {
                                    "Browse…"
                                } else {
                                    "Browse files…"
                                },
                                1,
                                enabled,
                            )
                            .on_click(cx.listener(
                                move |dialog, _: &ClickEvent, window, cx| {
                                    dialog.browse(true, mixed_picker, window, cx);
                                },
                            )),
                        )
                        .when(!mixed_picker, |row| {
                            row.child(
                                dialog_button(
                                    "shortcut-browse-folders",
                                    "Browse folders…",
                                    2,
                                    enabled,
                                )
                                .on_click(cx.listener(
                                    |dialog, _: &ClickEvent, window, cx| {
                                        dialog.browse(false, true, window, cx);
                                    },
                                )),
                            )
                        })
                    }),
            )
            .when_some(self.error.clone(), |dialog, error| {
                dialog.child(div().text_color(rgb(0x9b1c1c)).child(error))
            })
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .when(naming, |row| {
                        row.child(dialog_button("shortcut-back", "Back", 3, enabled).on_click(
                            cx.listener(|dialog, _: &ClickEvent, window, cx| {
                                dialog.back(window, cx)
                            }),
                        ))
                    })
                    .child(
                        dialog_button(
                            if naming {
                                "shortcut-finish"
                            } else {
                                "shortcut-next"
                            },
                            if naming { "Finish" } else { "Next" },
                            4,
                            can_advance,
                        )
                        .on_click(cx.listener(
                            |dialog, _: &ClickEvent, window, cx| {
                                dialog.advance(window, cx);
                            },
                        )),
                    )
                    .child(
                        dialog_button("shortcut-cancel", "Cancel", 5, !self.saving()).on_click(
                            cx.listener(|dialog, _: &ClickEvent, window, _| dialog.cancel(window)),
                        ),
                    ),
            )
    }
}

fn dialog_button(
    id: &'static str,
    label: &'static str,
    index: isize,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .min_w(px(84.0))
        .h(px(28.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(0xc7c7c7))
        .bg(rgb(0xf7f7f7))
        .when(enabled, |button| {
            button
                .tab_index(index)
                .focus(|style| style.border_color(rgb(0x0078d4)))
                .hover(|style| style.bg(rgb(0xe5f3ff)))
                .active(|style| style.bg(rgb(0xcce4f7)))
        })
        .when(!enabled, |button| button.text_color(rgb(0x707070)))
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        RenamePaste, RenameSelectAll,
        test_support::{TempDir, test_view_entity_at_path},
    };
    use gpui::{ClipboardItem, KeyBinding, Modifiers, TestAppContext, VisualTestContext};

    fn click(cx: &mut VisualTestContext, id: &'static str) {
        let position = cx
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("missing {id}"))
            .center();
        cx.simulate_click(position, Modifiers::default());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn toolbar_shortcut_wizard_can_cancel_go_back_and_create(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        std::fs::write(temp.path().join("source.txt"), b"source").unwrap();
        cx.update(|app| {
            app.bind_keys([
                KeyBinding::new("enter", RenameCommit, Some("ExplorerRenameInput")),
                KeyBinding::new("escape", RenameCancel, Some("ExplorerRenameInput")),
            ]);
        });
        let (explorer, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        let open_wizard = |cx: &mut VisualTestContext| {
            click(cx, "utility-new");
            click(cx, "utility-new-shortcut");
            cx.read_entity(&explorer, |view, _| view.active_dialog_window.unwrap())
        };

        let cancelled = open_wizard(cx);
        VisualTestContext::from_window(cancelled, cx).simulate_keystrokes("escape");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
        assert_eq!(cx.windows().len(), 1);

        let handle = open_wizard(cx);
        let wizard = handle.downcast::<ShortcutDialog>().unwrap();
        let mut dialog_cx = VisualTestContext::from_window(handle, cx);
        dialog_cx.write_to_clipboard(ClipboardItem::new_string("missing.txt".into()));
        dialog_cx.dispatch_action(RenamePaste);
        dialog_cx.simulate_keystrokes("enter");
        wizard
            .update(&mut dialog_cx, |wizard, _, _| {
                assert!(wizard.error.is_some());
            })
            .unwrap();

        dialog_cx.dispatch_action(RenameSelectAll);
        dialog_cx.write_to_clipboard(ClipboardItem::new_string("source.txt".into()));
        dialog_cx.dispatch_action(RenamePaste);
        dialog_cx.simulate_keystrokes("enter");
        assert!(dialog_cx.debug_bounds("shortcut-finish").is_some());
        click(&mut dialog_cx, "shortcut-back");
        wizard
            .update(&mut dialog_cx, |wizard, _, cx| {
                assert_eq!(wizard.destination.read(cx).content(), "source.txt");
            })
            .unwrap();
        dialog_cx.simulate_keystrokes("enter");
        dialog_cx.write_to_clipboard(ClipboardItem::new_string("My report.txt".into()));
        dialog_cx.dispatch_action(RenamePaste);
        dialog_cx.simulate_keystrokes("enter");

        #[cfg(target_os = "windows")]
        let created = temp.path().join("My report.txt.lnk");
        #[cfg(unix)]
        let created = temp.path().join("My report.txt");
        assert!(created.is_file());
        assert_eq!(cx.windows().len(), 1);
        cx.read_entity(&explorer, |view, _| {
            assert!(view.active_dialog_window.is_none());
            assert_eq!(view.selected_paths(), vec![created]);
        });
    }
}
