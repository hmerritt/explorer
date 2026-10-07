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

use super::book::{Block, Chapter, ContentPoint, Span, TextKind};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PageGeometry {
    pub width: f32,
    pub height: f32,
    pub margin_x: f32,
    pub margin_y: f32,
}

impl PageGeometry {
    pub fn new(width: f32, height: f32) -> Self {
        let margin_x = if width < 400.0 { 16.0 } else { 48.0 };
        let margin_y = if height < 240.0 { 16.0 } else { 32.0 };
        Self {
            width: (width - margin_x * 2.0).max(1.0),
            height: (height - margin_y * 2.0).max(1.0),
            margin_x,
            margin_y,
        }
    }
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
    (width.max(1.0) * factor, height.max(1.0) * factor)
}

pub(super) fn text_font(family: &str, kind: &TextKind, bold: bool, italic: bool) -> Font {
    let mut font = font(family.to_owned());
    if bold || kind.heading > 0 {
        font.weight = FontWeight::BOLD;
    }
    if italic || kind.quote {
        font.style = FontStyle::Italic;
    }
    font
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
                font: text_font(family, kind, span.style.bold, span.style.italic),
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
                    y,
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
                let height = size * 1.5;
                let x = (kind.indent as f32 * font_size).min(geometry.width * 0.3);
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
                        let line_runs =
                            runs_for_range(spans, start..end, family, kind, color, link_color);
                        let line = window.text_system().shape_line(
                            text[start..end].to_owned().into(),
                            px(size),
                            &line_runs,
                            None,
                        );
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
                    }
                    if full {
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
                // Include trailing line breaks in the content cursor even though
                // they have no glyph. This guarantees progress for empty lines.
                cursor.offset = end;
                if cursor.offset >= text.len() {
                    cursor = ContentPoint {
                        block: cursor.block + 1,
                        offset: 0,
                    };
                    y += font_size * 0.5;
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
