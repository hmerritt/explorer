use std::collections::BTreeSet;

use gpui::{Bounds, MouseButton, Pixels, Point, px, size};

use crate::{
    explorer::{
        constants::{
            FILE_ICON_SLOT_WIDTH, LARGE_ICON_TILE_HEIGHT, LARGE_ICON_TILE_WIDTH,
            SCROLLBAR_GUTTER_WIDTH,
        },
        large_icons::LargeIconLayout,
        selection::SelectionModifiers,
        view::ExplorerView,
    },
    settings::FileViewMode,
};

const DRAG_ACTIVATION_DISTANCE: f32 = 3.0;
const DRAG_AUTOSCROLL_MARGIN: f32 = 24.0;
const DETAILS_NAME_CELL_LEFT_PADDING: f32 = 16.0;
const DETAILS_NAME_ICON_TEXT_GAP: f32 = 8.0;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MouseSelectionDrag {
    pub(super) button: MouseButton,
    pub(super) start: Point<Pixels>,
    pub(super) current: Point<Pixels>,
    pub(super) start_scroll_top: f32,
    pub(super) current_scroll_top: f32,
    pub(super) modifiers: SelectionModifiers,
    pub(super) initial_selection: BTreeSet<usize>,
    pub(super) visible: bool,
    pub(super) active: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PointerDragIntent {
    ItemDrag,
    RubberBand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MouseSelectionPointerDownOutcome {
    pub(super) menu_closed: bool,
    pub(super) selection_started: bool,
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
    pub(super) fn pointer_drag_intent_with_details_name_hit_targets(
        &self,
        local_position: Point<Pixels>,
        viewport_size: gpui::Size<Pixels>,
        details_name_item_hit_rights: &[f32],
    ) -> Option<PointerDragIntent> {
        let scroll_top = self
            .scrollbar_metrics()
            .map_or(0.0, |metrics| metrics.scroll_top);
        let viewport_width = f32::from(viewport_size.width);
        if self.view_mode == FileViewMode::LargeIcons {
            return large_icon_pointer_drag_intent_at(
                f32::from(local_position.x),
                f32::from(local_position.y),
                scroll_top,
                viewport_width,
                self.entries.len(),
                self.large_icon_layout.as_ref(),
            );
        }

        let name_column_width =
            self.effective_name_column_width(viewport_width + SCROLLBAR_GUTTER_WIDTH);
        pointer_drag_intent_at_with_offsets_and_name_hits(
            f32::from(local_position.x),
            f32::from(local_position.y),
            scroll_top,
            self.visible_horizontal_scroll_offset(),
            viewport_width,
            name_column_width,
            self.entries.len(),
            &self.selection.selected_indices,
            self.entry_row_height(),
            details_name_item_hit_rights,
        )
    }

    #[cfg(test)]
    pub(super) fn begin_mouse_selection_drag_for_intent(
        &mut self,
        button: MouseButton,
        local_position: Point<Pixels>,
        viewport_size: gpui::Size<Pixels>,
        modifiers: SelectionModifiers,
    ) -> bool {
        self.begin_mouse_selection_drag_for_intent_with_details_name_hit_targets(
            button,
            local_position,
            viewport_size,
            modifiers,
            &[],
        )
    }

    pub(super) fn begin_mouse_selection_drag_for_intent_with_details_name_hit_targets(
        &mut self,
        button: MouseButton,
        local_position: Point<Pixels>,
        viewport_size: gpui::Size<Pixels>,
        modifiers: SelectionModifiers,
        details_name_item_hit_rights: &[f32],
    ) -> bool {
        match self.pointer_drag_intent_with_details_name_hit_targets(
            local_position,
            viewport_size,
            details_name_item_hit_rights,
        ) {
            Some(PointerDragIntent::RubberBand) => {
                if !modifiers.toggle && !modifiers.extend {
                    self.clear_selection();
                }
                self.begin_mouse_selection_drag(button, local_position, modifiers);
                true
            }
            Some(PointerDragIntent::ItemDrag) => {
                self.cancel_mouse_selection_drag();
                false
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub(super) fn begin_mouse_selection_drag_after_menu_dismissal(
        &mut self,
        button: MouseButton,
        local_position: Point<Pixels>,
        viewport_size: gpui::Size<Pixels>,
        modifiers: SelectionModifiers,
    ) -> MouseSelectionPointerDownOutcome {
        let menu_closed = self.close_context_menu();
        let selection_started = self.begin_mouse_selection_drag_for_intent(
            button,
            local_position,
            viewport_size,
            modifiers,
        );
        MouseSelectionPointerDownOutcome {
            menu_closed,
            selection_started,
        }
    }

    pub(super) fn begin_mouse_selection_drag_after_menu_dismissal_with_details_name_hit_targets(
        &mut self,
        button: MouseButton,
        local_position: Point<Pixels>,
        viewport_size: gpui::Size<Pixels>,
        modifiers: SelectionModifiers,
        details_name_item_hit_rights: &[f32],
    ) -> MouseSelectionPointerDownOutcome {
        let menu_closed = self.close_context_menu();
        let selection_started = self
            .begin_mouse_selection_drag_for_intent_with_details_name_hit_targets(
                button,
                local_position,
                viewport_size,
                modifiers,
                details_name_item_hit_rights,
            );
        MouseSelectionPointerDownOutcome {
            menu_closed,
            selection_started,
        }
    }

    pub(super) fn begin_mouse_selection_drag(
        &mut self,
        button: MouseButton,
        local_position: Point<Pixels>,
        modifiers: SelectionModifiers,
    ) {
        self.mouse_down_entry_selection = None;
        self.hovered_entry_path = None;
        let scroll_top = self
            .scrollbar_metrics()
            .map_or(0.0, |metrics| metrics.scroll_top);
        self.mouse_selection_drag = Some(MouseSelectionDrag {
            button,
            start: local_position,
            current: local_position,
            start_scroll_top: scroll_top,
            current_scroll_top: scroll_top,
            modifiers,
            initial_selection: self.selection.selected_indices.clone(),
            visible: button == MouseButton::Right,
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
            drag.visible = true;
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

    pub(super) fn end_mouse_selection_drag(&mut self, button: MouseButton) -> bool {
        let Some(drag) = self.mouse_selection_drag.take() else {
            return false;
        };
        if drag.button != button {
            self.mouse_selection_drag = Some(drag);
            return false;
        }

        if drag.active && button == MouseButton::Left {
            self.suppress_next_click = true;
        }
        drag.active
    }

    pub(super) fn suppress_next_click(&mut self) -> bool {
        let suppress = self.suppress_next_click;
        self.suppress_next_click = false;
        suppress
    }

    pub(super) fn visible_mouse_selection_box(&self) -> Option<SelectionBox> {
        let drag = self.mouse_selection_drag.as_ref()?;
        drag.visible
            .then(|| visible_selection_box_for_drag(drag, drag.current_scroll_top).for_render())
    }

    fn apply_mouse_selection_drag(
        &mut self,
        drag: &MouseSelectionDrag,
        viewport_size: gpui::Size<Pixels>,
    ) {
        let viewport_width = f32::from(viewport_size.width);
        let selection_box =
            content_selection_box_for_drag(drag).clipped_horizontally(viewport_width);
        let box_indices = if self.view_mode == FileViewMode::LargeIcons {
            large_icon_grid_indices_intersecting_content_box(
                selection_box,
                viewport_width,
                self.entries.len(),
                self.large_icon_layout.as_ref(),
            )
        } else {
            row_indices_intersecting_content_box_with_row_height(
                selection_box,
                self.entries.len(),
                self.entry_row_height(),
            )
        };

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
            metrics.scroll_by(-self.entry_row_height())
        } else if local_y > viewport_height - DRAG_AUTOSCROLL_MARGIN {
            metrics.scroll_by(self.entry_row_height())
        } else {
            return metrics.scroll_top;
        };
        self.set_scroll_offset(scroll_top);
        scroll_top
    }
}

impl SelectionBox {
    fn for_render(self) -> Self {
        const MINIMUM_SIZE: f32 = 2.0;

        Self {
            width: self.width.max(MINIMUM_SIZE),
            height: self.height.max(MINIMUM_SIZE),
            ..self
        }
    }

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

#[cfg(test)]
pub(super) fn row_indices_intersecting_content_box(
    selection_box: SelectionBox,
    entry_count: usize,
) -> BTreeSet<usize> {
    row_indices_intersecting_content_box_with_row_height(
        selection_box,
        entry_count,
        crate::explorer::constants::ROW_HEIGHT,
    )
}

pub(super) fn row_indices_intersecting_content_box_with_row_height(
    selection_box: SelectionBox,
    entry_count: usize,
    row_height: f32,
) -> BTreeSet<usize> {
    if selection_box.is_empty() || entry_count == 0 {
        return BTreeSet::new();
    }

    let first = (selection_box.top / row_height).floor().max(0.0) as usize;
    let last = ((selection_box.bottom() / row_height).ceil() as usize)
        .saturating_sub(1)
        .min(entry_count - 1);

    (first..=last)
        .filter(|ix| {
            let row_top = *ix as f32 * row_height;
            let row_bottom = row_top + row_height;
            row_top < selection_box.bottom() && row_bottom > selection_box.top
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LargeIconGridMetrics {
    pub(super) columns: usize,
    pub(super) column_gap: f32,
}

impl LargeIconGridMetrics {
    pub(super) fn new(viewport_width: f32) -> Self {
        let columns = large_icon_grid_columns(viewport_width);
        let column_gap = if columns > 1 {
            ((viewport_width - LARGE_ICON_TILE_WIDTH * columns as f32) / (columns - 1) as f32)
                .max(0.0)
        } else {
            0.0
        };

        Self {
            columns,
            column_gap,
        }
    }

    pub(super) fn row_count(self, entry_count: usize) -> usize {
        large_icon_grid_row_count(entry_count, self.columns)
    }

    pub(super) fn row_for_index(self, ix: usize) -> usize {
        large_icon_grid_row_for_index(ix, self.columns)
    }

    pub(super) fn index_bounds(self, ix: usize) -> (f32, f32, f32, f32) {
        let column = ix % self.columns;
        let row = self.row_for_index(ix);
        let stride = LARGE_ICON_TILE_WIDTH + self.column_gap;

        (
            column as f32 * stride,
            row as f32 * LARGE_ICON_TILE_HEIGHT,
            LARGE_ICON_TILE_WIDTH,
            LARGE_ICON_TILE_HEIGHT,
        )
    }

    fn column_at_x(self, content_x: f32) -> Option<usize> {
        for column in 0..self.columns {
            let left = column as f32 * (LARGE_ICON_TILE_WIDTH + self.column_gap);
            let right = left + LARGE_ICON_TILE_WIDTH;
            if content_x >= left && content_x < right {
                return Some(column);
            }
        }

        None
    }
}

pub(super) fn large_icon_grid_columns(viewport_width: f32) -> usize {
    (viewport_width / LARGE_ICON_TILE_WIDTH).floor().max(1.0) as usize
}

pub(super) fn large_icon_grid_row_count(entry_count: usize, columns: usize) -> usize {
    if entry_count == 0 {
        0
    } else {
        entry_count.div_ceil(columns.max(1))
    }
}

pub(super) fn large_icon_grid_row_for_index(ix: usize, columns: usize) -> usize {
    ix / columns.max(1)
}

pub(super) fn large_icon_grid_indices_intersecting_content_box(
    selection_box: SelectionBox,
    viewport_width: f32,
    entry_count: usize,
    layout: Option<&LargeIconLayout>,
) -> BTreeSet<usize> {
    if selection_box.is_empty() || entry_count == 0 {
        return BTreeSet::new();
    }

    if let Some(layout) = layout {
        return (0..entry_count)
            .filter(|ix| {
                let Some((left, top, width, height)) = layout.index_bounds(*ix) else {
                    return false;
                };
                left < selection_box.left + selection_box.width
                    && left + width > selection_box.left
                    && top < selection_box.bottom()
                    && top + height > selection_box.top
            })
            .collect();
    }

    let metrics = LargeIconGridMetrics::new(viewport_width);
    let row_count = metrics.row_count(entry_count);
    if row_count == 0 {
        return BTreeSet::new();
    }

    let first_row = (selection_box.top / LARGE_ICON_TILE_HEIGHT)
        .floor()
        .max(0.0) as usize;
    let last_row = ((selection_box.bottom() / LARGE_ICON_TILE_HEIGHT).ceil() as usize)
        .saturating_sub(1)
        .min(row_count - 1);

    (first_row..=last_row)
        .flat_map(|row| {
            let start = row * metrics.columns;
            let end = (start + metrics.columns).min(entry_count);
            start..end
        })
        .filter(|ix| {
            let (left, top, width, height) = metrics.index_bounds(*ix);
            left < selection_box.left + selection_box.width
                && left + width > selection_box.left
                && top < selection_box.bottom()
                && top + height > selection_box.top
        })
        .collect()
}

#[cfg(test)]
pub(super) fn row_indices_intersecting_box(
    selection_box: SelectionBox,
    scroll_top: f32,
    entry_count: usize,
) -> BTreeSet<usize> {
    row_indices_intersecting_content_box(selection_box.translated_y(scroll_top), entry_count)
}

#[cfg(test)]
pub(super) fn pointer_drag_intent_at(
    local_x: f32,
    local_y: f32,
    scroll_top: f32,
    viewport_width: f32,
    entry_count: usize,
    selected_indices: &BTreeSet<usize>,
) -> Option<PointerDragIntent> {
    pointer_drag_intent_at_with_row_height(
        local_x,
        local_y,
        scroll_top,
        viewport_width,
        entry_count,
        selected_indices,
        crate::explorer::constants::ROW_HEIGHT,
    )
}

#[cfg(test)]
pub(super) fn pointer_drag_intent_at_with_row_height(
    local_x: f32,
    local_y: f32,
    scroll_top: f32,
    viewport_width: f32,
    entry_count: usize,
    selected_indices: &BTreeSet<usize>,
    row_height: f32,
) -> Option<PointerDragIntent> {
    let name_column_width = crate::explorer::constants::effective_name_column_width(
        viewport_width + SCROLLBAR_GUTTER_WIDTH,
    );
    pointer_drag_intent_at_with_offsets(
        local_x,
        local_y,
        scroll_top,
        0.0,
        viewport_width,
        name_column_width,
        entry_count,
        selected_indices,
        row_height,
    )
}

pub(super) fn details_name_item_hit_right(visible_text_width: f32, name_column_width: f32) -> f32 {
    (DETAILS_NAME_CELL_LEFT_PADDING
        + FILE_ICON_SLOT_WIDTH
        + DETAILS_NAME_ICON_TEXT_GAP
        + visible_text_width.max(0.0))
    .min(name_column_width)
    .max(0.0)
}

#[cfg(test)]
pub(super) fn pointer_drag_intent_at_with_offsets(
    local_x: f32,
    local_y: f32,
    scroll_top: f32,
    scroll_left: f32,
    viewport_width: f32,
    name_column_width: f32,
    entry_count: usize,
    selected_indices: &BTreeSet<usize>,
    row_height: f32,
) -> Option<PointerDragIntent> {
    pointer_drag_intent_at_with_offsets_and_name_hits(
        local_x,
        local_y,
        scroll_top,
        scroll_left,
        viewport_width,
        name_column_width,
        entry_count,
        selected_indices,
        row_height,
        &[],
    )
}

pub(super) fn pointer_drag_intent_at_with_offsets_and_name_hits(
    local_x: f32,
    local_y: f32,
    scroll_top: f32,
    scroll_left: f32,
    viewport_width: f32,
    name_column_width: f32,
    entry_count: usize,
    selected_indices: &BTreeSet<usize>,
    row_height: f32,
    details_name_item_hit_rights: &[f32],
) -> Option<PointerDragIntent> {
    if local_x < 0.0 || local_y < 0.0 || local_x > viewport_width {
        return None;
    }

    let Some(ix) = row_index_at_content_y(local_y + scroll_top, entry_count, row_height) else {
        return Some(PointerDragIntent::RubberBand);
    };

    let content_x = local_x + scroll_left;
    if content_x < name_column_width {
        let hit_right = details_name_item_hit_rights
            .get(ix)
            .copied()
            .unwrap_or(name_column_width)
            .min(name_column_width)
            .max(0.0);
        if content_x >= DETAILS_NAME_CELL_LEFT_PADDING && content_x <= hit_right {
            Some(PointerDragIntent::ItemDrag)
        } else if selected_indices.contains(&ix) {
            Some(PointerDragIntent::ItemDrag)
        } else {
            Some(PointerDragIntent::RubberBand)
        }
    } else {
        Some(PointerDragIntent::ItemDrag)
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

fn row_index_at_content_y(content_y: f32, entry_count: usize, row_height: f32) -> Option<usize> {
    if content_y < 0.0 || entry_count == 0 {
        return None;
    }

    let ix = (content_y / row_height).floor() as usize;
    (ix < entry_count).then_some(ix)
}

fn large_icon_pointer_drag_intent_at(
    local_x: f32,
    local_y: f32,
    scroll_top: f32,
    viewport_width: f32,
    entry_count: usize,
    layout: Option<&LargeIconLayout>,
) -> Option<PointerDragIntent> {
    if local_x < 0.0 || local_y < 0.0 || local_x > viewport_width {
        return None;
    }

    if let Some(layout) = layout {
        return if layout
            .index_at_content_point(local_x, local_y + scroll_top, entry_count)
            .is_some()
        {
            Some(PointerDragIntent::ItemDrag)
        } else {
            Some(PointerDragIntent::RubberBand)
        };
    }

    if large_icon_grid_index_at_content_point(
        local_x,
        local_y + scroll_top,
        viewport_width,
        entry_count,
    )
    .is_some()
    {
        Some(PointerDragIntent::ItemDrag)
    } else {
        Some(PointerDragIntent::RubberBand)
    }
}

fn large_icon_grid_index_at_content_point(
    content_x: f32,
    content_y: f32,
    viewport_width: f32,
    entry_count: usize,
) -> Option<usize> {
    if content_x < 0.0 || content_y < 0.0 || entry_count == 0 {
        return None;
    }

    let metrics = LargeIconGridMetrics::new(viewport_width);
    let column = metrics.column_at_x(content_x)?;
    let row = (content_y / LARGE_ICON_TILE_HEIGHT).floor() as usize;

    let ix = row * metrics.columns + column;
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
    use crate::explorer::constants::{
        FILE_ICON_SLOT_WIDTH, LARGE_ICON_TILE_HEIGHT, LARGE_ICON_TILE_WIDTH, ROW_HEIGHT,
        effective_name_column_width,
    };
    use crate::explorer::context_menu::ContextMenuState;
    use crate::explorer::large_icons::{LargeIconLayout, large_icon_tile_height_for_rows};
    use crate::explorer::test_support::{selected_names, test_view_with_entries};
    use crate::settings::FileColumnKind;

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
    fn large_icon_grid_columns_use_tile_width_with_minimum_one() {
        assert_eq!(large_icon_grid_columns(0.0), 1);
        assert_eq!(large_icon_grid_columns(LARGE_ICON_TILE_WIDTH - 1.0), 1);
        assert_eq!(large_icon_grid_columns(LARGE_ICON_TILE_WIDTH * 3.0), 3);
    }

    #[test]
    fn large_icon_grid_row_count_rounds_up_partial_rows() {
        assert_eq!(large_icon_grid_row_count(0, 3), 0);
        assert_eq!(large_icon_grid_row_count(1, 3), 1);
        assert_eq!(large_icon_grid_row_count(4, 3), 2);
    }

    #[test]
    fn large_icon_grid_metrics_distribute_extra_width_as_column_gap() {
        let metrics = LargeIconGridMetrics::new(LARGE_ICON_TILE_WIDTH * 3.0 + 60.0);

        assert_eq!(metrics.columns, 3);
        assert_eq!(metrics.column_gap, 30.0);
        assert_eq!(
            metrics.index_bounds(1),
            (
                LARGE_ICON_TILE_WIDTH + 30.0,
                0.0,
                LARGE_ICON_TILE_WIDTH,
                LARGE_ICON_TILE_HEIGHT
            )
        );
        assert_eq!(
            metrics.index_bounds(2),
            (
                (LARGE_ICON_TILE_WIDTH + 30.0) * 2.0,
                0.0,
                LARGE_ICON_TILE_WIDTH,
                LARGE_ICON_TILE_HEIGHT
            )
        );
    }

    #[test]
    fn large_icon_grid_metrics_keep_partial_final_rows_left_aligned() {
        let metrics = LargeIconGridMetrics::new(LARGE_ICON_TILE_WIDTH * 3.0 + 60.0);

        assert_eq!(metrics.row_count(5), 2);
        assert_eq!(
            metrics.index_bounds(3),
            (
                0.0,
                LARGE_ICON_TILE_HEIGHT,
                LARGE_ICON_TILE_WIDTH,
                LARGE_ICON_TILE_HEIGHT
            )
        );
        assert_eq!(
            metrics.index_bounds(4),
            (
                LARGE_ICON_TILE_WIDTH + 30.0,
                LARGE_ICON_TILE_HEIGHT,
                LARGE_ICON_TILE_WIDTH,
                LARGE_ICON_TILE_HEIGHT
            )
        );
    }

    #[test]
    fn large_icon_grid_intersections_use_tile_rectangles() {
        let gap = 30.0;
        let box_over_second_column = SelectionBox::new(
            LARGE_ICON_TILE_WIDTH + gap + 1.0,
            1.0,
            LARGE_ICON_TILE_WIDTH * 2.0 + gap - 1.0,
            LARGE_ICON_TILE_HEIGHT - 1.0,
        );

        let indices = large_icon_grid_indices_intersecting_content_box(
            box_over_second_column,
            LARGE_ICON_TILE_WIDTH * 3.0 + gap * 2.0,
            6,
            None,
        );

        assert_eq!(indices, BTreeSet::from([1]));
    }

    #[test]
    fn large_icon_grid_intersections_ignore_dynamic_column_gaps() {
        let gap = 30.0;
        let box_inside_gap = SelectionBox::new(
            LARGE_ICON_TILE_WIDTH + 1.0,
            1.0,
            LARGE_ICON_TILE_WIDTH + gap - 1.0,
            LARGE_ICON_TILE_HEIGHT - 1.0,
        );

        let indices = large_icon_grid_indices_intersecting_content_box(
            box_inside_gap,
            LARGE_ICON_TILE_WIDTH * 3.0 + gap * 2.0,
            6,
            None,
        );

        assert!(indices.is_empty());
    }

    #[test]
    fn large_icon_grid_intersections_span_rows() {
        let selection_box = SelectionBox::new(
            LARGE_ICON_TILE_WIDTH - 1.0,
            LARGE_ICON_TILE_HEIGHT - 1.0,
            LARGE_ICON_TILE_WIDTH * 2.0 + 1.0,
            LARGE_ICON_TILE_HEIGHT + 1.0,
        );

        let indices = large_icon_grid_indices_intersecting_content_box(
            selection_box,
            LARGE_ICON_TILE_WIDTH * 2.0,
            4,
            None,
        );

        assert_eq!(indices, BTreeSet::from([0, 1, 2, 3]));
    }

    #[test]
    fn large_icon_grid_intersections_use_variable_tile_heights() {
        let layout = LargeIconLayout::from_tile_heights(
            2,
            20.0,
            vec![
                large_icon_tile_height_for_rows(1),
                large_icon_tile_height_for_rows(3),
                large_icon_tile_height_for_rows(1),
                large_icon_tile_height_for_rows(1),
            ],
        );
        let first_row = layout.row_bounds(0).expect("first row");
        let second_row = layout.row_bounds(1).expect("second row");
        let selection_box = SelectionBox::new(
            0.0,
            first_row.top + first_row.tile_height - 1.0,
            LARGE_ICON_TILE_WIDTH * 2.0 + 20.0,
            second_row.top + 1.0,
        );

        let indices =
            large_icon_grid_indices_intersecting_content_box(selection_box, 0.0, 4, Some(&layout));

        assert_eq!(indices, BTreeSet::from([1, 2, 3]));
    }

    #[test]
    fn large_icon_pointer_drag_intent_treats_variable_row_gap_as_rubber_band() {
        let layout = LargeIconLayout::from_tile_heights(
            2,
            20.0,
            vec![
                large_icon_tile_height_for_rows(1),
                large_icon_tile_height_for_rows(1),
                large_icon_tile_height_for_rows(1),
            ],
        );
        let first_row = layout.row_bounds(0).expect("first row");

        assert_eq!(
            large_icon_pointer_drag_intent_at(
                1.0,
                first_row.tile_height + 1.0,
                0.0,
                LARGE_ICON_TILE_WIDTH * 2.0 + 20.0,
                3,
                Some(&layout),
            ),
            Some(PointerDragIntent::RubberBand)
        );
        assert_eq!(
            large_icon_pointer_drag_intent_at(
                1.0,
                first_row.height,
                0.0,
                LARGE_ICON_TILE_WIDTH * 2.0 + 20.0,
                3,
                Some(&layout),
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn large_icon_pointer_drag_intent_treats_dynamic_column_gap_as_rubber_band() {
        let gap = 30.0;
        assert_eq!(
            large_icon_pointer_drag_intent_at(
                LARGE_ICON_TILE_WIDTH + gap / 2.0,
                1.0,
                0.0,
                LARGE_ICON_TILE_WIDTH * 3.0 + gap * 2.0,
                6,
                None,
            ),
            Some(PointerDragIntent::RubberBand)
        );
        assert_eq!(
            large_icon_pointer_drag_intent_at(
                LARGE_ICON_TILE_WIDTH + gap + 1.0,
                1.0,
                0.0,
                LARGE_ICON_TILE_WIDTH * 3.0 + gap * 2.0,
                6,
                None,
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn details_name_item_hit_right_includes_icon_gap_and_visible_text() {
        let name_column_width = 250.0;
        let hit_right = details_name_item_hit_right(40.0, name_column_width);

        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                DETAILS_NAME_CELL_LEFT_PADDING,
                1.0,
                0.0,
                0.0,
                800.0,
                name_column_width,
                1,
                &BTreeSet::new(),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                DETAILS_NAME_CELL_LEFT_PADDING + FILE_ICON_SLOT_WIDTH + 1.0,
                1.0,
                0.0,
                0.0,
                800.0,
                name_column_width,
                1,
                &BTreeSet::new(),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                hit_right - 1.0,
                1.0,
                0.0,
                0.0,
                800.0,
                name_column_width,
                1,
                &BTreeSet::new(),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn details_name_blank_padding_and_short_name_space_resolve_to_rubber_band() {
        let name_column_width = 250.0;
        let hit_right = details_name_item_hit_right(40.0, name_column_width);

        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                DETAILS_NAME_CELL_LEFT_PADDING - 1.0,
                1.0,
                0.0,
                0.0,
                800.0,
                name_column_width,
                1,
                &BTreeSet::new(),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::RubberBand)
        );
        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                hit_right + 1.0,
                1.0,
                0.0,
                0.0,
                800.0,
                name_column_width,
                1,
                &BTreeSet::new(),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::RubberBand)
        );
    }

    #[test]
    fn selected_row_blank_name_space_resolves_to_item_drag() {
        let name_column_width = 250.0;
        let hit_right = details_name_item_hit_right(40.0, name_column_width);

        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                hit_right + 1.0,
                1.0,
                0.0,
                0.0,
                800.0,
                name_column_width,
                1,
                &BTreeSet::from([0]),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn selected_row_name_hit_resolves_to_item_drag_with_scroll_offset() {
        let name_column_width = 250.0;
        let hit_right = details_name_item_hit_right(40.0, name_column_width);

        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                DETAILS_NAME_CELL_LEFT_PADDING,
                1.0,
                ROW_HEIGHT * 2.0,
                0.0,
                800.0,
                name_column_width,
                5,
                &BTreeSet::from([2]),
                ROW_HEIGHT,
                &[hit_right, hit_right, hit_right],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn selected_row_resolves_to_item_drag_outside_name_column() {
        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                500.0,
                1.0,
                ROW_HEIGHT * 2.0,
                0.0,
                800.0,
                250.0,
                5,
                &BTreeSet::from([2]),
                ROW_HEIGHT,
                &[],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn unselected_row_blank_name_column_resolves_to_rubber_band() {
        let name_column_width = 250.0;
        let hit_right = details_name_item_hit_right(40.0, name_column_width);

        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                hit_right + 1.0,
                1.0,
                ROW_HEIGHT * 2.0,
                0.0,
                800.0,
                name_column_width,
                5,
                &BTreeSet::from([1]),
                ROW_HEIGHT,
                &[hit_right, hit_right, hit_right],
            ),
            Some(PointerDragIntent::RubberBand)
        );
    }

    #[test]
    fn unselected_row_outside_name_column_resolves_to_item_drag() {
        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                500.0,
                1.0,
                ROW_HEIGHT * 2.0,
                0.0,
                800.0,
                250.0,
                5,
                &BTreeSet::from([1]),
                ROW_HEIGHT,
                &[],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn unselected_row_uses_name_item_hit_boundary() {
        let list_width = 800.0;
        let name_column_width = effective_name_column_width(list_width + SCROLLBAR_GUTTER_WIDTH);
        let hit_right = details_name_item_hit_right(40.0, name_column_width);

        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                hit_right + 1.0,
                1.0,
                0.0,
                0.0,
                list_width,
                name_column_width,
                5,
                &BTreeSet::new(),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::RubberBand)
        );
        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                name_column_width + 1.0,
                1.0,
                0.0,
                0.0,
                list_width,
                name_column_width,
                5,
                &BTreeSet::new(),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn name_item_hit_boundary_accounts_for_horizontal_scroll() {
        let name_column_width = 250.0;
        let hit_right = details_name_item_hit_right(40.0, name_column_width);

        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                hit_right - 41.0,
                1.0,
                0.0,
                40.0,
                400.0,
                name_column_width,
                5,
                &BTreeSet::new(),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                hit_right - 39.0,
                1.0,
                0.0,
                40.0,
                400.0,
                name_column_width,
                5,
                &BTreeSet::new(),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::RubberBand)
        );
    }

    #[test]
    fn pointer_drag_intent_uses_clamped_visible_horizontal_scroll() {
        let view = test_view_with_entries(&["a.txt"]);
        {
            let mut scroll_state = view.scroll_handle.0.borrow_mut();
            scroll_state.last_item_size = Some(gpui::ItemSize {
                item: size(px(400.0), px(100.0)),
                contents: size(px(670.0), px(100.0)),
            });
            scroll_state
                .base_handle
                .set_offset(gpui::point(px(-500.0), px(0.0)));
        }

        assert_eq!(
            view.pointer_drag_intent_with_details_name_hit_targets(
                gpui::point(px(1.0), px(1.0)),
                size(px(400.0), px(100.0)),
                &[details_name_item_hit_right(40.0, 250.0)],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn pointer_drag_intent_uses_live_name_column_boundary() {
        let mut view = test_view_with_entries(&["a.txt"]);
        view.file_columns.widths.insert(FileColumnKind::Type, 300);
        let hit_right = details_name_item_hit_right(40.0, 330.0);

        assert_eq!(
            view.pointer_drag_intent_with_details_name_hit_targets(
                gpui::point(px(320.0), px(1.0)),
                size(px(900.0), px(100.0)),
                &[hit_right],
            ),
            Some(PointerDragIntent::RubberBand)
        );
        assert_eq!(
            view.pointer_drag_intent_with_details_name_hit_targets(
                gpui::point(px(350.0), px(1.0)),
                size(px(900.0), px(100.0)),
                &[hit_right],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn pointer_drag_intent_uses_manual_name_column_boundary() {
        let mut view = test_view_with_entries(&["a.txt"]);
        view.file_columns.name_width = Some(400);
        let hit_right = details_name_item_hit_right(40.0, 400.0);

        assert_eq!(
            view.pointer_drag_intent_with_details_name_hit_targets(
                gpui::point(px(hit_right + 1.0), px(1.0)),
                size(px(900.0), px(100.0)),
                &[hit_right],
            ),
            Some(PointerDragIntent::RubberBand)
        );
        assert_eq!(
            view.pointer_drag_intent_with_details_name_hit_targets(
                gpui::point(px(401.0), px(1.0)),
                size(px(900.0), px(100.0)),
                &[hit_right],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn long_details_name_hit_right_is_capped_to_name_column_width() {
        let name_column_width = 70.0;
        let hit_right = details_name_item_hit_right(500.0, name_column_width);

        assert_eq!(hit_right, name_column_width);
        assert_eq!(
            pointer_drag_intent_at_with_offsets_and_name_hits(
                name_column_width - 1.0,
                1.0,
                0.0,
                0.0,
                800.0,
                name_column_width,
                1,
                &BTreeSet::new(),
                ROW_HEIGHT,
                &[hit_right],
            ),
            Some(PointerDragIntent::ItemDrag)
        );
    }

    #[test]
    fn whitespace_resolves_to_rubber_band() {
        assert_eq!(
            pointer_drag_intent_at(1.0, ROW_HEIGHT * 5.0, 0.0, 800.0, 5, &BTreeSet::from([4])),
            Some(PointerDragIntent::RubberBand)
        );
    }

    #[test]
    fn empty_list_resolves_to_rubber_band() {
        assert_eq!(
            pointer_drag_intent_at(1.0, 1.0, 0.0, 800.0, 0, &BTreeSet::new()),
            Some(PointerDragIntent::RubberBand)
        );
    }

    #[test]
    fn outside_list_bounds_resolves_to_no_drag_intent() {
        assert_eq!(
            pointer_drag_intent_at(1.0, -1.0, 0.0, 800.0, 5, &BTreeSet::from([0])),
            None
        );
        assert_eq!(
            pointer_drag_intent_at(-1.0, 1.0, 0.0, 800.0, 5, &BTreeSet::from([0])),
            None
        );
        assert_eq!(
            pointer_drag_intent_at(801.0, 1.0, 0.0, 800.0, 5, &BTreeSet::from([0])),
            None
        );
    }

    #[test]
    fn item_drag_intent_does_not_create_mouse_selection_drag() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);

        view.begin_mouse_selection_drag_for_intent(
            MouseButton::Left,
            gpui::point(px(DETAILS_NAME_CELL_LEFT_PADDING), px(1.0)),
            size(px(800.0), px(100.0)),
            SelectionModifiers::default(),
        );

        assert!(view.mouse_selection_drag.is_none());
    }

    #[test]
    fn rubber_band_intent_creates_mouse_selection_drag() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);

        view.begin_mouse_selection_drag_for_intent(
            MouseButton::Left,
            gpui::point(px(1.0), px(ROW_HEIGHT + 1.0)),
            size(px(800.0), px(100.0)),
            SelectionModifiers::default(),
        );

        assert!(view.mouse_selection_drag.is_some());
    }

    #[test]
    fn left_name_column_mouse_down_closes_context_menu_and_starts_rubber_band() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.context_menu = Some(ContextMenuState::new(
            gpui::point(px(200.0), px(200.0)),
            Vec::new(),
        ));

        let outcome = view.begin_mouse_selection_drag_after_menu_dismissal(
            MouseButton::Left,
            gpui::point(px(1.0), px(ROW_HEIGHT + 1.0)),
            size(px(800.0), px(100.0)),
            SelectionModifiers::default(),
        );

        assert_eq!(
            outcome,
            MouseSelectionPointerDownOutcome {
                menu_closed: true,
                selection_started: true,
            }
        );
        assert!(view.context_menu.is_none());
        let drag = view
            .mouse_selection_drag
            .as_ref()
            .expect("rubber-band drag");
        assert_eq!(drag.button, MouseButton::Left);
    }

    #[test]
    fn left_non_name_column_mouse_down_closes_context_menu_without_rubber_band() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.context_menu = Some(ContextMenuState::new(
            gpui::point(px(200.0), px(200.0)),
            Vec::new(),
        ));

        let outcome = view.begin_mouse_selection_drag_after_menu_dismissal(
            MouseButton::Left,
            gpui::point(px(500.0), px(ROW_HEIGHT + 1.0)),
            size(px(800.0), px(100.0)),
            SelectionModifiers::default(),
        );

        assert_eq!(
            outcome,
            MouseSelectionPointerDownOutcome {
                menu_closed: true,
                selection_started: false,
            }
        );
        assert!(view.context_menu.is_none());
        assert!(view.mouse_selection_drag.is_none());
    }

    #[test]
    fn right_name_column_mouse_down_still_closes_context_menu_and_starts_rubber_band() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.context_menu = Some(ContextMenuState::new(
            gpui::point(px(200.0), px(200.0)),
            Vec::new(),
        ));

        let outcome = view.begin_mouse_selection_drag_after_menu_dismissal(
            MouseButton::Right,
            gpui::point(px(1.0), px(ROW_HEIGHT + 1.0)),
            size(px(800.0), px(100.0)),
            SelectionModifiers::default(),
        );

        assert_eq!(
            outcome,
            MouseSelectionPointerDownOutcome {
                menu_closed: true,
                selection_started: true,
            }
        );
        assert!(view.context_menu.is_none());
        let drag = view
            .mouse_selection_drag
            .as_ref()
            .expect("rubber-band drag");
        assert_eq!(drag.button, MouseButton::Right);
    }

    #[test]
    fn right_rubber_band_is_visible_but_inactive_on_mouse_down() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);

        view.begin_mouse_selection_drag_for_intent(
            MouseButton::Right,
            gpui::point(px(10.0), px(ROW_HEIGHT * 2.0)),
            size(px(800.0), px(100.0)),
            SelectionModifiers::default(),
        );

        let drag = view
            .mouse_selection_drag
            .as_ref()
            .expect("rubber-band drag");
        assert!(drag.visible);
        assert!(!drag.active);
        assert_eq!(
            view.visible_mouse_selection_box(),
            Some(SelectionBox {
                left: 10.0,
                top: ROW_HEIGHT * 2.0,
                width: 2.0,
                height: 2.0,
            })
        );
    }

    #[test]
    fn left_rubber_band_stays_hidden_until_drag_activation() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);

        view.begin_mouse_selection_drag_for_intent(
            MouseButton::Left,
            gpui::point(px(10.0), px(ROW_HEIGHT * 2.0)),
            size(px(800.0), px(100.0)),
            SelectionModifiers::default(),
        );

        let drag = view
            .mouse_selection_drag
            .as_ref()
            .expect("rubber-band drag");
        assert!(!drag.visible);
        assert!(!drag.active);
        assert!(view.visible_mouse_selection_box().is_none());

        view.update_mouse_selection_drag(
            gpui::point(px(14.0), px(ROW_HEIGHT * 2.0)),
            size(px(800.0), px(100.0)),
        );

        let drag = view
            .mouse_selection_drag
            .as_ref()
            .expect("rubber-band drag");
        assert!(drag.visible);
        assert!(drag.active);
        assert!(view.visible_mouse_selection_box().is_some());
    }

    #[test]
    fn plain_rubber_band_start_clears_existing_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);

        view.begin_mouse_selection_drag_for_intent(
            MouseButton::Left,
            gpui::point(px(1.0), px(ROW_HEIGHT + 1.0)),
            size(px(800.0), px(100.0)),
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
            MouseButton::Left,
            gpui::point(px(1.0), px(ROW_HEIGHT + 1.0)),
            size(px(800.0), px(100.0)),
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
    fn shift_mouse_down_preserves_anchor_for_click_selection() {
        let mut view = test_view_with_entries(&[
            "a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt", "g.txt", "h.txt",
        ]);
        view.select_single_index(4);

        view.begin_mouse_selection_drag_for_intent(
            MouseButton::Left,
            gpui::point(px(1.0), px(ROW_HEIGHT * 7.0 + 1.0)),
            size(px(800.0), px(300.0)),
            SelectionModifiers {
                toggle: false,
                extend: true,
            },
        );

        assert_eq!(selected_names(&view), vec!["e.txt"]);
        assert_eq!(
            view.mouse_selection_drag
                .as_ref()
                .expect("rubber-band drag")
                .initial_selection,
            BTreeSet::from([4])
        );

        view.end_mouse_selection_drag(MouseButton::Left);
        view.apply_click_selection(
            7,
            SelectionModifiers {
                toggle: false,
                extend: true,
            },
        );

        assert_eq!(
            selected_names(&view),
            vec!["e.txt", "f.txt", "g.txt", "h.txt"]
        );
        assert_eq!(view.selection.anchor_index, Some(4));
        assert_eq!(view.selection.focused_index, Some(7));
    }

    #[test]
    fn cancel_mouse_selection_drag_removes_active_selection_box() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);

        view.begin_mouse_selection_drag_for_intent(
            MouseButton::Left,
            gpui::point(px(1.0), px(1.0)),
            size(px(800.0), px(100.0)),
            SelectionModifiers::default(),
        );
        view.update_mouse_selection_drag(
            gpui::point(px(40.0), px(ROW_HEIGHT * 2.0)),
            size(px(100.0), px(100.0)),
        );
        assert!(view.visible_mouse_selection_box().is_some());

        assert!(view.cancel_mouse_selection_drag());

        assert!(view.mouse_selection_drag.is_none());
        assert!(view.visible_mouse_selection_box().is_none());
    }

    #[test]
    fn cancel_mouse_selection_drag_is_no_op_without_drag() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);

        assert!(!view.cancel_mouse_selection_drag());
        assert!(view.mouse_selection_drag.is_none());
    }

    #[test]
    fn mouse_selection_drag_only_ends_for_initiating_button() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.begin_mouse_selection_drag(
            MouseButton::Right,
            gpui::point(px(1.0), px(1.0)),
            SelectionModifiers::default(),
        );
        view.update_mouse_selection_drag(
            gpui::point(px(40.0), px(ROW_HEIGHT * 2.0)),
            size(px(100.0), px(100.0)),
        );

        assert!(!view.end_mouse_selection_drag(MouseButton::Left));
        assert!(view.mouse_selection_drag.is_some());
        assert!(view.end_mouse_selection_drag(MouseButton::Right));
        assert!(view.mouse_selection_drag.is_none());
        assert!(!view.suppress_next_click);
    }

    #[test]
    fn content_drag_box_keeps_start_pinned_after_scrolling() {
        let drag = MouseSelectionDrag {
            button: MouseButton::Left,
            start: gpui::point(px(10.0), px(60.0)),
            current: gpui::point(px(80.0), px(40.0)),
            start_scroll_top: 0.0,
            current_scroll_top: 112.0,
            modifiers: SelectionModifiers::default(),
            initial_selection: BTreeSet::new(),
            visible: true,
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
            button: MouseButton::Left,
            start: gpui::point(px(80.0), px(70.0)),
            current: gpui::point(px(10.0), px(8.0)),
            start_scroll_top: ROW_HEIGHT * 8.0,
            current_scroll_top: ROW_HEIGHT * 4.0,
            modifiers: SelectionModifiers::default(),
            initial_selection: BTreeSet::new(),
            visible: true,
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
