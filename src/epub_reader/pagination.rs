use std::{
    collections::HashMap,
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    Font, FontStyle, FontWeight, Hsla, RenderImage, ScrollDelta, ShapedLine, TextRun,
    UnderlineStyle, Window, font, px,
};

use super::{
    book::{Alignment, Block, Chapter, ContentPoint, Span, TextKind},
    state::Preferences,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PageGeometry {
    pub width: f32,
    pub height: f32,
    pub margin_x: f32,
    pub margin_y: f32,
}

impl PageGeometry {
    #[cfg(test)]
    pub fn new(width: f32, height: f32) -> Self {
        Self::with_width(width, height, 900.0)
    }

    pub fn with_width(width: f32, height: f32, maximum: f32) -> Self {
        let margin_x = if width < 400.0 { 16.0 } else { 48.0 };
        let content_width = (width - margin_x * 2.0).clamp(1.0, maximum.max(1.0));
        let margin_y = if height < 240.0 { 16.0 } else { 32.0 };
        Self {
            width: content_width,
            height: (height - margin_y * 2.0).max(1.0),
            margin_x: margin_x.max((width - content_width) * 0.5),
            margin_y,
        }
    }
}

/// Expand word spaces in a private layout: painting and byte-index hit testing
/// must see the same glyph positions without changing GPUI's shaping cache.
pub(super) fn justify_line(line: &mut ShapedLine, width: f32) {
    let text = line.text.as_ref();
    let first = text.len() - text.trim_start_matches(' ').len();
    let last = text.trim_end_matches(' ').len();
    let spaces: Vec<_> = text
        .char_indices()
        .filter_map(|(index, ch)| (ch == ' ' && index >= first && index < last).then_some(index))
        .collect();
    let extra = width - f32::from(line.width);
    if spaces.is_empty()
        || extra <= 0.0
        || extra / spaces.len() as f32 > f32::from(line.font_size) * 0.6
    {
        return;
    }
    let layout: &gpui::LineLayout = line;
    let mut runs = layout.runs.clone();
    for run in &mut runs {
        for glyph in &mut run.glyphs {
            let count = spaces.partition_point(|index| *index < glyph.index);
            glyph.position.x += px(extra * count as f32 / spaces.len() as f32);
        }
    }
    let layout = gpui::LineLayout {
        font_size: layout.font_size,
        width: px(width),
        ascent: layout.ascent,
        descent: layout.descent,
        runs,
        len: layout.len,
    };
    **line = Arc::new(layout);
}

#[derive(Clone)]
pub(super) struct ImageAsset {
    pub image: Arc<RenderImage>,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone)]
pub(super) struct ImageSlot {
    pub asset: Option<ImageAsset>,
    pub width: f32,
    pub height: f32,
    pub failed: bool,
}

impl ImageSlot {
    pub fn from_result(asset: Option<ImageAsset>) -> Self {
        let (width, height) = asset
            .as_ref()
            .map_or((240.0, 100.0), |asset| (asset.width, asset.height));
        Self {
            failed: asset.is_none(),
            asset,
            width,
            height,
        }
    }
}

#[derive(Clone)]
pub(super) enum PageItem {
    Rule {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    Text {
        block: usize,
        range: Range<usize>,
        line: ShapedLine,
        x: f32,
        y: f32,
        height: f32,
    },
    Image {
        block: usize,
        asset: Option<ImageAsset>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
}

#[derive(Clone)]
pub(super) struct Page {
    pub start: ContentPoint,
    pub end: ContentPoint,
    pub items: Vec<PageItem>,
    pub pending_images: Vec<usize>,
}

pub(super) fn fit_image(width: f32, height: f32, geometry: PageGeometry) -> (f32, f32) {
    let factor = (geometry.width / width.max(1.0))
        .min(geometry.height / height.max(1.0))
        .min(1.0);
    (
        (width.max(1.0) * factor).min(geometry.width),
        (height.max(1.0) * factor).min(geometry.height),
    )
}

pub(super) fn text_font(family: &str, kind: &TextKind, bold: bool, italic: bool) -> Font {
    let mut font = font(if kind.monospace {
        monospace_family().to_owned()
    } else {
        family.to_owned()
    });
    if bold || kind.heading > 0 {
        font.weight = FontWeight::BOLD;
    }
    if italic || kind.quote {
        font.style = FontStyle::Italic;
    }
    font
}

fn monospace_family() -> &'static str {
    if cfg!(target_os = "windows") {
        "Courier New"
    } else if cfg!(target_os = "macos") {
        "Menlo"
    } else {
        "DejaVu Sans Mono"
    }
}

fn runs_for_range(
    spans: &[Span],
    range: Range<usize>,
    family: &str,
    kind: &TextKind,
    color: Hsla,
    link_color: Hsla,
) -> Vec<TextRun> {
    spans
        .iter()
        .filter_map(|span| {
            let start = span.range.start.max(range.start);
            let end = span.range.end.min(range.end);
            (start < end).then(|| TextRun {
                len: end - start,
                font: text_font(
                    if span.style.monospace {
                        monospace_family()
                    } else {
                        family
                    },
                    kind,
                    span.style.bold,
                    span.style.italic,
                ),
                color: if span.style.link.is_some() {
                    link_color
                } else {
                    color
                },
                background_color: None,
                underline: (span.style.underline || span.style.link.is_some()).then_some(
                    UnderlineStyle {
                        thickness: px(1.0),
                        color: None,
                        wavy: false,
                    },
                ),
                strikethrough: None,
            })
        })
        .collect()
}

fn segment_end(text: &str, start: usize) -> usize {
    let mut end = (start + 8192).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end < text.len() {
        if let Some((index, _)) = text[start..end]
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace())
        {
            if index > 4096 {
                end = start + index + text[start + index..].chars().next().unwrap().len_utf8();
            }
        }
    }
    end
}

pub(super) fn normalize_point(chapter: &Chapter, mut point: ContentPoint) -> ContentPoint {
    point.block = point.block.min(chapter.blocks.len());
    match chapter.blocks.get(point.block) {
        Some(Block::Text { text, .. }) => {
            point.offset = point.offset.min(text.len());
            while !text.is_char_boundary(point.offset) {
                point.offset -= 1;
            }
        }
        _ => point.offset = 0,
    }
    point
}

/// Shapes at most one page of content (with bounded lookahead), rather than a
/// whole chapter. The page's end is also the next page's start.
#[cfg(test)]
pub(super) fn layout_page(
    chapter: &Chapter,
    start: ContentPoint,
    geometry: PageGeometry,
    family: &str,
    font_size: f32,
    color: Hsla,
    link_color: Hsla,
    images: &HashMap<usize, ImageSlot>,
    window: &mut Window,
) -> Result<Page, String> {
    layout_page_with_preferences(
        chapter,
        start,
        geometry,
        family,
        font_size,
        color,
        link_color,
        images,
        &Preferences::default(),
        window,
    )
}

pub(super) fn layout_page_with_preferences(
    chapter: &Chapter,
    start: ContentPoint,
    geometry: PageGeometry,
    family: &str,
    font_size: f32,
    color: Hsla,
    link_color: Hsla,
    images: &HashMap<usize, ImageSlot>,
    preferences: &Preferences,
    window: &mut Window,
) -> Result<Page, String> {
    let start = normalize_point(chapter, start);
    let mut page = Page {
        start,
        end: start,
        items: Vec::new(),
        pending_images: Vec::new(),
    };
    let mut cursor = start;
    let mut y = 0.0;
    while let Some(block) = chapter.blocks.get(cursor.block) {
        match block {
            Block::Rule => {
                let height = font_size * 1.5;
                if y + height > geometry.height && !page.items.is_empty() {
                    break;
                }
                page.items.push(PageItem::Rule {
                    x: geometry.width * 0.35,
                    y: y + height * 0.5,
                    width: geometry.width * 0.3,
                    height: 1.0,
                });
                y += height;
                cursor = ContentPoint {
                    block: cursor.block + 1,
                    offset: 0,
                };
            }
            Block::Image { .. } => {
                let slot = images.get(&cursor.block);
                let asset = slot.and_then(|slot| slot.asset.clone());
                let (width, height) = slot
                    .map(|slot| fit_image(slot.width, slot.height, geometry))
                    .unwrap_or((geometry.width.min(240.0), geometry.height.min(100.0)));
                if y + height > geometry.height && !page.items.is_empty() {
                    break;
                }
                if slot.is_none_or(|slot| slot.asset.is_none() && !slot.failed) {
                    page.pending_images.push(cursor.block);
                }
                page.items.push(PageItem::Image {
                    block: cursor.block,
                    asset,
                    x: (geometry.width - width) * 0.5,
                    y: if chapter.is_cover && chapter.blocks.len() == 1 {
                        (geometry.height - height) * 0.5
                    } else {
                        y
                    },
                    width,
                    height,
                });
                y += height + font_size * 0.5;
                cursor = ContentPoint {
                    block: cursor.block + 1,
                    offset: 0,
                };
            }
            Block::Text { text, spans, kind } => {
                if kind.table_columns > 1 && cursor.offset == 0 {
                    let row = kind.table_row;
                    let cells: Vec<_> = chapter.blocks[cursor.block..].iter().take_while(|block| matches!(block, Block::Text {kind, ..} if kind.table_row == row)).collect();
                    let cell_width = geometry.width / kind.table_columns as f32;
                    let mut items = Vec::new();
                    let mut row_height: f32 = 0.0;
                    let mut fits = true;
                    for (index, cell) in cells.iter().enumerate() {
                        if let Block::Text { kind, .. } = cell {
                            let mut cell = (*cell).clone();
                            if let Block::Text { kind, .. } = &mut cell {
                                kind.table_row = None;
                                kind.table_columns = 0;
                            }
                            let content = Chapter {
                                blocks: vec![cell],
                                ..Chapter::default()
                            };
                            let mut cell_page = layout_page_with_preferences(
                                &content,
                                ContentPoint::default(),
                                PageGeometry {
                                    width: (cell_width * kind.table_span.max(1) as f32 - 12.0)
                                        .max(1.0),
                                    ..geometry
                                },
                                family,
                                font_size,
                                color,
                                link_color,
                                images,
                                preferences,
                                window,
                            )?;
                            if cell_page.end.block == 0 {
                                fits = false;
                                break;
                            }
                            for item in &mut cell_page.items {
                                if let PageItem::Text {
                                    block,
                                    x,
                                    y: cell_y,
                                    height,
                                    ..
                                } = item
                                {
                                    *block = cursor.block + index;
                                    *x += cell_width * kind.table_column as f32 + 6.0;
                                    row_height = row_height.max(*cell_y + *height + 12.0);
                                    *cell_y += y + 6.0;
                                }
                            }
                            items.extend(cell_page.items);
                        }
                    }
                    row_height =
                        row_height.max(font_size * preferences.line_spacing as f32 / 10.0 + 12.0);
                    if fits && row_height <= geometry.height {
                        if y + row_height > geometry.height && !page.items.is_empty() {
                            break;
                        }
                        for cell in &cells {
                            if let Block::Text { kind, .. } = cell {
                                items.push(PageItem::Rule {
                                    x: kind.table_column as f32 * cell_width,
                                    y,
                                    width: 1.0,
                                    height: row_height,
                                });
                            }
                        }
                        items.push(PageItem::Rule {
                            x: geometry.width - 1.0,
                            y,
                            width: 1.0,
                            height: row_height,
                        });
                        items.push(PageItem::Rule {
                            x: 0.0,
                            y,
                            width: geometry.width,
                            height: 1.0,
                        });
                        items.push(PageItem::Rule {
                            x: 0.0,
                            y: y + row_height,
                            width: geometry.width,
                            height: 1.0,
                        });
                        page.items.extend(items);
                        y += row_height;
                        cursor = ContentPoint {
                            block: cursor.block + cells.len(),
                            offset: 0,
                        };
                        continue;
                    }
                    // Oversized rows flow as complete cells rather than clipping text.
                }
                if cursor.offset >= text.len() {
                    cursor = ContentPoint {
                        block: cursor.block + 1,
                        offset: 0,
                    };
                    continue;
                }
                let size = font_size
                    * match kind.heading {
                        1 => 1.6,
                        2 => 1.4,
                        3 => 1.2,
                        _ => 1.0,
                    };
                let height = size * preferences.line_spacing as f32 / 10.0;
                if cursor.offset == 0 && preferences.publisher_styles {
                    if kind.break_before && !page.items.is_empty() {
                        break;
                    }
                    let spacing = kind.margin_before.unwrap_or(0) as f32 / 1000.0 * font_size;
                    if y + spacing.max(0.0) + height > geometry.height && !page.items.is_empty() {
                        break;
                    }
                    y += spacing.max(0.0).min((geometry.height - height).max(0.0));
                }
                let x = ((kind.indent as f32 * font_size)
                    + if cursor.offset == 0 && preferences.publisher_styles {
                        kind.text_indent.unwrap_or(0) as f32 * font_size / 1000.0
                    } else {
                        0.0
                    })
                .clamp(0.0, geometry.width * 0.3);
                let first_line_indent = cursor.offset == 0
                    && preferences.publisher_styles
                    && kind.text_indent.is_some_and(|indent| indent != 0);
                let end = segment_end(text, cursor.offset);
                let runs =
                    runs_for_range(spans, cursor.offset..end, family, kind, color, link_color);
                let wrapped = window
                    .text_system()
                    .shape_text(
                        text[cursor.offset..end].to_owned().into(),
                        px(size),
                        &runs,
                        Some(px((geometry.width - x).max(1.0))),
                        None,
                    )
                    .map_err(|e| format!("Could not lay out EPUB text: {e}"))?;
                let mut physical_start = cursor.offset;
                let mut full = false;
                let mut restart = false;
                for physical in wrapped {
                    let mut boundaries = vec![0];
                    for boundary in physical.wrap_boundaries() {
                        boundaries
                            .push(physical.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index);
                    }
                    boundaries.push(physical.text.len());
                    for range in boundaries.windows(2) {
                        if y + height > geometry.height && !page.items.is_empty() {
                            full = true;
                            break;
                        }
                        let start = physical_start + range[0];
                        let end = physical_start + range[1];
                        let alignment = if preferences.publisher_styles {
                            kind.alignment
                        } else {
                            None
                        }
                        .unwrap_or(if preferences.justify {
                            Alignment::Justify
                        } else {
                            Alignment::Left
                        });
                        let justify = kind.heading == 0
                            && !kind.pre
                            && kind.table_row.is_none()
                            && alignment == Alignment::Justify;
                        // Keep consumed spaces in the semantic range, but don't
                        // let a wrap's trailing space push visible ink past the edge.
                        let visible = if justify {
                            text[start..end].trim_end_matches(' ')
                        } else {
                            &text[start..end]
                        };
                        let line_runs = runs_for_range(
                            spans,
                            start..start + visible.len(),
                            family,
                            kind,
                            color,
                            link_color,
                        );
                        let mut line = window
                            .text_system()
                            .shape_line(visible.to_owned().into(), px(size), &line_runs, None)
                            .with_len(end - start);
                        if justify && end < text.len() && !text[end..].starts_with('\n') {
                            justify_line(&mut line, (geometry.width - x).max(1.0));
                        }
                        if spans.iter().any(|span| {
                            span.style.baseline != 0
                                && span.range.start < end
                                && span.range.end > start
                        }) {
                            let mut layout = gpui::LineLayout {
                                font_size: line.font_size,
                                width: line.width,
                                ascent: line.ascent,
                                descent: line.descent,
                                len: line.len,
                                runs: line.runs.clone(),
                            };
                            for run in &mut layout.runs {
                                for glyph in &mut run.glyphs {
                                    if let Some(span) = spans
                                        .iter()
                                        .find(|span| span.range.contains(&(start + glyph.index)))
                                    {
                                        glyph.position.y +=
                                            px(span.style.baseline as f32 * size * 0.25);
                                    }
                                }
                            }
                            *line = Arc::new(layout);
                        }
                        let x = x + match alignment {
                            Alignment::Center => {
                                ((geometry.width - x - f32::from(line.width)) * 0.5).max(0.0)
                            }
                            Alignment::Right => {
                                (geometry.width - x - f32::from(line.width)).max(0.0)
                            }
                            _ => 0.0,
                        };
                        page.items.push(PageItem::Text {
                            block: cursor.block,
                            range: start..end,
                            line,
                            x,
                            y,
                            height,
                        });
                        y += height;
                        cursor.offset = end;
                        if first_line_indent {
                            restart = true;
                            break;
                        }
                    }
                    if full || restart {
                        break;
                    }
                    physical_start += physical.text.len();
                    if text.as_bytes().get(physical_start) == Some(&b'\n') {
                        physical_start += 1;
                        cursor.offset = physical_start;
                    }
                }
                if full {
                    break;
                }
                if restart {
                    continue;
                }
                // Include trailing line breaks in the content cursor even though
                // they have no glyph. This guarantees progress for empty lines.
                cursor.offset = end;
                if cursor.offset >= text.len() {
                    cursor = ContentPoint {
                        block: cursor.block + 1,
                        offset: 0,
                    };
                    y += if preferences.publisher_styles {
                        kind.margin_after
                            .map(|value| value as f32 / 1000.0 * font_size)
                            .unwrap_or(font_size * 0.5)
                            .max(0.0)
                    } else {
                        font_size * 0.5
                    };
                    if preferences.publisher_styles && kind.break_after {
                        break;
                    }
                }
            }
        }
    }
    page.end = cursor;
    Ok(page)
}

#[derive(Default)]
pub(super) struct WheelPager {
    accumulated: f32,
    last_input: Option<Instant>,
    last_turn: Option<Instant>,
    direction: f32,
    precise: bool,
}

impl WheelPager {
    pub fn turn(
        &mut self,
        delta: ScrollDelta,
        ordinary_lines_per_notch: f32,
        now: Instant,
    ) -> isize {
        let (value, precise) = match delta {
            ScrollDelta::Lines(delta) => (delta.y, false),
            ScrollDelta::Pixels(delta) => (f32::from(delta.y), true),
        };
        if !value.is_finite() || value == 0.0 {
            return 0;
        }
        if self
            .last_input
            .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(200))
            || self.direction != value.signum()
            || self.precise != precise
        {
            self.accumulated = 0.0;
        }
        self.last_input = Some(now);
        self.direction = value.signum();
        self.precise = precise;
        if precise
            && self
                .last_turn
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(150))
        {
            self.accumulated = 0.0;
            return 0;
        }
        self.accumulated += value;
        let threshold = if precise {
            80.0
        } else {
            ordinary_lines_per_notch.max(0.01)
        };
        let steps = (self.accumulated.abs() / threshold).floor() as isize;
        if steps == 0 {
            return 0;
        }
        let direction = if value < 0.0 { 1 } else { -1 };
        if precise {
            self.accumulated = 0.0;
            self.last_turn = Some(now);
            direction
        } else {
            self.accumulated -= self.direction * threshold * steps as f32;
            direction * steps
        }
    }
}

pub(super) fn selected_text(chapter: &Chapter, a: ContentPoint, b: ContentPoint) -> String {
    let (a, b) = (
        normalize_point(chapter, a.min(b)),
        normalize_point(chapter, a.max(b)),
    );
    let mut pieces = Vec::new();
    for index in a.block..=b.block.min(chapter.blocks.len().saturating_sub(1)) {
        if let Some(Block::Text { text, .. }) = chapter.blocks.get(index) {
            let start = if index == a.block { a.offset } else { 0 };
            let end = if index == b.block {
                b.offset
            } else {
                text.len()
            };
            if start <= end {
                pieces.push(text[start..end].to_owned());
            }
        }
    }
    pieces.join("\n\n")
}
