use super::{
    remote_dialog::{open_remote_credentials_dialog, open_remote_host_key_dialog},
    remote_download::RemoteDownloadError,
    remote_fs,
    remote_transfer::{self, JobSnapshot, State},
    tooltip::explorer_tooltip,
    view::ExplorerView,
};
use crate::settings::SettingsState;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, Context, FontWeight, IntoElement,
    SharedString, Window, div, prelude::*, px, relative, rgb,
};
use std::time::Duration;

const TRANSFER_UI_UPDATE_INTERVAL: Duration = Duration::from_millis(500);
const TRANSFER_PANEL_MAX_HEIGHT: f32 = 260.0;
const TRANSFER_TOOLBAR_HEIGHT: f32 = 36.0;
const TRANSFER_HEADER_HEIGHT: f32 = 28.0;
const TRANSFER_ROW_HEIGHT: f32 = super::constants::ROW_HEIGHT;
const TRANSFER_NAME_MIN_WIDTH: f32 = 220.0;
const TRANSFER_PROGRESS_WIDTH: f32 = 280.0;
const TRANSFER_SPEED_WIDTH: f32 = 112.0;
const TRANSFER_REMAINING_WIDTH: f32 = 104.0;
const TRANSFER_ACTIONS_WIDTH: f32 = 104.0;
const TRANSFER_TABLE_MIN_WIDTH: f32 = TRANSFER_NAME_MIN_WIDTH
    + TRANSFER_PROGRESS_WIDTH
    + TRANSFER_SPEED_WIDTH
    + TRANSFER_REMAINING_WIDTH
    + TRANSFER_ACTIONS_WIDTH;
const TRANSFER_ACTION_BUTTON_SIZE: f32 = 22.0;
const TRANSFER_BORDER: u32 = 0xe7e7e7;
const TRANSFER_BORDER_SOFT: u32 = 0xf0f0f0;
const TRANSFER_SURFACE: u32 = 0xffffff;
const TRANSFER_SURFACE_MUTED: u32 = 0xf8f8f8;
const TRANSFER_SURFACE_HOVER: u32 = 0xf3f3f3;
const TRANSFER_ROW_HOVER: u32 = 0xfafafa;
const TRANSFER_TEXT_PRIMARY: u32 = 0x1f1f1f;
const TRANSFER_TEXT_SECONDARY: u32 = 0x595959;
const TRANSFER_TEXT_TERTIARY: u32 = 0x767676;
const TRANSFER_BLUE: u32 = 0x0067c0;
const TRANSFER_BLUE_HOVER: u32 = 0xe5f3ff;
const TRANSFER_GREEN: u32 = 0x36a646;
const TRANSFER_GREEN_TRACK: u32 = 0xe1f3e4;
const TRANSFER_AMBER: u32 = 0x946200;
const TRANSFER_AMBER_TRACK: u32 = 0xffedc2;
const TRANSFER_AMBER_SURFACE: u32 = 0xfff8e7;
const TRANSFER_AMBER_BORDER: u32 = 0xe2c06f;
const TRANSFER_AMBER_TEXT: u32 = 0x6f4b00;
const TRANSFER_DANGER: u32 = 0xb42318;
const TRANSFER_DANGER_HOVER: u32 = 0xffe8e6;
const TRANSFER_NEUTRAL_TRACK: u32 = 0xe5e5e5;
const TRANSFER_COLLAPSE_DOWN: &str = "\u{E70D}";
const TRANSFER_COLLAPSE_RIGHT: &str = "\u{E76C}";
const TRANSFER_ACTION_PAUSE: &str = "\u{E769}";
const TRANSFER_ACTION_RESUME: &str = "\u{E768}";
const TRANSFER_ACTION_CANCEL: &str = "\u{E711}";
const TRANSFER_ACTION_DISMISS: &str = "\u{E73E}";
const TRANSFER_ACTION_DISCARD: &str = "\u{E74D}";

impl ExplorerView {
    pub(super) fn start_remote_events(&mut self, cx: &mut Context<Self>) {
        self.remote_transfer_snapshots = super::remote_transfer::snapshots();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(TRANSFER_UI_UPDATE_INTERVAL)
                    .await;
                if this
                    .update(cx, |view, cx| {
                        let snapshots = super::remote_transfer::snapshots();
                        if snapshots != view.remote_transfer_snapshots {
                            if transfer_panel_should_expand(
                                &view.remote_transfer_snapshots,
                                &snapshots,
                            ) {
                                view.remote_transfer_panel_collapsed = false;
                            }
                            view.remote_transfer_snapshots = snapshots;
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

        cx.spawn(async move |this, cx| {
            let mut completion = super::remote_transfer::completion_revision();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                if this
                    .update(cx, |view, cx| {
                        let revision = super::remote_transfer::completion_revision();
                        if revision != completion {
                            completion = revision;
                            view.reload_with_entry_metadata_resolution(cx);
                            view.emit_filesystem_changed(cx);
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
        let sftp = cx.global::<SettingsState>().value.sftp;
        match super::remote_transfer::enqueue(paths, destination, move_sources, sftp) {
            Ok(_) => {
                self.remote_transfer_panel_collapsed = false;
                self.clear_operation_notice();
            }
            Err(error) => self.set_error_notice(error),
        }
        cx.notify();
    }

    pub(super) fn render_native_transfers(&self, cx: &mut Context<Self>) -> AnyElement {
        render_transfer_panel(
            self.remote_transfer_snapshots.clone(),
            self.remote_transfer_panel_collapsed,
            cx.listener(|view, _: &ClickEvent, _, cx| {
                view.remote_transfer_panel_collapsed = !view.remote_transfer_panel_collapsed;
                cx.stop_propagation();
                cx.notify();
            }),
            cx,
        )
    }
}

fn transfer_panel_should_expand(previous: &[JobSnapshot], next: &[JobSnapshot]) -> bool {
    next.iter().any(|job| {
        previous
            .iter()
            .find(|previous_job| previous_job.id == job.id)
            .map_or(true, |previous_job| {
                previous_job.state != State::Attention && job.state == State::Attention
            })
    })
}

fn render_transfer_panel<V: 'static>(
    jobs: Vec<JobSnapshot>,
    collapsed: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &mut Context<V>,
) -> AnyElement {
    if jobs.is_empty() {
        return div().into_any_element();
    }
    let sftp = cx
        .try_global::<SettingsState>()
        .map(|settings| settings.value.sftp)
        .unwrap_or_default();
    let attention_count = jobs
        .iter()
        .filter(|job| job.state == State::Attention)
        .count();

    div()
        .id("sftp-transfers")
        .debug_selector(|| "sftp-transfers".to_owned())
        .flex()
        .flex_col()
        .w_full()
        .max_h(px(TRANSFER_PANEL_MAX_HEIGHT))
        .overflow_hidden()
        .flex_shrink_0()
        .border_t_1()
        .border_color(rgb(TRANSFER_BORDER))
        .bg(rgb(TRANSFER_SURFACE))
        .text_size(px(12.0))
        .text_color(rgb(TRANSFER_TEXT_PRIMARY))
        .child(render_transfer_toolbar(
            jobs.len(),
            attention_count,
            collapsed,
            on_toggle,
        ))
        .when(!collapsed, |panel| {
            panel.child(
                div()
                    .id("sftp-transfer-table-scroll")
                    .debug_selector(|| "sftp-transfer-table-scroll".to_owned())
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_x_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .h_full()
                            .min_w(px(TRANSFER_TABLE_MIN_WIDTH))
                            .child(render_transfer_table_header())
                            .child(
                                div()
                                    .id("sftp-transfer-rows")
                                    .debug_selector(|| "sftp-transfer-rows".to_owned())
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .overflow_y_scroll()
                                    .children(
                                        jobs.into_iter()
                                            .map(|job| render_transfer_job(job, sftp, cx)),
                                    ),
                            ),
                    ),
            )
        })
        .into_any_element()
}

fn render_transfer_toolbar(
    job_count: usize,
    attention_count: usize,
    collapsed: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let chevron = if collapsed {
        TRANSFER_COLLAPSE_RIGHT
    } else {
        TRANSFER_COLLAPSE_DOWN
    };
    let tooltip = if collapsed {
        "Expand server transfers"
    } else {
        "Collapse server transfers"
    };

    div()
        .id("sftp-transfer-toggle")
        .debug_selector(|| "sftp-transfer-toggle".to_owned())
        .flex()
        .flex_row()
        .items_center()
        .h(px(TRANSFER_TOOLBAR_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .px(px(12.0))
        .gap(px(8.0))
        .bg(rgb(TRANSFER_SURFACE_MUTED))
        .border_b_1()
        .border_color(rgb(TRANSFER_BORDER))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(TRANSFER_SURFACE_HOVER)))
        .active(|style| style.opacity(0.78))
        .tooltip(explorer_tooltip(tooltip))
        .on_click(on_toggle)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.0))
                .h(px(16.0))
                .font(super::icons::nav_icon_font())
                .text_size(px(9.0))
                .text_color(rgb(TRANSFER_TEXT_SECONDARY))
                .child(chevron),
        )
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(13.0))
                .child("Server transfers"),
        )
        .child(
            div()
                .px(px(7.0))
                .py(px(2.0))
                .rounded(px(8.0))
                .bg(rgb(TRANSFER_NEUTRAL_TRACK))
                .text_size(px(11.0))
                .text_color(rgb(TRANSFER_TEXT_SECONDARY))
                .child(transfer_count_label(job_count)),
        )
        .child(div().flex_1())
        .when(attention_count > 0, |toolbar| {
            toolbar.child(
                div()
                    .id("sftp-transfer-attention-count")
                    .debug_selector(|| "sftp-transfer-attention-count".to_owned())
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(2.0))
                    .bg(rgb(TRANSFER_AMBER_TRACK))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TRANSFER_AMBER))
                    .child(attention_count_label(attention_count)),
            )
        })
        .into_any_element()
}

fn render_transfer_table_header() -> AnyElement {
    div()
        .id("sftp-transfer-columns")
        .debug_selector(|| "sftp-transfer-columns".to_owned())
        .flex()
        .flex_row()
        .h(px(TRANSFER_HEADER_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .bg(rgb(TRANSFER_SURFACE))
        .border_b_1()
        .border_color(rgb(TRANSFER_BORDER_SOFT))
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(TRANSFER_TEXT_TERTIARY))
        .child(transfer_header_cell("name", "Name", None, true))
        .child(transfer_header_cell(
            "progress",
            "Progress",
            Some(TRANSFER_PROGRESS_WIDTH),
            true,
        ))
        .child(transfer_header_cell(
            "speed",
            "Speed",
            Some(TRANSFER_SPEED_WIDTH),
            true,
        ))
        .child(transfer_header_cell(
            "remaining",
            "Remaining",
            Some(TRANSFER_REMAINING_WIDTH),
            true,
        ))
        .child(transfer_header_cell(
            "actions",
            "Actions",
            Some(TRANSFER_ACTIONS_WIDTH),
            false,
        ))
        .into_any_element()
}

fn transfer_header_cell(
    key: &'static str,
    label: &'static str,
    width: Option<f32>,
    separator: bool,
) -> AnyElement {
    let cell = div()
        .id(SharedString::from(format!("sftp-transfer-column-{key}")))
        .debug_selector(move || format!("sftp-transfer-column-{key}"))
        .flex()
        .items_center()
        .h_full()
        .min_w(px(0.0))
        .when(key == "name", |cell| cell.pl(px(36.0)).pr(px(12.0)))
        .when(key != "name", |cell| cell.px(px(12.0)))
        .when(separator, |cell| {
            cell.border_r_1().border_color(rgb(TRANSFER_BORDER_SOFT))
        })
        .child(label);
    if let Some(width) = width {
        cell.w(px(width)).flex_shrink_0().into_any_element()
    } else {
        cell.flex_1()
            .min_w(px(TRANSFER_NAME_MIN_WIDTH))
            .into_any_element()
    }
}

fn render_transfer_job<V: 'static>(
    job: JobSnapshot,
    sftp: crate::settings::SftpSettings,
    cx: &mut Context<V>,
) -> AnyElement {
    let has_detail = job.state == State::Attention || !job.warnings.is_empty();
    let detail_job = has_detail.then(|| job.clone());

    div()
        .id(SharedString::from(format!("sftp-transfer-job-{}", job.id)))
        .debug_selector({
            let id = job.id;
            move || format!("sftp-transfer-job-{id}")
        })
        .flex()
        .flex_col()
        .w_full()
        .border_b_1()
        .border_color(rgb(TRANSFER_BORDER_SOFT))
        .child(render_transfer_row(job, sftp, cx))
        .when_some(detail_job, |item, job| {
            item.child(render_transfer_detail_band(job, sftp, cx))
        })
        .into_any_element()
}

fn render_transfer_row<V: 'static>(
    job: JobSnapshot,
    sftp: crate::settings::SftpSettings,
    cx: &mut Context<V>,
) -> AnyElement {
    let id = job.id;
    let title = job.title();
    let file_count = job.files();
    let tooltip = transfer_name_tooltip(&title, &job.message);
    let icon = if job.current_is_directory {
        super::icons::folder_icon().into_any_element()
    } else {
        super::icons::file_icon_for_path(std::path::Path::new(&title)).into_any_element()
    };

    div()
        .id(SharedString::from(format!("sftp-transfer-row-{id}")))
        .debug_selector(move || format!("sftp-transfer-row-{id}"))
        .flex()
        .flex_row()
        .items_center()
        .h(px(TRANSFER_ROW_HEIGHT))
        .w_full()
        .bg(rgb(TRANSFER_SURFACE))
        .hover(|style| style.bg(rgb(TRANSFER_ROW_HOVER)))
        .child(
            div()
                .id(("sftp-filename", id))
                .debug_selector(move || format!("sftp-filename-{id}"))
                .flex()
                .items_center()
                .flex_1()
                .min_w(px(TRANSFER_NAME_MIN_WIDTH))
                .h_full()
                .pl(px(16.0))
                .pr(px(12.0))
                .gap(px(8.0))
                .tooltip(explorer_tooltip(tooltip))
                .child(
                    div()
                        .id(("sftp-transfer-icon", id))
                        .debug_selector(move || format!("sftp-transfer-icon-{id}"))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(super::constants::FILE_ICON_SLOT_WIDTH))
                        .h(px(super::constants::FILE_ICON_SLOT_HEIGHT))
                        .flex_shrink_0()
                        .child(icon),
                )
                .child(div().flex_1().min_w(px(0.0)).truncate().child(title))
                .when(file_count > 1, |name| {
                    name.child(
                        div()
                            .ml(px(8.0))
                            .flex_shrink_0()
                            .text_size(px(11.0))
                            .text_color(rgb(TRANSFER_TEXT_TERTIARY))
                            .child(format!("{file_count} items")),
                    )
                }),
        )
        .child(render_transfer_progress_cell(&job))
        .child(render_transfer_speed_cell(&job))
        .child(
            div()
                .id(SharedString::from(format!("sftp-transfer-remaining-{id}")))
                .debug_selector(move || format!("sftp-transfer-remaining-{id}"))
                .flex()
                .items_center()
                .w(px(TRANSFER_REMAINING_WIDTH))
                .h_full()
                .flex_shrink_0()
                .px(px(12.0))
                .text_color(rgb(TRANSFER_TEXT_SECONDARY))
                .child(transfer_remaining_text(&job)),
        )
        .child(render_transfer_actions(&job, sftp, cx))
        .into_any_element()
}

fn render_transfer_speed_cell(job: &JobSnapshot) -> AnyElement {
    let id = job.id;
    div()
        .id(SharedString::from(format!("sftp-transfer-speed-{id}")))
        .debug_selector(move || format!("sftp-transfer-speed-{id}"))
        .flex()
        .items_center()
        .w(px(TRANSFER_SPEED_WIDTH))
        .h_full()
        .flex_shrink_0()
        .px(px(12.0))
        .text_color(rgb(TRANSFER_TEXT_SECONDARY))
        .child(transfer_speed_text(job))
        .into_any_element()
}

fn render_transfer_progress_cell(job: &JobSnapshot) -> AnyElement {
    let id = job.id;
    let labels = transfer_progress_labels(job);

    div()
        .id(SharedString::from(format!("sftp-transfer-progress-{id}")))
        .debug_selector(move || format!("sftp-transfer-progress-{id}"))
        .flex()
        .items_center()
        .w(px(TRANSFER_PROGRESS_WIDTH))
        .h_full()
        .flex_shrink_0()
        .px(px(12.0))
        .gap(px(10.0))
        .child(transfer_progress_bar(job))
        .child(
            div()
                .w(px(34.0))
                .flex_shrink_0()
                .text_color(rgb(TRANSFER_TEXT_PRIMARY))
                .child(labels.primary),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(11.0))
                .text_color(rgb(TRANSFER_TEXT_TERTIARY))
                .child(labels.secondary),
        )
        .into_any_element()
}

fn transfer_progress_bar(job: &JobSnapshot) -> AnyElement {
    let (color, track) = transfer_progress_colors(job.state);
    let bar = div()
        .id(SharedString::from(format!(
            "sftp-transfer-progress-bar-{}",
            job.id
        )))
        .debug_selector({
            let id = job.id;
            move || format!("sftp-transfer-progress-bar-{id}")
        })
        .relative()
        .w(px(92.0))
        .h(px(4.0))
        .flex_shrink_0()
        .overflow_hidden()
        .rounded(px(2.0))
        .bg(rgb(track));

    if let Some(percentage) = job.percentage {
        bar.child(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(relative(f32::from(percentage) / 100.0))
                .bg(rgb(color)),
        )
        .into_any_element()
    } else {
        let id = job.id;
        bar.child(
            div()
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .bg(rgb(color))
                .with_animation(
                    ("sftp-transfer-indeterminate", id),
                    Animation::new(Duration::from_millis(1_400)).repeat(),
                    |segment, delta| {
                        let left = -0.30 + (1.30 * delta);
                        segment.left(relative(left)).w(relative(0.30))
                    },
                ),
        )
        .into_any_element()
    }
}

fn render_transfer_actions<V: 'static>(
    job: &JobSnapshot,
    sftp: crate::settings::SftpSettings,
    cx: &mut Context<V>,
) -> AnyElement {
    let id = job.id;
    div()
        .id(SharedString::from(format!("sftp-transfer-actions-{id}")))
        .debug_selector(move || format!("sftp-transfer-actions-{id}"))
        .flex()
        .items_center()
        .justify_end()
        .w(px(TRANSFER_ACTIONS_WIDTH))
        .h_full()
        .flex_shrink_0()
        .px(px(8.0))
        .gap(px(4.0))
        .children(
            transfer_row_actions(job)
                .into_iter()
                .map(|action| render_transfer_action_button(id, action, sftp, cx)),
        )
        .into_any_element()
}

fn render_transfer_action_button<V: 'static>(
    id: u64,
    action: TransferAction,
    sftp: crate::settings::SftpSettings,
    cx: &mut Context<V>,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("sftp-{}-{id}", action.key)))
        .debug_selector(move || format!("sftp-{}-{id}", action.key))
        .flex()
        .items_center()
        .justify_center()
        .w(px(TRANSFER_ACTION_BUTTON_SIZE))
        .h(px(TRANSFER_ACTION_BUTTON_SIZE))
        .flex_shrink_0()
        .rounded(px(2.0))
        .cursor_pointer()
        .font(super::icons::nav_icon_font())
        .text_size(px(12.0))
        .text_color(rgb(if action.destructive {
            TRANSFER_DANGER
        } else {
            TRANSFER_BLUE
        }))
        .hover(move |style| {
            style.bg(rgb(if action.destructive {
                TRANSFER_DANGER_HOVER
            } else {
                TRANSFER_BLUE_HOVER
            }))
        })
        .active(|style| style.opacity(0.72))
        .tooltip(explorer_tooltip(action.label))
        .child(action.glyph)
        .on_click(cx.listener(move |_, _, _, cx| {
            remote_transfer::control(id, action.key, sftp);
            cx.stop_propagation();
            cx.notify();
        }))
        .into_any_element()
}

fn render_transfer_detail_band<V: 'static>(
    job: JobSnapshot,
    sftp: crate::settings::SftpSettings,
    cx: &mut Context<V>,
) -> AnyElement {
    let id = job.id;
    div()
        .id(SharedString::from(format!("sftp-transfer-detail-{id}")))
        .debug_selector(move || format!("sftp-transfer-detail-{id}"))
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_h(px(36.0))
        .px(px(12.0))
        .py(px(6.0))
        .gap(px(12.0))
        .bg(rgb(TRANSFER_AMBER_SURFACE))
        .border_t_1()
        .border_color(rgb(TRANSFER_AMBER_TRACK))
        .text_size(px(11.0))
        .text_color(rgb(TRANSFER_AMBER))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .when(job.state == State::Attention, |details| {
                    details.child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(job.message.clone()),
                    )
                })
                .children(job.warnings.into_iter().map(|warning| div().child(warning))),
        )
        .when(job.state == State::Attention, |details| {
            details.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap(px(6.0))
                    .children(
                        conflict_actions()
                            .into_iter()
                            .map(|action| render_conflict_action_button(id, action, sftp, cx)),
                    ),
            )
        })
        .into_any_element()
}

fn render_conflict_action_button<V: 'static>(
    id: u64,
    action: TransferAction,
    sftp: crate::settings::SftpSettings,
    cx: &mut Context<V>,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("sftp-{}-{id}", action.key)))
        .debug_selector(move || format!("sftp-{}-{id}", action.key))
        .flex()
        .items_center()
        .justify_center()
        .h(px(26.0))
        .px(px(9.0))
        .flex_shrink_0()
        .rounded(px(2.0))
        .border_1()
        .border_color(rgb(TRANSFER_AMBER_BORDER))
        .bg(rgb(TRANSFER_SURFACE))
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(TRANSFER_AMBER_TEXT))
        .hover(|style| style.bg(rgb(TRANSFER_AMBER_TRACK)))
        .active(|style| style.opacity(0.72))
        .child(action.label)
        .on_click(cx.listener(move |_, _, _, cx| {
            remote_transfer::control(id, action.key, sftp);
            cx.stop_propagation();
            cx.notify();
        }))
        .into_any_element()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TransferAction {
    key: &'static str,
    label: &'static str,
    glyph: &'static str,
    destructive: bool,
}

fn transfer_row_actions(job: &JobSnapshot) -> Vec<TransferAction> {
    let idle = matches!(
        job.state,
        State::Paused | State::Attention | State::Cancelled
    );
    let mut actions = Vec::with_capacity(3);
    if idle {
        actions.push(TransferAction {
            key: "resume",
            label: "Resume",
            glyph: TRANSFER_ACTION_RESUME,
            destructive: false,
        });
    } else if job.state != State::Completed {
        actions.push(TransferAction {
            key: "pause",
            label: "Pause",
            glyph: TRANSFER_ACTION_PAUSE,
            destructive: false,
        });
    }
    if !matches!(job.state, State::Completed | State::Cancelled) {
        actions.push(TransferAction {
            key: "cancel",
            label: "Cancel",
            glyph: TRANSFER_ACTION_CANCEL,
            destructive: true,
        });
    }
    if matches!(job.state, State::Completed | State::Cancelled) && !job.retained_partials {
        actions.push(TransferAction {
            key: "dismiss",
            label: "Dismiss",
            glyph: TRANSFER_ACTION_DISMISS,
            destructive: false,
        });
    }
    if (idle || job.state == State::Completed) && job.retained_partials {
        actions.push(TransferAction {
            key: "discard",
            label: "Discard partials",
            glyph: TRANSFER_ACTION_DISCARD,
            destructive: true,
        });
    }
    actions
}

fn conflict_actions() -> [TransferAction; 4] {
    [
        TransferAction {
            key: "replace",
            label: "Replace all",
            glyph: "",
            destructive: false,
        },
        TransferAction {
            key: "skip",
            label: "Skip conflicts",
            glyph: "",
            destructive: false,
        },
        TransferAction {
            key: "keep",
            label: "Keep both",
            glyph: "",
            destructive: false,
        },
        TransferAction {
            key: "skip_item",
            label: "Skip this item",
            glyph: "",
            destructive: false,
        },
    ]
}

fn transfer_progress_colors(state: State) -> (u32, u32) {
    match state {
        State::Attention => (TRANSFER_AMBER, TRANSFER_AMBER_TRACK),
        State::Paused | State::Cancelled => (TRANSFER_TEXT_TERTIARY, TRANSFER_NEUTRAL_TRACK),
        _ => (TRANSFER_GREEN, TRANSFER_GREEN_TRACK),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TransferProgressLabels {
    primary: String,
    secondary: String,
}

fn transfer_progress_labels(job: &JobSnapshot) -> TransferProgressLabels {
    match job.percentage {
        Some(percentage) => TransferProgressLabels {
            primary: format!("{percentage}%"),
            secondary: transfer_size_pair(job.bytes, job.total),
        },
        None => TransferProgressLabels {
            primary: String::new(),
            secondary: "Preparing".to_owned(),
        },
    }
}

fn transfer_size_pair(bytes: u64, total: u64) -> String {
    use super::formatting::format_size_parts;
    let (bytes_value, bytes_unit) = format_size_parts(bytes);
    let (total_value, total_unit) = format_size_parts(total);
    if bytes_unit == total_unit {
        format!("{bytes_value} / {total_value} {total_unit}")
    } else {
        format!("{bytes_value} {bytes_unit} / {total_value} {total_unit}")
    }
}

fn transfer_speed_text(job: &JobSnapshot) -> String {
    use super::formatting::format_size_parts;
    if job.state != State::Transferring {
        return "~".to_owned();
    }
    let Some(speed) = job.speed.filter(|speed| speed.is_finite() && *speed >= 0.0) else {
        return "~".to_owned();
    };
    let (value, unit) = format_size_parts(speed.round() as u64);
    format!("{value} {unit}/s")
}

fn transfer_name_tooltip(title: &str, message: &str) -> String {
    let message = message.trim();
    if message.is_empty() || message == title {
        title.to_owned()
    } else {
        format!("{title}\n{message}")
    }
}

fn transfer_remaining_text(job: &JobSnapshot) -> String {
    use super::formatting::format_transfer_remaining;
    if job.state != State::Transferring {
        return "~".to_owned();
    }
    job.remaining
        .map(format_transfer_remaining)
        .unwrap_or_else(|| "~".to_owned())
}

fn transfer_count_label(count: usize) -> String {
    format!(
        "{count} {}",
        if count == 1 { "transfer" } else { "transfers" }
    )
}

fn attention_count_label(count: usize) -> String {
    format!(
        "{count} need{} attention",
        if count == 1 { "s" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::super::remote_transfer::{JobSnapshot, State};
    use super::*;

    #[test]
    fn transfer_ui_updates_are_limited_to_twice_per_second() {
        assert_eq!(TRANSFER_UI_UPDATE_INTERVAL, Duration::from_millis(500));
    }

    struct Panel {
        jobs: Vec<JobSnapshot>,
        collapsed: bool,
    }

    impl gpui::Render for Panel {
        fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div()
                    .debug_selector(|| "transfer-space".into())
                    .w_full()
                    .child(render_transfer_panel(
                        self.jobs.clone(),
                        self.collapsed,
                        cx.listener(|panel, _: &ClickEvent, _, cx| {
                            panel.collapsed = !panel.collapsed;
                            cx.notify();
                        }),
                        cx,
                    )),
            )
        }
    }

    #[test]
    fn transfer_progress_remaining_and_speed_copy_is_compact_and_separator_free() {
        let mut job = JobSnapshot::for_test(State::Transferring);
        let labels = transfer_progress_labels(&job);
        assert_eq!(labels.primary, "50%");
        assert_eq!(labels.secondary, "512 bytes / 1.0 KB");
        assert_eq!(transfer_speed_text(&job), "512 bytes/s");
        for forbidden in ["...", "…", "·"] {
            assert!(!labels.primary.contains(forbidden));
            assert!(!labels.secondary.contains(forbidden));
            assert!(!transfer_speed_text(&job).contains(forbidden));
        }
        assert_eq!(transfer_remaining_text(&job), "1s");

        job.bytes = 2 * super::super::constants::MB_BYTES;
        job.total = 10 * super::super::constants::MB_BYTES;
        assert_eq!(transfer_progress_labels(&job).secondary, "2.00 / 10.00 MB");

        job.remaining = None;
        assert_eq!(transfer_remaining_text(&job), "~");
        job.state = State::Paused;
        assert_eq!(transfer_remaining_text(&job), "~");
        assert_eq!(transfer_speed_text(&job), "~");

        job.percentage = None;
        assert_eq!(
            transfer_progress_labels(&job),
            TransferProgressLabels {
                primary: String::new(),
                secondary: "Preparing".to_owned(),
            }
        );
    }

    #[test]
    fn transfer_speed_uses_file_size_units_and_handles_unavailable_samples() {
        use super::super::constants::{GB_BYTES, KB_BYTES, MB_BYTES, TB_BYTES};

        let mut job = JobSnapshot::for_test(State::Transferring);
        for (speed, expected) in [
            (0.0, "0 bytes/s"),
            (512.0, "512 bytes/s"),
            ((KB_BYTES + KB_BYTES / 2) as f64, "1.5 KB/s"),
            ((MB_BYTES + 512 * KB_BYTES) as f64, "1.50 MB/s"),
            ((GB_BYTES + 512 * MB_BYTES) as f64, "1.50 GB/s"),
            ((TB_BYTES + 512 * GB_BYTES) as f64, "1.50 TB/s"),
        ] {
            job.speed = Some(speed);
            assert_eq!(transfer_speed_text(&job), expected);
        }

        job.speed = None;
        assert_eq!(transfer_speed_text(&job), "~");
        job.speed = Some(f64::NAN);
        assert_eq!(transfer_speed_text(&job), "~");
        job.speed = Some(-1.0);
        assert_eq!(transfer_speed_text(&job), "~");
        job.state = State::Verifying;
        job.speed = Some(MB_BYTES as f64);
        assert_eq!(transfer_speed_text(&job), "~");
    }

    #[test]
    fn transfer_name_tooltip_keeps_the_file_name_and_state_message() {
        assert_eq!(
            transfer_name_tooltip("report.zip", "Reconnecting"),
            "report.zip\nReconnecting"
        );
        assert_eq!(transfer_name_tooltip("report.zip", ""), "report.zip");
    }

    #[test]
    fn transfer_action_model_preserves_existing_controls() {
        let action_keys = |job: &JobSnapshot| {
            transfer_row_actions(job)
                .into_iter()
                .map(|action| action.key)
                .collect::<Vec<_>>()
        };
        let mut job = JobSnapshot::for_test(State::Transferring);
        assert_eq!(action_keys(&job), vec!["pause", "cancel"]);

        job.state = State::Attention;
        assert_eq!(action_keys(&job), vec!["resume", "cancel"]);
        assert_eq!(
            conflict_actions().map(|action| action.key),
            ["replace", "skip", "keep", "skip_item"]
        );

        job.state = State::Cancelled;
        assert_eq!(action_keys(&job), vec!["resume", "dismiss"]);
        job.retained_partials = true;
        assert_eq!(action_keys(&job), vec!["resume", "discard"]);
    }

    #[test]
    fn transfer_counts_are_named_without_punctuation_separators() {
        assert_eq!(transfer_count_label(1), "1 transfer");
        assert_eq!(transfer_count_label(3), "3 transfers");
        assert_eq!(attention_count_label(1), "1 needs attention");
        assert_eq!(attention_count_label(2), "2 need attention");
    }

    #[test]
    fn transfer_panel_auto_expands_for_new_jobs_and_attention_transitions() {
        let active = JobSnapshot::for_test(State::Transferring);
        assert!(transfer_panel_should_expand(
            &[],
            std::slice::from_ref(&active)
        ));
        assert!(!transfer_panel_should_expand(
            std::slice::from_ref(&active),
            std::slice::from_ref(&active)
        ));

        let mut attention = active.clone();
        attention.state = State::Attention;
        assert!(transfer_panel_should_expand(
            std::slice::from_ref(&active),
            std::slice::from_ref(&attention)
        ));
        assert!(!transfer_panel_should_expand(
            std::slice::from_ref(&attention),
            std::slice::from_ref(&attention)
        ));
        assert!(!transfer_panel_should_expand(&[active], &[]));
    }

    #[gpui::test]
    fn transfer_panel_renders_details_table_and_attention_band_at_small_width(
        cx: &mut gpui::TestAppContext,
    ) {
        let (panel, cx) = cx.add_window_view(|_, _| Panel {
            jobs: vec![JobSnapshot::for_test(State::Attention)],
            collapsed: false,
        });
        cx.simulate_resize(gpui::size(px(320.0), px(240.0)));
        cx.run_until_parked();
        let bounds = cx.debug_bounds("sftp-transfers").expect("transfer panel");
        assert!(bounds.size.width <= px(320.0));
        assert!(cx.debug_bounds("sftp-filename-123").is_some());
        assert!(cx.debug_bounds("sftp-transfer-icon-123").is_some());
        assert_eq!(
            cx.debug_bounds("sftp-transfer-row-123").unwrap().size.height,
            px(super::super::constants::ROW_HEIGHT)
        );
        assert!(cx.debug_bounds("sftp-transfer-columns").is_some());
        for selector in [
            "sftp-transfer-column-name",
            "sftp-transfer-column-progress",
            "sftp-transfer-column-speed",
            "sftp-transfer-column-remaining",
            "sftp-transfer-column-actions",
        ] {
            assert!(cx.debug_bounds(selector).is_some());
        }
        assert!(cx.debug_bounds("sftp-transfer-column-status").is_none());
        assert!(cx.debug_bounds("sftp-transfer-status-123").is_none());
        assert!(cx.debug_bounds("sftp-transfer-progress-123").is_some());
        assert!(cx.debug_bounds("sftp-transfer-speed-123").is_some());
        assert!(cx.debug_bounds("sftp-transfer-detail-123").is_some());
        assert!(cx.debug_bounds("sftp-replace-123").is_some());
        assert!(cx.debug_bounds("sftp-transfer-table-scroll").is_some());
        assert!(cx.debug_bounds("transfer-space").unwrap().size.height > px(0.0));

        let toggle = cx
            .debug_bounds("sftp-transfer-toggle")
            .expect("transfer tray toggle")
            .center();
        cx.simulate_mouse_down(toggle, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(toggle, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.read_entity(&panel, |panel, _| assert!(panel.collapsed));
        assert_eq!(
            cx.debug_bounds("sftp-transfers").unwrap().size.height,
            px(TRANSFER_TOOLBAR_HEIGHT + 1.0)
        );

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

    #[gpui::test]
    fn expanded_transfer_rows_scroll_within_the_capped_tray(cx: &mut gpui::TestAppContext) {
        let jobs = (0..12)
            .map(|id| {
                let mut job = JobSnapshot::for_test(State::Transferring);
                job.id = id;
                job
            })
            .collect();
        let (_, cx) = cx.add_window_view(|_, _| Panel {
            jobs,
            collapsed: false,
        });
        cx.simulate_resize(gpui::size(px(900.0), px(700.0)));
        cx.run_until_parked();

        let panel = cx.debug_bounds("sftp-transfers").unwrap();
        let rows = cx.debug_bounds("sftp-transfer-rows").unwrap();
        assert!(panel.size.height <= px(TRANSFER_PANEL_MAX_HEIGHT + 1.0));
        assert!(rows.size.height < px(12.0 * TRANSFER_ROW_HEIGHT));
    }

    #[gpui::test]
    fn collapsed_transfer_panel_renders_only_the_toolbar(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Panel {
            jobs: vec![JobSnapshot::for_test(State::Transferring)],
            collapsed: true,
        });
        cx.run_until_parked();

        assert!(cx.debug_bounds("sftp-transfer-toggle").is_some());
        assert!(cx.debug_bounds("sftp-transfer-columns").is_none());
        assert_eq!(
            cx.debug_bounds("sftp-transfers").unwrap().size.height,
            px(TRANSFER_TOOLBAR_HEIGHT + 1.0)
        );
    }
}
