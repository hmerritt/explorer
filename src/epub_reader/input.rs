//! Single-line native input, using Explorer's UTF-8/UTF-16 editing and IME state.
use super::theme::ReaderPalette;
use crate::explorer::{
    RenameBackspace, RenameBackspaceWord, RenameCancel, RenameCommit, RenameCopy, RenameCut,
    RenameDelete, RenameEnd, RenameHome, RenameLeft, RenameNoop, RenamePaste, RenameRight,
    RenameSelectAll, RenameSelectEnd, RenameSelectHome, RenameSelectLeft, RenameSelectRight,
    RenameSelectWordLeft, RenameSelectWordRight, RenameWordLeft, RenameWordRight, TextInputRedo,
    TextInputUndo, text_input::EditableTextState,
};
use gpui::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, Pixels, Point, UTF16Selection, Window, canvas, div, fill, point,
    prelude::*, px, rgb, size,
};
use std::ops::Range;

pub(super) enum InputEvent {
    Changed,
    Submit,
    Dismiss,
}
pub(super) struct ReaderInput {
    pub text: EditableTextState,
    pub light: bool,
    placeholder: String,
    focus: FocusHandle,
}
impl EventEmitter<InputEvent> for ReaderInput {}
impl Focusable for ReaderInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl ReaderInput {
    pub fn new(placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self {
            text: EditableTextState::new(String::new()),
            light: false,
            placeholder: placeholder.into(),
            focus: cx.focus_handle(),
        }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Changed);
        cx.notify();
    }
}
impl Render for ReaderInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = ReaderPalette::new(self.light);
        let entity = cx.entity();
        let paint_entity = entity.clone();
        div()
            .id("epub-input")
            .key_context("ExplorerRenameInput")
            .track_focus(&self.focus)
            .tab_index(0)
            .w_full()
            .h(px(32.0))
            .px(px(8.0))
            .py(px(5.0))
            .overflow_hidden()
            .bg(palette.control)
            .rounded(px(6.0))
            .border_1()
            .border_color(palette.border)
            .focus(|style| style.border_color(palette.accent))
            .cursor(gpui::CursorStyle::IBeam)
            .on_action(cx.listener(|input, _: &RenameBackspace, _, cx| {
                input.text.delete_backward();
                input.changed(cx)
            }))
            .on_action(cx.listener(|input, _: &RenameBackspaceWord, _, cx| {
                input.text.delete_previous_word_or_selection();
                input.changed(cx)
            }))
            .on_action(cx.listener(|input, _: &RenameDelete, _, cx| {
                input.text.delete_forward();
                input.changed(cx)
            }))
            .on_action(cx.listener(|input, _: &RenameLeft, _, cx| {
                let offset = if input.text.selected_range.is_empty() {
                    input.text.previous_boundary(input.text.cursor_offset())
                } else {
                    input.text.selected_range.start
                };
                input.text.move_to(offset);
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameRight, _, cx| {
                let offset = if input.text.selected_range.is_empty() {
                    input.text.next_boundary(input.text.cursor_offset())
                } else {
                    input.text.selected_range.end
                };
                input.text.move_to(offset);
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameWordLeft, _, cx| {
                input.text.move_to(
                    input
                        .text
                        .previous_word_boundary(input.text.cursor_offset()),
                );
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameWordRight, _, cx| {
                input
                    .text
                    .move_to(input.text.next_word_boundary(input.text.cursor_offset()));
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameSelectLeft, _, cx| {
                input
                    .text
                    .select_to(input.text.previous_boundary(input.text.cursor_offset()));
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameSelectRight, _, cx| {
                input
                    .text
                    .select_to(input.text.next_boundary(input.text.cursor_offset()));
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameSelectWordLeft, _, cx| {
                input.text.select_to(
                    input
                        .text
                        .previous_word_boundary(input.text.cursor_offset()),
                );
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameSelectWordRight, _, cx| {
                input
                    .text
                    .select_to(input.text.next_word_boundary(input.text.cursor_offset()));
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameHome, _, cx| {
                input.text.move_to(0);
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameEnd, _, cx| {
                input.text.move_to(input.text.content.len());
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameSelectHome, _, cx| {
                input.text.select_to(0);
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameSelectEnd, _, cx| {
                input.text.select_to(input.text.content.len());
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameSelectAll, _, cx| {
                input.text.select_all();
                cx.notify();
            }))
            .on_action(cx.listener(|input, _: &RenameCopy, _, cx| {
                if let Some(text) = input.text.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
                }
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|input, _: &RenameCut, _, cx| {
                if let Some(text) = input.text.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
                    input.text.replace_text_in_range_utf16(None, "");
                    input.changed(cx);
                }
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|input, _: &RenamePaste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    input
                        .text
                        .replace_text_in_range_utf16(None, &text.replace(['\r', '\n'], " "));
                    input.changed(cx);
                }
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|input, _: &TextInputUndo, _, cx| {
                input.text.undo();
                input.changed(cx);
            }))
            .on_action(cx.listener(|input, _: &TextInputRedo, _, cx| {
                input.text.redo();
                input.changed(cx);
            }))
            .on_action(cx.listener(|_, _: &RenameNoop, _, cx| cx.stop_propagation()))
            .on_action(cx.listener(|_, _: &RenameCommit, _, cx| {
                cx.emit(InputEvent::Submit);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|_, _: &RenameCancel, _, cx| {
                cx.emit(InputEvent::Dismiss);
                cx.stop_propagation();
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|input, event: &gpui::MouseDownEvent, window, cx| {
                    input.focus.focus(window);
                    let offset = input.text.index_for_mouse_position(event.position);
                    if event.click_count >= 2 {
                        input.text.select_word_at(offset);
                    } else if event.modifiers.shift {
                        input.text.select_to(offset);
                    } else {
                        input.text.move_to(offset);
                    }
                    input.text.is_selecting = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|input, event: &gpui::MouseMoveEvent, _, cx| {
                if input.text.is_selecting {
                    input
                        .text
                        .select_to(input.text.index_for_mouse_position(event.position));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|input, _, _, _| input.text.is_selecting = false),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|input, _, _, _| input.text.is_selecting = false),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let input = entity.read(cx);
                        let style = window.text_style();
                        let display = if input.text.content.is_empty() {
                            input.placeholder.clone()
                        } else {
                            input.text.content.clone()
                        };
                        let color = rgb(if input.light { 0x202020 } else { 0xe6e6e6 }).into();
                        let run = gpui::TextRun {
                            len: display.len(),
                            font: style.font(),
                            color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let runs = if input.text.content.is_empty() {
                            vec![run]
                        } else {
                            crate::explorer::text_input::editable_text_runs(
                                display.len(),
                                run,
                                &input.text.selected_range,
                                input.text.marked_range.as_ref(),
                            )
                        };
                        let line =
                            window
                                .text_system()
                                .shape_line(display.into(), px(14.0), &runs, None);
                        entity.update(cx, |input, _| {
                            input.text.update_layout(line.clone(), bounds)
                        });
                        line
                    },
                    move |bounds, line, window, cx| {
                        let input = paint_entity.read(cx);
                        let offset = input.text.scroll_offset;
                        let focus = input.focus.clone();
                        let selected = input.text.selected_range.clone();
                        let cursor = input.text.cursor_offset();
                        let content_empty = input.text.content.is_empty();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, paint_entity.clone()),
                            cx,
                        );
                        if !selected.is_empty() {
                            window.paint_quad(fill(
                                Bounds::from_corners(
                                    point(
                                        bounds.left() + line.x_for_index(selected.start) - offset,
                                        bounds.top(),
                                    ),
                                    point(
                                        bounds.left() + line.x_for_index(selected.end) - offset,
                                        bounds.bottom(),
                                    ),
                                ),
                                rgb(0x335a80),
                            ));
                        }
                        let _ = line.paint(
                            bounds.origin - point(offset, px(0.0)),
                            bounds.size.height,
                            window,
                            cx,
                        );
                        if focus.is_focused(window) && selected.is_empty() {
                            let x = if content_empty {
                                px(0.0)
                            } else {
                                line.x_for_index(cursor)
                            };
                            window.paint_quad(fill(
                                Bounds::new(
                                    bounds.origin + point(x - offset, px(0.0)),
                                    size(px(1.0), bounds.size.height),
                                ),
                                rgb(0x62a6ff),
                            ));
                        }
                    },
                )
                .size_full(),
            )
    }
}
impl EntityInputHandler for ReaderInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.text.range_from_utf16(&range);
        *actual = Some(self.text.range_to_utf16(&range));
        Some(self.text.content[range].into())
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
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.text.unmark_text();
        self.changed(cx);
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text
            .replace_text_in_range_utf16(range, &text.replace(['\r', '\n'], " "));
        self.changed(cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text.replace_and_mark_text_in_range_utf16(
            range,
            &text.replace(['\r', '\n'], " "),
            selected,
        );
        self.changed(cx);
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.text.bounds_for_range(range, bounds)
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
