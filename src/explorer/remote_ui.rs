use super::{
    remote_dialog::{open_remote_credentials_dialog, open_remote_host_key_dialog},
    remote_download::RemoteDownloadError,
    remote_fs,
    view::ExplorerView,
};
use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px, rgb};
use std::time::Duration;

impl ExplorerView {
    pub(super) fn start_remote_events(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut previous = Vec::new();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                if this
                    .update(cx, |view, cx| {
                        let snapshots = super::remote_transfer::snapshots();
                        let current: Vec<_> = snapshots
                            .iter()
                            .map(|m| (m.id, m.state, m.message.clone(), m.current))
                            .collect();
                        if current != previous {
                            let completed = snapshots.iter().any(|m| {
                                m.state == super::remote_transfer::State::Completed
                                    && !previous.iter().any(|(id, state, _, _)| {
                                        *id == m.id
                                            && *state == super::remote_transfer::State::Completed
                                    })
                            });
                            previous = current;
                            if completed {
                                view.reload_with_entry_metadata_resolution(cx);
                                view.emit_filesystem_changed(cx);
                            }
                            cx.notify();
                        }
                        if view.active_dialog_window.is_some() {
                            return;
                        }
                        if let Some((id, error)) = remote_fs::take_prompt() {
                            let result = match error {
                                RemoteDownloadError::UnknownHost(key) => {
                                    open_remote_host_key_dialog(cx.entity(), id, *key, cx)
                                }
                                RemoteDownloadError::CredentialsRequired {
                                    host,
                                    username,
                                    message,
                                } => open_remote_credentials_dialog(
                                    cx.entity(),
                                    id,
                                    host,
                                    username,
                                    message,
                                    false,
                                    cx,
                                ),
                                RemoteDownloadError::PassphraseRequired {
                                    host,
                                    username,
                                    key_path,
                                } => open_remote_credentials_dialog(
                                    cx.entity(),
                                    id,
                                    host,
                                    username,
                                    Some(format!("Unlock private key {}", key_path.display())),
                                    true,
                                    cx,
                                ),
                                RemoteDownloadError::Fatal(message) => Err(message),
                            };
                            match result {
                                Ok(handle) => view.active_dialog_window = Some(handle),
                                Err(error) => {
                                    remote_fs::reply(id, remote_fs::PromptReply::Cancel);
                                    view.set_error_notice(error);
                                }
                            }
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn start_native_transfer(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        destination: std::path::PathBuf,
        move_sources: bool,
        cx: &mut Context<Self>,
    ) {
        match super::remote_transfer::enqueue(paths, destination, move_sources) {
            Ok(_) => self.clear_operation_notice(),
            Err(error) => self.set_error_notice(error),
        }
        cx.notify();
    }

    pub(super) fn render_native_transfers(&self, cx: &mut Context<Self>) -> AnyElement {
        use super::remote_transfer::{self, State};
        let jobs = remote_transfer::snapshots();
        if jobs.is_empty() {
            return div().into_any_element();
        }
        div()
            .id("sftp-transfers")
            .max_h(px(220.0))
            .overflow_y_scroll()
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(0xdddddd))
            .bg(rgb(0xf8f8f8))
            .p(px(8.0))
            .text_size(px(12.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child("Server transfers")
                    .child(
                        div()
                            .id("sftp-full-verification")
                            .cursor_pointer()
                            .text_color(rgb(0x0067c0))
                            .child(if remote_transfer::full_verify() {
                                "Verify content for new transfers: On"
                            } else {
                                "Verify content for new transfers: Off"
                            })
                            .on_click(cx.listener(|_, _, _, cx| {
                                remote_transfer::toggle_full_verify();
                                cx.notify();
                            })),
                    ),
            )
            .children(jobs.into_iter().map(|job| {
                let id = job.id;
                let idle = matches!(
                    job.state,
                    State::Paused | State::Attention | State::Cancelled
                );
                let mut actions = Vec::new();
                if idle {
                    actions.push(("resume", "Resume"));
                } else if job.state != State::Completed {
                    actions.push(("pause", "Pause"));
                }
                if job.state == State::Attention {
                    actions.extend([
                        ("replace", "Replace all"),
                        ("skip", "Skip conflicts"),
                        ("keep", "Keep both"),
                        ("skip_item", "Skip this item"),
                    ]);
                }
                if !matches!(job.state, State::Completed | State::Cancelled) {
                    actions.push(("cancel", "Cancel"));
                }
                if matches!(job.state, State::Completed | State::Cancelled)
                    && !job.retained_partials
                {
                    actions.push(("dismiss", "Dismiss"));
                }
                if (idle || job.state == State::Completed) && job.retained_partials {
                    actions.push(("discard", "Discard partials"));
                }
                div()
                    .my(px(5.0))
                    .child(div().truncate().child(job.title()))
                    .child(div().text_color(rgb(0x555555)).child(format!(
                        "{:?} · {} · {}/{} items · {}/{} bytes",
                        job.state,
                        job.message,
                        job.current,
                        job.files(),
                        job.bytes,
                        job.total
                    )))
                    .children(
                        job.warnings
                            .into_iter()
                            .map(|warning| div().text_color(rgb(0x946200)).child(warning)),
                    )
                    .child(div().flex().gap(px(12.0)).children(actions.into_iter().map(
                        |(action, label)| {
                            div()
                                .id(gpui::SharedString::from(format!("sftp-{action}-{id}")))
                                .cursor_pointer()
                                .text_color(rgb(0x0067c0))
                                .child(label)
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    remote_transfer::control(id, action);
                                    cx.notify();
                                }))
                        },
                    )))
            }))
            .into_any_element()
    }
}
