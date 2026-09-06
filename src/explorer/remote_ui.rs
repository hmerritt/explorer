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
            let mut completion = super::remote_transfer::completion_revision();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                if this
                    .update(cx, |view, cx| {
                        let snapshots = super::remote_transfer::snapshots();
                        let current: Vec<_> = snapshots
                            .iter()
                            .map(|m| {
                                (
                                    m.id,
                                    m.state,
                                    m.message.clone(),
                                    m.current,
                                    m.bytes,
                                    m.percentage,
                                    m.speed.map(f64::to_bits),
                                    m.remaining.map(|d| d.as_secs()),
                                )
                            })
                            .collect();
                        let revision = super::remote_transfer::completion_revision();
                        if revision != completion {
                            completion = revision;
                            view.reload_with_entry_metadata_resolution(cx);
                            view.emit_filesystem_changed(cx);
                        }
                        if current != previous {
                            previous = current;
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
        render_transfer_panel(super::remote_transfer::snapshots(), cx)
    }
}

fn render_transfer_panel<V: 'static>(
    jobs: Vec<super::remote_transfer::JobSnapshot>,
    cx: &mut Context<V>,
) -> AnyElement {
    use super::remote_transfer::{self, State};
    if jobs.is_empty() {
        return div().into_any_element();
    }
    div()
        .id("sftp-transfers")
        .debug_selector(|| "sftp-transfers".to_owned())
        .w_full()
        .max_h(px(220.0))
        .overflow_y_scroll()
        .flex_shrink_0()
        .border_t_1()
        .border_color(rgb(0xdddddd))
        .bg(rgb(0xf8f8f8))
        .p(px(8.0))
        .text_size(px(12.0))
        .child("Server transfers")
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
            if matches!(job.state, State::Completed | State::Cancelled) && !job.retained_partials {
                actions.push(("dismiss", "Dismiss"));
            }
            if (idle || job.state == State::Completed) && job.retained_partials {
                actions.push(("discard", "Discard partials"));
            }
            div()
                .my(px(5.0))
                .child(
                    div()
                        .id(("sftp-filename", id))
                        .debug_selector(move || format!("sftp-filename-{id}"))
                        .truncate()
                        .child(job.title()),
                )
                .child(
                    div()
                        .text_color(rgb(0x555555))
                        .child(transfer_statistics(&job)),
                )
                .child(div().text_color(rgb(0x555555)).child(job.message.clone()))
                .children(
                    job.warnings
                        .into_iter()
                        .map(|warning| div().text_color(rgb(0x946200)).child(warning)),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(12.0))
                        .children(actions.into_iter().map(|(action, label)| {
                            div()
                                .id(gpui::SharedString::from(format!("sftp-{action}-{id}")))
                                .cursor_pointer()
                                .text_color(rgb(0x0067c0))
                                .child(label)
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    remote_transfer::control(id, action);
                                    cx.notify();
                                }))
                        })),
                )
        }))
        .into_any_element()
}

fn transfer_statistics(job: &super::remote_transfer::JobSnapshot) -> String {
    use super::formatting::{format_size, format_transfer_remaining};
    use super::remote_transfer::State;
    let percentage = job
        .percentage
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "Preparing…".into());
    let mut text = format!(
        "{percentage} · {} / {} · {}/{} items",
        format_size(Some(job.bytes)),
        format_size(Some(job.total)),
        job.current,
        job.files()
    );
    if job.files() > 1 && job.current_file_total > 0 {
        let percent = (job.current_file_bytes as f64 / job.current_file_total as f64 * 100.0)
            .clamp(0.0, 100.0) as u8;
        text.push_str(&format!(" · current file {percent}%"));
    }
    if job.state == State::Transferring {
        let speed = job.speed.unwrap_or(0.0).max(0.0).round() as u64;
        text.push_str(&format!(
            " · {}/s · {}",
            format_size(Some(speed)),
            job.remaining
                .map(format_transfer_remaining)
                .map(|s| format!("{s} remaining"))
                .unwrap_or_else(|| "Estimating…".into())
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::super::remote_transfer::{JobSnapshot, State};
    use super::*;
    struct Panel {
        jobs: Vec<JobSnapshot>,
    }
    impl gpui::Render for Panel {
        fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div()
                    .debug_selector(|| "transfer-space".into())
                    .w_full()
                    .child(render_transfer_panel(self.jobs.clone(), cx)),
            )
        }
    }
    #[test]
    fn remote_statistics_format_units_speed_and_remaining_time() {
        let mut job = JobSnapshot::for_test(State::Transferring);
        let stats = transfer_statistics(&job);
        assert!(stats.contains("50%"));
        assert!(stats.contains("512 bytes / 1.0 KB"));
        assert!(stats.contains("512 bytes/s"));
        assert!(stats.contains("1s remaining"));
        for (speed, unit) in [
            (1024.0, "1.0 KB/s"),
            (1048576.0, "1.00 MB/s"),
            (1073741824.0, "1.00 GB/s"),
        ] {
            job.speed = Some(speed);
            assert!(transfer_statistics(&job).contains(unit));
        }
        job.speed = None;
        job.remaining = None;
        assert!(transfer_statistics(&job).contains("Estimating…"));
        job.state = State::Paused;
        let stats = transfer_statistics(&job);
        assert!(!stats.contains("/s"));
        assert!(!stats.contains("remaining"));
    }
    #[gpui::test]
    fn remote_panel_renders_at_small_width_and_disappears_when_empty(
        cx: &mut gpui::TestAppContext,
    ) {
        let (panel, cx) = cx.add_window_view(|_, _| Panel {
            jobs: vec![JobSnapshot::for_test(State::Attention)],
        });
        cx.simulate_resize(gpui::size(px(320.0), px(240.0)));
        cx.run_until_parked();
        let bounds = cx.debug_bounds("sftp-transfers").expect("transfer panel");
        assert!(bounds.size.width <= px(320.0));
        assert!(cx.debug_bounds("sftp-filename-123").is_some());
        assert!(cx.debug_bounds("sftp-full-verification").is_none());
        assert!(cx.debug_bounds("transfer-space").unwrap().size.height > px(0.0));
        panel.update(cx, |panel, cx| {
            panel.jobs.clear();
            cx.notify();
        });
        cx.run_until_parked();
        // GPUI retains old debug selectors across frames; measure reclaimed layout space.
        assert_eq!(
            cx.debug_bounds("transfer-space").unwrap().size.height,
            px(0.0)
        );
    }
}
