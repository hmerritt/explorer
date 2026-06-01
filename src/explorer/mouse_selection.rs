use std::collections::BTreeSet;

use gpui::{Bounds, Pixels, Point, px, size};

use crate::explorer::{constants::ROW_HEIGHT, selection::SelectionModifiers, view::ExplorerView};

const DRAG_ACTIVATION_DISTANCE: f32 = 3.0;
const DRAG_AUTOSCROLL_MARGIN: f32 = 24.0;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MouseSelectionDrag {
    pub(super) start: Point<Pixels>,
    pub(super) current: Point<Pixels>,
    pub(super) start_scroll_top: f32,
    pub(super) current_scroll_top: f32,
    pub(super) modifiers: SelectionModifiers,
    pub(super) initial_selection: BTreeSet<usize>,
    pub(super) active: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PointerDragIntent {
    ItemDrag,
    RubberBand,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SelectionBox {
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

impl SelectionBox {
    pub(super) fn new(start_x: f32, start_y: f32, current_x: f32, current_y: f32) -> Self {
        let left = start_x.min(current_x);
        let top = start_y.min(current_y);
        Self {
            left,
            top,
            width: (start_x - current_x).abs(),
            height: (start_y - current_y).abs(),
        }
    }

    fn bottom(self) -> f32 {
        self.top + self.height
    }

    fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

impl ExplorerView {
    pub(super) fn pointer_drag_intent(
        &self,
        local_position: Point<Pixels>,
    ) -> Option<PointerDragIntent> {
        let scroll_top = self
            .scrollbar_metrics()
            .map_or(0.0, |metrics| metrics.scroll_top);
        pointer_drag_intent_at(
            f32::from(local_position.y),
            scroll_top,
            self.entries.len(),
            &self.selection.selected_indices,
        )
    }

    pub(super) fn begin_mouse_selection_drag_for_intent(
        &mut self,
        local_position: Point<Pixels>,
        modifiers: SelectionModifiers,
    ) {
        match self.pointer_drag_intent(local_position) {
            Some(PointerDragIntent::RubberBand) => {
                if !modifiers.toggle {
                    self.clear_selection();
                }
                self.begin_mouse_selection_drag(local_position, modifiers);
            }
            Some(PointerDragIntent::ItemDrag) => {
                self.cancel_mouse_selection_drag();
            }
            None => {}
        }
    }

    pub(super) fn begin_mouse_selection_drag(
        &mut self,
        local_position: Point<Pixels>,
        modifiers: SelectionModifiers,
    ) {
        let scroll_top = self
            .scrollbar_metrics()
            .map_or(0.0, |metrics| metrics.scroll_top);
        self.mouse_selection_drag = Some(MouseSelectionDrag {
            start: local_position,
            current: local_position,
            start_scroll_top: scroll_top,
            current_scroll_top: scroll_top,
            modifiers,
            initial_selection: self.selection.selected_indices.clone(),
            active: false,
        });
    }

    pub(super) fn cancel_mouse_selection_drag(&mut self) -> bool {
        self.mouse_selection_drag.take().is_some()
    }

    pub(super) fn update_mouse_selection_drag(
        &mut self,
        local_position: Point<Pixels>,
        viewport_size: gpui::Size<Pixels>,
    ) {
        let Some(mut drag) = self.mouse_selection_drag.take() else {
            return;
        };

        drag.current = local_position;
        if !drag.active && drag_distance(drag.start, drag.current) >= DRAG_ACTIVATION_DISTANCE {
            drag.active = true;
        }

        if drag.active {
            let local_y = f32::from(local_position.y);
            let viewport_height = f32::from(viewport_size.height);
            drag.current_scroll_top =
                self.autoscroll_mouse_selection_drag(local_y, viewport_height);
            self.apply_mouse_selection_drag(&drag, viewport_size);
        }

        self.mouse_selection_drag = Some(drag);
    }

    pub(super) fn end_mouse_selection_drag(&mut self) {
        if self
            .mouse_selection_drag
            .take()
            .is_some_and(|drag| drag.active)
        {
            self.suppress_next_click = true;
        }
    }

    pub(super) fn suppress_next_click(&mut self) -> bool {
        let suppress = self.suppress_next_click;
        self.suppress_next_click = false;
        suppress
    }

    pub(super) fn active_selection_box(&self) -> Option<SelectionBox> {
        let drag = self.mouse_selection_drag.as_ref()?;
        drag.active
            .then(|| visible_selection_box_for_drag(drag, drag.current_scroll_top))
    }

    fn apply_mouse_selection_drag(
        &mut self,
        drag: &MouseSelectionDrag,
        viewport_size: gpui::Size<Pixels>,
    ) {
        let viewport_width = f32::from(viewport_size.width);
        let selection_box =
            content_selection_box_for_drag(drag).clipped_horizontally(viewport_width);
        let box_indices = row_indices_intersecting_content_box(selection_box, self.entries.len());

        let selected_indices = if drag.modifiers.toggle {
            toggle_indices(drag.initial_selection.clone(), &box_indices)
        } else {
            box_indices
        };
        self.replace_selection_with_indices(selected_indices);
    }

    fn autoscroll_mouse_selection_drag(&self, local_y: f32, viewport_height: f32) -> f32 {
        let Some(metrics) = self.scrollbar_metrics() else {
            return 0.0;
        };

        let scroll_top = if local_y < DRAG_AUTOSCROLL_MARGIN {
            metrics.scroll_by(-ROW_HEIGHT)
        } else if local_y > viewport_height - DRAG_AUTOSCROLL_MARGIN {
            metrics.scroll_by(ROW_HEIGHT)
        } else {
            return metrics.scroll_top;
        };
        self.set_scroll_offset(scroll_top);
        scroll_top
    }
}

impl SelectionBox {
    fn clipped_horizontally(self, viewport_width: f32) -> Self {
        let left = self.left.clamp(0.0, viewport_width);
        let right = (self.left + self.width).clamp(0.0, viewport_width);

        Self {
            left,
            top: self.top,
            width: (right - left).max(0.0),
            height: self.height,
        }
    }

    fn translated_y(self, offset: f32) -> Self {
        Self {
            top: self.top + offset,
            ..self
        }
    }
}

pub(super) fn content_selection_box_for_drag(drag: &MouseSelectionDrag) -> SelectionBox {
    SelectionBox::new(
        f32::from(drag.start.x),
        f32::from(drag.start.y) + drag.start_scroll_top,
        f32::from(drag.current.x),
        f32::from(drag.current.y) + drag.current_scroll_top,
    )
}

pub(super) fn visible_selection_box_for_drag(
    drag: &MouseSelectionDrag,
    scroll_top: f32,
) -> SelectionBox {
    content_selection_box_for_drag(drag).translated_y(-scroll_top)
}

pub(super) fn row_indices_intersecting_content_box(
    selection_box: SelectionBox,
    entry_count: usize,
) -> BTreeSet<usize> {
    if selection_box.is_empty() || entry_count == 0 {
        return BTreeSet::new();
    }

    let first = (selection_box.top / ROW_HEIGHT).floor().max(0.0) as usize;
    let last = ((selection_box.bottom() / ROW_HEIGHT).ceil() as usize)
        .saturating_sub(1)
        .min(entry_count - 1);

    (first..=last)
        .filter(|ix| {
            let row_top = *ix as f32 * ROW_HEIGHT;
            let row_bottom = row_top + ROW_HEIGHT;
            row_top < selection_box.bottom() && row_bottom > selection_box.top
        })
        .collect()
}

pub(super) fn row_indices_intersecting_box(
    selection_box: SelectionBox,
    scroll_top: f32,
    entry_count: usize,
) -> BTreeSet<usize> {
    row_indices_intersecting_content_box(selection_box.translated_y(scroll_top), entry_count)
}

pub(super) fn pointer_drag_intent_at(
    local_y: f32,
    scroll_top: f32,
    entry_count: usize,
    selected_indices: &BTreeSet<usize>,
) -> Option<PointerDragIntent> {
    if local_y < 0.0 {
        return None;
    }

    let Some(ix) = row_index_at_content_y(local_y + scroll_top, entry_count) else {
        return Some(PointerDragIntent::RubberBand);
    };

    if selected_indices.contains(&ix) {
        Some(PointerDragIntent::ItemDrag)
    } else {
        Some(PointerDragIntent::RubberBand)
    }
}

pub(super) fn toggle_indices(
    mut selected_indices: BTreeSet<usize>,
    toggled_indices: &BTreeSet<usize>,
) -> BTreeSet<usize> {
    for ix in toggled_indices {
        if !selected_indices.remove(ix) {
            selected_indices.insert(*ix);
        }
    }
    selected_indices
}

pub(super) fn local_point(position: Point<Pixels>, bounds: &Bounds<Pixels>) -> Point<Pixels> {
    position - bounds.origin
}

pub(super) fn viewport_size(bounds: &Bounds<Pixels>) -> gpui::Size<Pixels> {
    size(bounds.size.width, bounds.size.height)
}

fn drag_distance(start: Point<Pixels>, current: Point<Pixels>) -> f32 {
    let dx = f32::from(current.x - start.x);
    let dy = f32::from(current.y - start.y);
    dx.abs().max(dy.abs())
}

fn row_index_at_content_y(content_y: f32, entry_count: usize) -> Option<usize> {
    if content_y < 0.0 || entry_count == 0 {
        return None;
    }

    let ix = (content_y / ROW_HEIGHT).floor() as usize;
    (ix < entry_count).then_some(ix)
}

pub(super) fn selection_box_bounds(selection_box: SelectionBox) -> Bounds<Pixels> {
    Bounds::new(
        gpui::point(px(selection_box.left), px(selection_box.top)),
        size(px(selection_box.width), px(selection_box.height)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::{selected_names, test_view_with_entries};

    #[test]
    fn selection_box_normalizes_reverse_drags() {
        assert_eq!(
            SelectionBox::new(80.0, 100.0, 20.0, 30.0),
            SelectionBox {
                left: 20.0,
                top: 30.0,
                width: 60.0,
                height: 70.0,
            }
        );
    }

    #[test]
    fn row_intersections_include_partially_covered_rows() {
        let indices = row_indices_intersecting_box(
            SelectionBox::new(1.0, ROW_HEIGHT - 1.0, 20.0, ROW_HEIGHT * 2.0 + 1.0),
            0.0,
            5,
        );

        assert_eq!(indices, BTreeSet::from([0, 1, 2]));
    }

    #[test]
    fn row_intersections_apply_scroll_offset() {
        let indices =
            row_indices_intersecting_box(SelectionBox::new(0.0, 0.0, 20.0, 30.0), 56.0, 5);

        assert_eq!(indices, BTreeSet::from([2, 3]));
    }

    #[test]
    fn selected_row_resolves_to_item_drag_with_scroll_offset() {
        assert_eq!(
            pointer_drag_intent_at(1.0, ROW_HEIGHT * 2.0, 5, &BTreeSet::from([2])),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn unselected_row_resolves_to_rubber_band_with_scroll_offset() {
        assert_eq!(
            pointer_drag_intent_at(1.0, ROW_HEIGHT * 2.0, 5, &BTreeSet::from([1])),
            Some(PointerDragIntent::RubberBand)
        );
    }

    #[test]
    fn whitespace_resolves_to_rubber_band() {
        assert_eq!(
            pointer_drag_intent_at(ROW_HEIGHT * 5.0, 0.0, 5, &BTreeSet::from([4])),
            Some(PointerDragIntent::RubberBand)
        );
    }

    #[test]
    fn empty_list_resolves_to_rubber_band() {
        assert_eq!(
            pointer_drag_intent_at(1.0, 0.0, 0, &BTreeSet::new()),
            Some(PointerDragIntent::RubberBand)
        );
    }

    #[test]
    fn outside_list_bounds_resolves_to_no_drag_intent() {
        assert_eq!(
            pointer_drag_intent_at(-1.0, 0.0, 5, &BTreeSet::from([0])),
            None
        );
    }

    #[test]
    fn item_drag_intent_does_not_create_mouse_selection_drag() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);

        view.begin_mouse_selection_drag_for_intent(
            gpui::point(px(1.0), px(1.0)),
            SelectionModifiers::default(),
        );

        assert!(view.mouse_selection_drag.is_none());
    }

    #[test]
    fn rubber_band_intent_creates_mouse_selection_drag() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);

        view.begin_mouse_selection_drag_for_intent(
            gpui::point(px(1.0), px(ROW_HEIGHT + 1.0)),
            SelectionModifiers::default(),
        );

        assert!(view.mouse_selection_drag.is_some());
    }

    #[test]
    fn plain_rubber_band_start_clears_existing_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);

        view.begin_mouse_selection_drag_for_intent(
            gpui::point(px(1.0), px(ROW_HEIGHT + 1.0)),
            SelectionModifiers::default(),
        );

        assert!(selected_names(&view).is_empty());
        assert_eq!(
            view.mouse_selection_drag
                .as_ref()
                .expect("rubber-band drag")
                .initial_selection,
            BTreeSet::new()
        );
    }

    #[test]
    fn ctrl_rubber_band_start_preserves_initial_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);

        view.begin_mouse_selection_drag_for_intent(
            gpui::point(px(1.0), px(ROW_HEIGHT + 1.0)),
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );

        assert_eq!(selected_names(&view), vec!["a.txt"]);
        assert_eq!(
            view.mouse_selection_drag
                .as_ref()
                .expect("rubber-band drag")
                .initial_selection,
            BTreeSet::from([0])
        );
    }

    #[test]
    fn cancel_mouse_selection_drag_removes_active_selection_box() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);

        view.begin_mouse_selection_drag_for_intent(
            gpui::point(px(1.0), px(1.0)),
            SelectionModifiers::default(),
        );
        view.update_mouse_selection_drag(
            gpui::point(px(40.0), px(ROW_HEIGHT * 2.0)),
            size(px(100.0), px(100.0)),
        );
        assert!(view.active_selection_box().is_some());

        assert!(view.cancel_mouse_selection_drag());

        assert!(view.mouse_selection_drag.is_none());
        assert!(view.active_selection_box().is_none());
    }

    #[test]
    fn cancel_mouse_selection_drag_is_no_op_without_drag() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);

        assert!(!view.cancel_mouse_selection_drag());
        assert!(view.mouse_selection_drag.is_none());
    }

    #[test]
    fn content_drag_box_keeps_start_pinned_after_scrolling() {
        let drag = MouseSelectionDrag {
            start: gpui::point(px(10.0), px(60.0)),
            current: gpui::point(px(80.0), px(40.0)),
            start_scroll_top: 0.0,
            current_scroll_top: 112.0,
            modifiers: SelectionModifiers::default(),
            initial_selection: BTreeSet::new(),
            active: true,
        };

        assert_eq!(
            content_selection_box_for_drag(&drag),
            SelectionBox {
                left: 10.0,
                top: 60.0,
                width: 70.0,
                height: 92.0,
            }
        );
        assert_eq!(
            visible_selection_box_for_drag(&drag, 112.0),
            SelectionBox {
                left: 10.0,
                top: -52.0,
                width: 70.0,
                height: 92.0,
            }
        );
    }

    #[test]
    fn content_drag_selects_rows_spanning_offscreen_scroll() {
        let selection_box = SelectionBox::new(10.0, ROW_HEIGHT, 80.0, ROW_HEIGHT * 6.0 + 4.0);

        assert_eq!(
            row_indices_intersecting_content_box(selection_box, 10),
            BTreeSet::from([1, 2, 3, 4, 5, 6])
        );
    }

    #[test]
    fn reverse_content_drag_selects_rows_above_current_viewport() {
        let drag = MouseSelectionDrag {
            start: gpui::point(px(80.0), px(70.0)),
            current: gpui::point(px(10.0), px(8.0)),
            start_scroll_top: ROW_HEIGHT * 8.0,
            current_scroll_top: ROW_HEIGHT * 4.0,
            modifiers: SelectionModifiers::default(),
            initial_selection: BTreeSet::new(),
            active: true,
        };

        let selection_box = content_selection_box_for_drag(&drag);

        assert_eq!(
            row_indices_intersecting_content_box(selection_box, 20),
            BTreeSet::from([4, 5, 6, 7, 8, 9, 10])
        );
    }

    #[test]
    fn row_intersections_ignore_empty_boxes_and_empty_lists() {
        assert!(
            row_indices_intersecting_box(SelectionBox::new(0.0, 0.0, 0.0, 30.0), 0.0, 5).is_empty()
        );
        assert!(
            row_indices_intersecting_box(SelectionBox::new(0.0, 0.0, 20.0, 30.0), 0.0, 0)
                .is_empty()
        );
    }

    #[test]
    fn toggle_indices_flips_membership_against_starting_selection() {
        assert_eq!(
            toggle_indices(BTreeSet::from([1, 3]), &BTreeSet::from([2, 3])),
            BTreeSet::from([1, 2])
        );
    }
}
