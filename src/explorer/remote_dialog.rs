use std::ops::Range;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle,
    Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable,
    GlobalElementId, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, ShapedLine, SharedString,
    Style, TextRun, TitlebarOptions, UTF16Selection, UnderlineStyle, WeakEntity, Window,
    WindowBounds, WindowDecorations, WindowKind, WindowOptions, div, fill, point, prelude::*, px,
    relative, rgb, rgba, size,
};

use crate::explorer::{
    RenameBackspace, RenameBackspaceWord, RenameCancel, RenameCommit, RenameCopy, RenameCut,
    RenameDelete, RenameEnd, RenameHome, RenameLeft, RenameNoop, RenamePaste, RenameRight,
    RenameSelectAll, RenameSelectEnd, RenameSelectHome, RenameSelectLeft, RenameSelectRight,
    RenameSelectWordLeft, RenameSelectWordRight, RenameWordLeft, RenameWordRight, TextInputRedo,
    TextInputUndo,
    remote_download::{RemoteCredentials, RemoteHostKey},
    text_input::{EditableTextEditKind, EditableTextState},
    view::ExplorerView,
};

const DIALOG_WIDTH: f32 = 430.0;
const CREDENTIALS_HEIGHT: f32 = 260.0;
const HOST_KEY_HEIGHT: f32 = 240.0;

enum RemoteDialogKind {
    Site {
        name: Entity<RemoteCredentialInput>,
        address: Entity<RemoteCredentialInput>,
        error: Option<String>,
    },
    Credentials {
        id: u64,
        host: String,
        message: Option<String>,
        passphrase: bool,
        username: Entity<RemoteCredentialInput>,
        password: Entity<RemoteCredentialInput>,
    },
    HostKey {
        id: u64,
        key: RemoteHostKey,
    },
}

struct RemoteDownloadDialog {
    kind: RemoteDialogKind,
    explorer: WeakEntity<ExplorerView>,
    focus_handle: FocusHandle,
    completed: bool,
    font: gpui::Font,
}

struct RemoteCredentialInput {
    focus_handle: FocusHandle,
    text: EditableTextState,
    placeholder: SharedString,
    password: bool,
}

pub(super) fn open_site_dialog(
    explorer: Entity<ExplorerView>,
    cx: &mut Context<ExplorerView>,
) -> Result<AnyWindowHandle, String> {
    let location = super::remote_fs::RemoteLocation::from_provider(&explorer.read(cx).path);
    let name = location
        .as_ref()
        .and_then(|loc| {
            super::remote_fs::saved_sites()
                .into_iter()
                .find(|site| site.location.site == loc.site)
        })
        .map(|site| site.name)
        .unwrap_or_default();
    let address = location
        .map(|loc| loc.address())
        .unwrap_or_else(|| "sftp://".into());
    let options = remote_window_options("Connect to SFTP server", 310.0, cx);
    cx.open_window(options, move |window, cx| {
        let name = cx.new(|cx| {
            RemoteCredentialInput::new(name, "Site name (optional)", false, cx.focus_handle())
        });
        let address = cx.new(|cx| {
            RemoteCredentialInput::new(address, "sftp://user@host/folder", false, cx.focus_handle())
        });
        address.read(cx).focus_handle.focus(window);
        cx.new(|cx| {
            cx.on_release(|dialog: &mut RemoteDownloadDialog, cx| dialog.release(cx))
                .detach();
            RemoteDownloadDialog::new(
                RemoteDialogKind::Site {
                    name,
                    address,
                    error: None,
                },
                explorer.downgrade(),
                cx.focus_handle(),
                cx,
            )
        })
    })
    .map(Into::into)
    .map_err(|e| e.to_string())
}

pub(super) fn open_remote_credentials_dialog(
    explorer: Entity<ExplorerView>,
    id: u64,
    host: String,
    username: String,
    message: Option<String>,
    passphrase: bool,
    cx: &mut Context<ExplorerView>,
) -> Result<AnyWindowHandle, String> {
    let options = remote_window_options("Sign in", CREDENTIALS_HEIGHT, cx);
    let handle = cx
        .open_window(options, move |window, cx| {
            let username_input = cx.new(|cx| {
                RemoteCredentialInput::new(username, "Username", false, cx.focus_handle())
            });
            let password_input = cx.new(|cx| {
                RemoteCredentialInput::new(
                    String::new(),
                    if passphrase { "Passphrase" } else { "Password" },
                    true,
                    cx.focus_handle(),
                )
            });
            if username_input.read(cx).text.content.is_empty() {
                username_input.read(cx).focus_handle.focus(window);
            } else {
                password_input.read(cx).focus_handle.focus(window);
            }
            let focus_handle = cx.focus_handle();
            cx.new(|cx| {
                cx.on_release(|dialog: &mut RemoteDownloadDialog, cx| dialog.release(cx))
                    .detach();
                RemoteDownloadDialog::new(
                    RemoteDialogKind::Credentials {
                        id,
                        host,
                        message,
                        passphrase,
                        username: username_input,
                        password: password_input,
                    },
                    explorer.downgrade(),
                    focus_handle,
                    cx,
                )
            })
        })
        .map_err(|error| error.to_string())?;
    Ok(handle.into())
}

pub(super) fn open_remote_host_key_dialog(
    explorer: Entity<ExplorerView>,
    id: u64,
    key: RemoteHostKey,
    cx: &mut Context<ExplorerView>,
) -> Result<AnyWindowHandle, String> {
    let options = remote_window_options("Unknown host", HOST_KEY_HEIGHT, cx);
    let handle = cx
        .open_window(options, move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            cx.new(|cx| {
                cx.on_release(|dialog: &mut RemoteDownloadDialog, cx| dialog.release(cx))
                    .detach();
                RemoteDownloadDialog::new(
                    RemoteDialogKind::HostKey { id, key },
                    explorer.downgrade(),
                    focus_handle,
                    cx,
                )
            })
        })
        .map_err(|error| error.to_string())?;
    Ok(handle.into())
}

fn remote_window_options(title: &'static str, height: f32, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(
            size(px(DIALOG_WIDTH), px(height)),
            cx,
        )),
        window_min_size: Some(size(px(DIALOG_WIDTH), px(height))),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(title)),
            ..Default::default()
        }),
        kind: WindowKind::Floating,
        is_movable: true,
        is_resizable: false,
        is_minimizable: true,
        window_decorations: Some(WindowDecorations::Server),
        ..Default::default()
    }
}

impl RemoteDownloadDialog {
    fn new(
        kind: RemoteDialogKind,
        explorer: WeakEntity<ExplorerView>,
        focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let dialog = Self {
            kind,
            explorer,
            focus_handle,
            completed: false,
            font: crate::settings::current_app_font(cx),
        };
        cx.observe_global::<crate::settings::SettingsState>(|this, cx| {
            this.font = crate::settings::current_app_font(cx);
            cx.notify();
        })
        .detach();
        dialog
    }

    fn submit(&mut self, _: &RenameCommit, window: &mut Window, cx: &mut Context<Self>) {
        match &self.kind {
            RemoteDialogKind::Site { name, address, .. } => {
                let result =
                    super::remote_fs::RemoteLocation::parse(address.read(cx).text.content.trim())
                        .and_then(|location| {
                            super::remote_fs::update_site(
                                location.clone(),
                                name.read(cx).text.content.clone(),
                            )?;
                            Ok(location)
                        });
                let location = match result {
                    Ok(location) => location,
                    Err(message) => {
                        if let RemoteDialogKind::Site { error, .. } = &mut self.kind {
                            *error = Some(message);
                        }
                        cx.notify();
                        return;
                    }
                };
                self.completed = true;
                let _ = self.explorer.update(cx, |explorer, cx| {
                    explorer.active_dialog_window = None;
                    explorer.navigate_to_directory_with_watcher(
                        location.provider_path(),
                        super::navigation::HistoryMode::Record,
                        cx,
                    );
                    cx.notify();
                });
            }
            RemoteDialogKind::Credentials {
                id,
                username,
                password,
                passphrase,
                ..
            } => {
                let username_input = username.clone();
                let username = username_input.read(cx).text.content.trim().to_owned();
                let password = password.read(cx).text.content.clone();
                if username.is_empty() {
                    username_input.read(cx).focus_handle.focus(window);
                    return;
                }
                self.completed = true;
                let _ = self.explorer.update(cx, |explorer, cx| {
                    explorer.submit_remote_credentials(
                        *id,
                        RemoteCredentials {
                            username,
                            password,
                            passphrase: *passphrase,
                        },
                        cx,
                    );
                    cx.notify();
                });
            }
            RemoteDialogKind::HostKey { id, key } => {
                self.completed = true;
                let key = key.clone();
                let _ = self.explorer.update(cx, |explorer, cx| {
                    explorer.confirm_remote_host_key(*id, key, cx);
                    cx.notify();
                });
            }
        }
        window.remove_window();
        cx.stop_propagation();
    }

    fn cancel(&mut self, _: &RenameCancel, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_inner(window, cx);
        cx.stop_propagation();
    }

    fn cancel_inner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.completed = true;
        let id = match &self.kind {
            RemoteDialogKind::Site { .. } => {
                self.clear_site_dialog(cx);
                window.remove_window();
                return;
            }
            RemoteDialogKind::Credentials { id, .. } | RemoteDialogKind::HostKey { id, .. } => *id,
        };
        let _ = self.explorer.update(cx, |explorer, cx| {
            explorer.cancel_remote_prompt(id, cx);
            cx.notify();
        });
        window.remove_window();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != "tab" {
            return;
        }
        let (username, password) = match &self.kind {
            RemoteDialogKind::Credentials {
                username, password, ..
            } => (username, password),
            RemoteDialogKind::Site { name, address, .. } => (name, address),
            _ => return,
        };
        if username.read(cx).focus_handle.is_focused(window) {
            password.read(cx).focus_handle.focus(window);
        } else {
            username.read(cx).focus_handle.focus(window);
        }
        cx.stop_propagation();
    }

    fn release(&mut self, cx: &mut App) {
        if self.completed {
            return;
        }
        let id = match &self.kind {
            RemoteDialogKind::Site { .. } => {
                self.clear_site_dialog(cx);
                return;
            }
            RemoteDialogKind::Credentials { id, .. } | RemoteDialogKind::HostKey { id, .. } => *id,
        };
        let _ = self.explorer.update(cx, |explorer, cx| {
            explorer.cancel_remote_prompt(id, cx);
            cx.notify();
        });
    }

    fn clear_site_dialog(&self, cx: &mut App) {
        let _ = self.explorer.update(cx, |explorer, cx| {
            explorer.active_dialog_window = None;
            cx.notify();
        });
    }

    fn render_site(
        &self,
        name: Entity<RemoteCredentialInput>,
        address: Entity<RemoteCredentialInput>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().text_size(px(16.0)).child("Connect to SFTP server"))
            .child("Use a server address or an alias from your SSH config.")
            .child("Site name")
            .child(name)
            .child("Address")
            .child(address)
            .when_some(error, |this, error| {
                this.child(div().text_color(rgb(0x9b1c1c)).child(error))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .mt(px(10.0))
                    .child(
                        remote_button("site-forget", "Forget site").on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                if let RemoteDialogKind::Site { address, error, .. } =
                                    &mut this.kind
                                {
                                    let result = super::remote_fs::RemoteLocation::parse(
                                        address.read(cx).text.content.trim(),
                                    )
                                    .and_then(|loc| super::remote_fs::forget_site(&loc.site));
                                    if let Err(message) = result {
                                        *error = Some(message);
                                        cx.notify();
                                        return;
                                    }
                                }
                                let _ = this.explorer.update(cx, |view, cx| {
                                    view.reload_with_entry_metadata_resolution(cx);
                                });
                                this.cancel_inner(window, cx);
                            },
                        )),
                    )
                    .child(
                        remote_button("site-connect", "Connect").on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.submit(&RenameCommit, window, cx)
                            },
                        )),
                    )
                    .child(remote_button("site-cancel", "Cancel").on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| this.cancel_inner(window, cx),
                    ))),
            )
            .into_any_element()
    }

    fn render_credentials(
        &self,
        host: &str,
        message: Option<&str>,
        username: Entity<RemoteCredentialInput>,
        password: Entity<RemoteCredentialInput>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(16.0))
                    .child(format!("Sign in to {host}")),
            )
            .child("Explorer could not authenticate automatically. Enter credentials to continue.")
            .when_some(message.map(str::to_owned), |this, message| {
                this.child(div().text_color(rgb(0x9b1c1c)).child(message))
            })
            .child(div().mt(px(4.0)).child("Username"))
            .child(username)
            .child(
                if matches!(
                    self.kind,
                    RemoteDialogKind::Credentials {
                        passphrase: true,
                        ..
                    }
                ) {
                    "Key passphrase"
                } else {
                    "Password"
                },
            )
            .child(password)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(10.0))
                    .mt(px(10.0))
                    .child(
                        remote_button("remote-sign-in", "Sign in").on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.submit(&RenameCommit, window, cx);
                            },
                        )),
                    )
                    .child(
                        remote_button("remote-cancel", "Cancel").on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.cancel_inner(window, cx);
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn render_host_key(&self, key: &RemoteHostKey, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(div().text_size(px(16.0)).child("Confirm the server identity"))
            .child(format!(
                "Explorer has not connected to {}:{} before. Confirm this fingerprint with the server administrator.",
                key.host, key.port
            ))
            .child(div().child(format!("Key type: {}", key.algorithm)))
            .child(
                div()
                    .font_family("monospace")
                    .text_size(px(11.0))
                    .child(key.fingerprint.clone()),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(10.0))
                    .mt(px(14.0))
                    .child(remote_button("remote-connect", "Connect").on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| {
                            this.submit(&RenameCommit, window, cx);
                        },
                    )))
                    .child(remote_button("remote-host-cancel", "Cancel").on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| {
                            this.cancel_inner(window, cx);
                        },
                    ))),
            )
            .into_any_element()
    }
}

impl Render for RemoteDownloadDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.kind {
            RemoteDialogKind::Site {
                name,
                address,
                error,
            } => self.render_site(name.clone(), address.clone(), error.clone(), cx),
            RemoteDialogKind::Credentials {
                host,
                message,
                username,
                password,
                ..
            } => self.render_credentials(
                host,
                message.as_deref(),
                username.clone(),
                password.clone(),
                cx,
            ),
            RemoteDialogKind::HostKey { key, .. } => self.render_host_key(key, cx),
        };
        div()
            .font(self.font.clone())
            .key_context("ExplorerRenameInput")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(rgb(0xffffff))
            .p(px(20.0))
            .text_size(px(12.0))
            .text_color(rgb(0x202020))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .child(content)
    }
}

impl Focusable for RemoteDownloadDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn remote_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .min_w(px(84.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(0xc7c7c7))
        .bg(rgb(0xf7f7f7))
        .hover(|style| style.bg(rgb(0xe5f3ff)))
        .active(|style| style.bg(rgb(0xcce4f7)))
        .child(label)
}

fn credential_display_text(
    content: &str,
    placeholder: SharedString,
    password: bool,
) -> SharedString {
    if content.is_empty() {
        placeholder
    } else if password {
        SharedString::from("*".repeat(content.len()))
    } else {
        SharedString::from(content.to_owned())
    }
}

impl RemoteCredentialInput {
    fn new(
        content: String,
        placeholder: &'static str,
        password: bool,
        focus_handle: FocusHandle,
    ) -> Self {
        Self {
            focus_handle,
            text: EditableTextState::new(content),
            placeholder: SharedString::from(placeholder),
            password,
        }
    }

    fn left(&mut self, _: &RenameLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.text.selected_range.is_empty() {
            self.text
                .move_to(self.text.previous_boundary(self.text.cursor_offset()));
        } else {
            self.text.move_to(self.text.selected_range.start);
        }
        cx.notify();
    }

    fn right(&mut self, _: &RenameRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.text.selected_range.is_empty() {
            self.text
                .move_to(self.text.next_boundary(self.text.cursor_offset()));
        } else {
            self.text.move_to(self.text.selected_range.end);
        }
        cx.notify();
    }

    fn word_left(&mut self, _: &RenameWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.text
            .move_to(self.text.previous_word_boundary(self.text.cursor_offset()));
        cx.notify();
    }

    fn word_right(&mut self, _: &RenameWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.text
            .move_to(self.text.next_word_boundary(self.text.cursor_offset()));
        cx.notify();
    }

    fn select_left(&mut self, _: &RenameSelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.text
            .select_to(self.text.previous_boundary(self.text.cursor_offset()));
        cx.notify();
    }

    fn select_right(&mut self, _: &RenameSelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.text
            .select_to(self.text.next_boundary(self.text.cursor_offset()));
        cx.notify();
    }

    fn select_word_left(
        &mut self,
        _: &RenameSelectWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text
            .select_to(self.text.previous_word_boundary(self.text.cursor_offset()));
        cx.notify();
    }

    fn select_word_right(
        &mut self,
        _: &RenameSelectWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text
            .select_to(self.text.next_word_boundary(self.text.cursor_offset()));
        cx.notify();
    }

    fn home(&mut self, _: &RenameHome, _: &mut Window, cx: &mut Context<Self>) {
        self.text.move_to(0);
        cx.notify();
    }

    fn end(&mut self, _: &RenameEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.text.move_to(self.text.content.len());
        cx.notify();
    }

    fn select_home(&mut self, _: &RenameSelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.text.select_to(0);
        cx.notify();
    }

    fn select_end(&mut self, _: &RenameSelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.text.select_to(self.text.content.len());
        cx.notify();
    }

    fn select_all(&mut self, _: &RenameSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.text.select_all();
        cx.notify();
    }

    fn backspace(&mut self, _: &RenameBackspace, _: &mut Window, cx: &mut Context<Self>) {
        self.text.delete_backward();
        cx.notify();
    }

    fn backspace_word(&mut self, _: &RenameBackspaceWord, _: &mut Window, cx: &mut Context<Self>) {
        self.text.delete_previous_word_or_selection();
        cx.notify();
    }

    fn delete(&mut self, _: &RenameDelete, _: &mut Window, cx: &mut Context<Self>) {
        self.text.delete_forward();
        cx.notify();
    }

    fn copy(&mut self, _: &RenameCopy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.password && !self.text.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text.content[self.text.selected_range.clone()].to_owned(),
            ));
        }
        cx.stop_propagation();
    }

    fn cut(&mut self, _: &RenameCut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.password && !self.text.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text.content[self.text.selected_range.clone()].to_owned(),
            ));
            self.text
                .replace_text_with_kind(None, "", EditableTextEditKind::Cut);
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn paste(&mut self, _: &RenamePaste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.text.replace_text_with_kind(
                None,
                &text.replace(['\r', '\n'], " "),
                EditableTextEditKind::Paste,
            );
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn undo(&mut self, _: &TextInputUndo, _: &mut Window, cx: &mut Context<Self>) {
        self.text.undo();
        cx.notify();
    }

    fn redo(&mut self, _: &TextInputRedo, _: &mut Window, cx: &mut Context<Self>) {
        self.text.redo();
        cx.notify();
    }

    fn noop(&mut self, _: &RenameNoop, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window);
        self.text.is_selecting = true;
        let offset = self.text.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.text.select_to(offset);
        } else {
            self.text.move_to(offset);
        }
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.text.is_selecting {
            let offset = self.text.index_for_mouse_position(event.position);
            self.text.select_to(offset);
            cx.notify();
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.text.is_selecting = false;
    }
}

impl Render for RemoteCredentialInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("ExplorerRenameInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::backspace_word))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::noop))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .w_full()
            .h(px(28.0))
            .border_1()
            .border_color(rgb(0xa0a0a0))
            .bg(rgb(0xffffff))
            .overflow_hidden()
            .px(px(6.0))
            .py(px(4.0))
            .child(RemoteTextElement { input: cx.entity() })
    }
}

impl Focusable for RemoteCredentialInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for RemoteCredentialInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.text.range_from_utf16(&range_utf16);
        actual_range.replace(self.text.range_to_utf16(&range));
        Some(self.text.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(self.text.selected_text_range_utf16())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.text.marked_text_range_utf16()
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.text.unmark_text();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text
            .replace_text_in_range_utf16(range_utf16, &text.replace(['\r', '\n'], " "));
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text.replace_and_mark_text_in_range_utf16(
            range_utf16,
            &new_text.replace(['\r', '\n'], " "),
            new_selected_range_utf16,
        );
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.text.bounds_for_range(range_utf16, bounds)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.text.character_index_for_point(point)
    }
}

struct RemoteTextElement {
    input: Entity<RemoteCredentialInput>,
}

struct RemoteTextPrepaint {
    line: ShapedLine,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_offset: Pixels,
}

impl IntoElement for RemoteTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RemoteTextElement {
    type RequestLayoutState = ();
    type PrepaintState = RemoteTextPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let scroll_offset = input.text.scroll_offset;
        let is_placeholder = input.text.content.is_empty();
        let display = credential_display_text(
            &input.text.content,
            input.placeholder.clone(),
            input.password,
        );
        let style = window.text_style();
        let base_run = TextRun {
            len: display.len(),
            font: style.font(),
            color: if is_placeholder {
                rgb(0x777777).into()
            } else {
                style.color
            },
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if !is_placeholder {
            if let Some(marked) = input.text.marked_range.as_ref() {
                vec![
                    TextRun {
                        len: marked.start,
                        ..base_run.clone()
                    },
                    TextRun {
                        len: marked.end - marked.start,
                        underline: Some(UnderlineStyle {
                            color: Some(base_run.color),
                            thickness: px(1.0),
                            wavy: false,
                        }),
                        ..base_run.clone()
                    },
                    TextRun {
                        len: display.len() - marked.end,
                        ..base_run
                    },
                ]
                .into_iter()
                .filter(|run| run.len > 0)
                .collect()
            } else {
                vec![base_run]
            }
        } else {
            vec![base_run]
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display, font_size, &runs, None);
        let cursor_offset = if is_placeholder {
            0
        } else {
            input.text.cursor_offset()
        };
        let selection = (!is_placeholder && !input.text.selected_range.is_empty()).then(|| {
            fill(
                Bounds::from_corners(
                    point(
                        bounds.left() + line.x_for_index(input.text.selected_range.start)
                            - scroll_offset,
                        bounds.top(),
                    ),
                    point(
                        bounds.left() + line.x_for_index(input.text.selected_range.end)
                            - scroll_offset,
                        bounds.bottom(),
                    ),
                ),
                rgba(0x0078d744),
            )
        });
        let cursor = selection.is_none().then(|| {
            fill(
                Bounds::new(
                    point(
                        bounds.left() + line.x_for_index(cursor_offset) - scroll_offset,
                        bounds.top(),
                    ),
                    size(px(1.0), bounds.size.height),
                ),
                rgb(0x202020),
            )
        });
        RemoteTextPrepaint {
            line,
            cursor,
            selection,
            scroll_offset,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        prepaint
            .line
            .paint(
                point(bounds.origin.x - prepaint.scroll_offset, bounds.origin.y),
                window.line_height(),
                window,
                cx,
            )
            .ok();
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        let line = prepaint.line.clone();
        self.input.update(cx, |input, _| {
            input.text.update_layout(line, bounds);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_display_never_contains_the_password() {
        let display = credential_display_text(
            "correct horse battery staple",
            SharedString::from("Password"),
            true,
        );
        assert_eq!(display.len(), "correct horse battery staple".len());
        assert!(display.chars().all(|character| character == '*'));
    }
}
