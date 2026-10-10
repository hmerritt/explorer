use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, FocusHandle,
    Focusable, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollWheelEvent, Task, TitlebarOptions, Window, WindowBounds, WindowDecorations,
    WindowOptions, canvas, div, fill, point, prelude::*, px, rgb, size,
};

use crate::{
    loaders::{LinearProgressStyle, linear_indeterminate},
    settings::{APP_ID, config_dir},
    window_chrome::{
        MAC_TRAFFIC_LIGHT_PADDING, TITLEBAR_HEIGHT, WindowDragState, render_platform_window_frame,
        render_titlebar_drag_overlay, render_titlebar_drag_region, render_window_controls,
    },
    window_state::{WindowStateOptions, load_window_state_from_path},
};

#[cfg(not(test))]
use crate::window_state::{StoredWindowState, save_window_state_to_path};

use super::{
    EpubBack, EpubBeginning, EpubCopy, EpubDismiss, EpubEnd, EpubNext, EpubPrevious,
    book::{Block, Book, Chapter, ContentPoint, ImageSource},
    pagination::{
        ImageAsset, ImageSlot, Page, PageGeometry, PageItem, WheelPager, layout_page,
        normalize_point, selected_text,
    },
    state::{self, Fingerprint, Location, Preferences},
};

const WINDOW_STATE_OPTIONS: WindowStateOptions = WindowStateOptions {
    min_width: 360.0,
    min_height: 240.0,
    include_fullscreen: true,
};
const PAGE_CACHE_SIZE: usize = 8;

pub(crate) fn open_epub_window(path: PathBuf, cx: &mut App) -> Result<(), String> {
    state::initialize(cx);
    let displays = cx
        .displays()
        .iter()
        .map(|display| display.bounds())
        .collect::<Vec<_>>();
    let bounds = config_dir()
        .and_then(|dir| {
            load_window_state_from_path(&dir.join("epub-window-state.json"), WINDOW_STATE_OPTIONS)
        })
        .and_then(|state| state.to_window_bounds(&displays, WINDOW_STATE_OPTIONS))
        .unwrap_or_else(|| {
            WindowBounds::Windowed(Bounds::centered(None, size(px(1024.0), px(820.0)), cx))
        });
    let title = format!(
        "{} — Explorer",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    cx.open_window(
        WindowOptions {
            window_bounds: Some(bounds),
            window_min_size: Some(size(px(360.0), px(240.0))),
            titlebar: Some(TitlebarOptions {
                title: Some(title.clone().into()),
                appears_transparent: true,
                traffic_light_position: cfg!(target_os = "macos")
                    .then_some(point(px(12.0), px(11.0))),
                ..Default::default()
            }),
            window_decorations: Some(if cfg!(target_os = "linux") {
                WindowDecorations::Client
            } else {
                WindowDecorations::Server
            }),
            app_id: Some(APP_ID.to_owned()),
            focus: true,
            ..Default::default()
        },
        move |window, cx| cx.new(|cx| Reader::new(path, title, window, cx)),
    )
    .map_err(|error| format!("Could not open EPUB reader window: {error}"))?;
    cx.activate(true);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum PageRequest {
    Index(isize),
    FromEnd(usize),
    Anchor(ContentPoint),
}

struct ChapterLayout {
    geometry: PageGeometry,
    font_size: u8,
    starts: Vec<ContentPoint>,
    complete: bool,
    pages: VecDeque<(usize, Page)>,
}

impl ChapterLayout {
    fn new(geometry: PageGeometry, font_size: u8) -> Self {
        Self {
            geometry,
            font_size,
            starts: vec![ContentPoint::default()],
            complete: false,
            pages: VecDeque::new(),
        }
    }
    fn cache(&mut self, index: usize, page: Page) {
        self.pages.retain(|(cached, _)| *cached != index);
        self.pages.push_back((index, page));
        while self.pages.len() > PAGE_CACHE_SIZE {
            self.pages.pop_front();
        }
    }
}

struct Reader {
    focus_handle: FocusHandle,
    title: String,
    should_move_window: bool,
    book: Option<Arc<Book>>,
    fingerprint: Option<Fingerprint>,
    key: String,
    href: String,
    chapters: HashMap<String, Arc<Chapter>>,
    empty_sections: HashSet<String>,
    layouts: HashMap<String, ChapterLayout>,
    images: HashMap<String, HashMap<usize, ImageSlot>>,
    current_page: Option<Page>,
    page_index: usize,
    request: Option<PageRequest>,
    anchor: ContentPoint,
    fragment: Option<String>,
    preferences: Preferences,
    font_family: String,
    error: Option<String>,
    chapters_open: bool,
    history: Vec<Location>,
    selection: Option<(ContentPoint, ContentPoint)>,
    selecting: bool,
    mouse_down: Option<Point<Pixels>>,
    wheel: WheelPager,
    load_cancel: Arc<AtomicBool>,
    chapter_cancel: Arc<AtomicBool>,
    load_task: Option<Task<()>>,
    chapter_task: Option<Task<()>>,
    prefetch_task: Option<Task<()>>,
    image_task: Option<Task<()>>,
}

impl Focusable for Reader {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl WindowDragState for Reader {
    fn set_window_drag_pending(&mut self, pending: bool) {
        self.should_move_window = pending;
    }
    fn take_window_drag_pending(&mut self) -> bool {
        std::mem::take(&mut self.should_move_window)
    }
}

impl Reader {
    fn new(path: PathBuf, title: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let names = cx.text_system().all_font_names();
        let font_family = [
            "Georgia",
            "Noto Serif",
            "DejaVu Serif",
            "Liberation Serif",
            "Times New Roman",
        ]
        .into_iter()
        .find(|family| names.iter().any(|name| name == family))
        .map(str::to_owned)
        .unwrap_or_else(|| window.text_style().font_family.to_string());
        let mut reader = Self {
            focus_handle,
            title,
            should_move_window: false,
            book: None,
            fingerprint: None,
            key: String::new(),
            href: String::new(),
            chapters: HashMap::new(),
            empty_sections: HashSet::new(),
            layouts: HashMap::new(),
            images: HashMap::new(),
            current_page: None,
            page_index: 0,
            request: None,
            anchor: ContentPoint::default(),
            fragment: None,
            preferences: state::preferences(cx),
            font_family,
            error: None,
            chapters_open: false,
            history: Vec::new(),
            selection: None,
            selecting: false,
            mouse_down: None,
            wheel: WheelPager::default(),
            load_cancel: Arc::new(AtomicBool::new(false)),
            chapter_cancel: Arc::new(AtomicBool::new(false)),
            load_task: None,
            chapter_task: None,
            prefetch_task: None,
            image_task: None,
        };
        cx.on_release(|reader, _| {
            reader.load_cancel.store(true, Ordering::Relaxed);
            reader.chapter_cancel.store(true, Ordering::Relaxed);
        })
        .detach();
        #[cfg(not(test))]
        cx.observe_window_bounds(window, |_, window, _| {
            if let (Some(path), Some(state)) = (
                config_dir().map(|dir| dir.join("epub-window-state.json")),
                StoredWindowState::from_window_bounds(window.window_bounds(), WINDOW_STATE_OPTIONS),
            ) {
                let _ = save_window_state_to_path(&path, &state);
            }
        })
        .detach();
        let cancel = reader.load_cancel.clone();
        reader.load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let fingerprint = Fingerprint::for_path(&path)
                        .map_err(|e| format!("Could not read book: {e}"))?;
                    let key = state::book_key(&path);
                    let book = Book::open(&path, &cancel)?;
                    Ok::<_, String>((Arc::new(book), key, fingerprint))
                })
                .await;
            let _ = this.update_in(cx, |reader, window, cx| {
                reader.load_task = None;
                match result {
                    Ok((book, key, fingerprint)) => {
                        let saved = state::resume(cx, &key, &fingerprint);
                        let location = saved
                            .filter(|location| book.readable_location(&location.href))
                            .unwrap_or_else(|| Location {
                                href: book.sections[0].href.clone(),
                                point: ContentPoint::default(),
                            });
                        reader.title = format!("{} — Explorer", book.title);
                        window.set_window_title(&reader.title);
                        reader.book = Some(book);
                        reader.key = key;
                        reader.fingerprint = Some(fingerprint);
                        let href = reader
                            .book
                            .as_ref()
                            .unwrap()
                            .reading_href(&location.href)
                            .to_owned();
                        reader.open_section(href, PageRequest::Anchor(location.point), None, cx);
                    }
                    Err(error) => reader.error = Some(error),
                }
                cx.notify();
            });
        }));
        reader
    }

    fn color(&self) -> Hsla {
        rgb(if self.preferences.light {
            0x202020
        } else {
            0xe8e6e3
        })
        .into()
    }
    fn link_color(&self) -> Hsla {
        rgb(if self.preferences.light {
            0x0759b5
        } else {
            0x8ebaff
        })
        .into()
    }

    fn clear_selection(&mut self) {
        self.selection = None;
        self.selecting = false;
        self.mouse_down = None;
    }

    fn open_section(
        &mut self,
        href: String,
        request: PageRequest,
        fragment: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.chapter_cancel.store(true, Ordering::Relaxed);
        self.chapter_cancel = Arc::new(AtomicBool::new(false));
        self.chapter_task = None;
        self.prefetch_task = None;
        self.image_task = None;
        self.href = href.clone();
        self.request = Some(request);
        self.anchor = match request {
            PageRequest::Anchor(point) => point,
            _ => ContentPoint::default(),
        };
        self.page_index = 0;
        self.current_page = None;
        self.error = None;
        self.fragment = fragment;
        self.clear_selection();
        if self.empty_sections.contains(&href)
            && self.book.as_ref().is_some_and(|book| book.is_cover(&href))
        {
            self.chapters
                .insert(href.clone(), Arc::new(Chapter::default()));
        }
        if self.chapters.contains_key(&href) {
            self.prefetch(cx);
            cx.notify();
            return;
        }
        let Some(book) = self.book.clone() else {
            return;
        };
        let cancel = self.chapter_cancel.clone();
        let expected_href = href.clone();
        self.chapter_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { book.chapter(&href, &cancel).map(Arc::new) })
                .await;
            let _ = this.update(cx, |reader, cx| {
                if reader.href != expected_href {
                    return;
                }
                reader.chapter_task = None;
                match result {
                    Ok(chapter) => {
                        if chapter.blocks.is_empty() {
                            reader.empty_sections.insert(reader.href.clone());
                        }
                        reader.chapters.insert(reader.href.clone(), chapter);
                        reader.prefetch(cx);
                    }
                    Err(error) => reader.error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn neighbors(&self) -> Vec<String> {
        let Some(book) = &self.book else {
            return vec![self.href.clone()];
        };
        let mut neighbors = vec![self.href.clone()];
        if let Some(index) = book
            .sections
            .iter()
            .position(|section| section.href == self.href)
        {
            if index > 0 {
                neighbors.push(book.sections[index - 1].href.clone());
            }
            if let Some(section) = book.sections.get(index + 1) {
                neighbors.push(section.href.clone());
            }
        }
        neighbors
    }

    fn prefetch(&mut self, cx: &mut Context<Self>) {
        let keep = self.neighbors();
        self.chapters.retain(|href, _| keep.contains(href));
        self.layouts.retain(|href, _| keep.contains(href));
        self.images.retain(|href, _| keep.contains(href));
        let missing = keep
            .into_iter()
            .filter(|href| !self.chapters.contains_key(href))
            .collect::<Vec<_>>();
        let Some(book) = self.book.clone() else {
            return;
        };
        if missing.is_empty() {
            return;
        }
        let cancel = self.chapter_cancel.clone();
        self.prefetch_task = Some(cx.spawn(async move |this, cx| {
            let chapters = cx
                .background_executor()
                .spawn(async move {
                    missing
                        .into_iter()
                        .filter_map(|href| {
                            book.chapter(&href, &cancel)
                                .ok()
                                .map(|chapter| (href, Arc::new(chapter)))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |reader, cx| {
                for (href, chapter) in chapters {
                    if reader.neighbors().contains(&href) {
                        if chapter.blocks.is_empty() {
                            reader.empty_sections.insert(href.clone());
                        }
                        reader.chapters.insert(href, chapter);
                    }
                }
                reader.prefetch_task = None;
                cx.notify();
            });
        }));
    }

    fn load_images(&mut self, pending: &[usize], cx: &mut Context<Self>) {
        if self.image_task.is_some() || pending.is_empty() {
            return;
        }
        let Some(chapter) = self.chapters.get(&self.href) else {
            return;
        };
        let Some(book) = self.book.clone() else {
            return;
        };
        let sources = pending
            .iter()
            .filter_map(|index| match chapter.blocks.get(*index) {
                Some(Block::Image { source, .. }) => Some((*index, source.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        let href = self.href.clone();
        let cancel = self.chapter_cancel.clone();
        self.image_task = Some(cx.spawn(async move |this, cx| {
            let images = cx
                .background_executor()
                .spawn(async move {
                    sources
                        .into_iter()
                        .map(|(index, source)| {
                            let asset = decode_image(&book, &source, &cancel).ok();
                            (index, asset)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |reader, cx| {
                reader.image_task = None;
                if reader.href != href {
                    return;
                }
                let cache = reader.images.entry(href.clone()).or_default();
                let mut dimensions_changed = false;
                for (index, asset) in images {
                    let slot = ImageSlot::from_result(asset);
                    dimensions_changed |= cache.get(&index).is_none_or(|previous| {
                        previous.width != slot.width || previous.height != slot.height
                    });
                    cache.insert(index, slot);
                }
                // A broken cover must never leave a reader stuck on a failed
                // image. Treat it like an empty section so paging skips it.
                let failed_cover = reader.chapters.get(&href).is_some_and(|chapter| {
                    chapter.is_cover
                        && !chapter.blocks.is_empty()
                        && chapter.blocks.iter().enumerate().all(|(index, block)| {
                            matches!(block, Block::Image { .. })
                                && cache.get(&index).is_some_and(|slot| slot.failed)
                        })
                });
                if failed_cover {
                    reader.empty_sections.insert(href.clone());
                    reader
                        .chapters
                        .insert(href.clone(), Arc::new(Chapter::default()));
                    reader.layouts.remove(&href);
                    reader.current_page = None;
                    reader.request = Some(PageRequest::Index(0));
                    cx.notify();
                    return;
                }
                // Keep the content position while replacing image placeholders
                // with their measured aspect ratios.
                if dimensions_changed {
                    reader.layouts.remove(&href);
                    reader.request = Some(PageRequest::Anchor(reader.anchor));
                } else if let Some(layout) = reader.layouts.get_mut(&href) {
                    layout.pages.clear();
                }
                reader.trim_images();
                cx.notify();
            });
        }));
    }

    fn turn(&mut self, steps: isize, cx: &mut Context<Self>) {
        if self.chapter_task.is_some() || self.current_page.is_none() || steps == 0 {
            return;
        }
        if self.request.is_none() {
            if self.at_boundary(steps > 0) {
                return;
            }
        }
        let index = match self.request {
            Some(PageRequest::Index(index)) => index,
            _ => self.page_index as isize,
        };
        self.request = Some(PageRequest::Index(index.saturating_add(steps)));
        self.clear_selection();
        cx.notify();
    }

    fn at_boundary(&self, end: bool) -> bool {
        let Some(book) = &self.book else {
            return true;
        };
        let Some(index) = book
            .sections
            .iter()
            .position(|section| section.href == self.href)
        else {
            return false;
        };
        if end {
            self.current_page.as_ref().is_some_and(|page| {
                self.chapters
                    .get(&self.href)
                    .is_some_and(|chapter| page.end.block >= chapter.blocks.len())
            }) && book.sections[index + 1..]
                .iter()
                .all(|section| self.empty_sections.contains(&section.href))
        } else {
            self.page_index == 0
                && book.sections[..index]
                    .iter()
                    .all(|section| self.empty_sections.contains(&section.href))
        }
    }

    fn adjacent_section(
        &mut self,
        forward: bool,
        request: PageRequest,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(book) = &self.book else {
            return false;
        };
        let Some(index) = book
            .sections
            .iter()
            .position(|section| section.href == self.href)
        else {
            return false;
        };
        let next = if forward {
            (index + 1..book.sections.len())
                .find(|index| !self.empty_sections.contains(&book.sections[*index].href))
        } else {
            (0..index)
                .rev()
                .find(|index| !self.empty_sections.contains(&book.sections[*index].href))
        };
        let Some(section) = next.and_then(|index| book.sections.get(index)) else {
            return false;
        };
        self.open_section(section.href.clone(), request, None, cx);
        true
    }

    fn boundary(&mut self, end: bool, cx: &mut Context<Self>) {
        let Some(book) = &self.book else {
            return;
        };
        let mut sections = book
            .sections
            .iter()
            .filter(|section| !self.empty_sections.contains(&section.href));
        let Some(section) = (if end {
            sections.next_back()
        } else {
            sections.next()
        }) else {
            return;
        };
        self.open_section(
            section.href.clone(),
            if end {
                PageRequest::FromEnd(0)
            } else {
                PageRequest::Index(0)
            },
            None,
            cx,
        );
    }

    fn remember(&self, cx: &mut Context<Self>) {
        if let Some(fingerprint) = &self.fingerprint {
            state::remember(
                cx,
                self.key.clone(),
                fingerprint.clone(),
                Location {
                    href: self.href.clone(),
                    point: self.anchor,
                },
            );
        }
    }

    fn prepare_page(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Page> {
        let chapter = self.chapters.get(&self.href)?.clone();
        if self.error.is_some() {
            return None;
        }
        if chapter.blocks.is_empty() && self.request.is_some() {
            let backwards = matches!(self.request, Some(PageRequest::FromEnd(_)))
                || matches!(self.request, Some(PageRequest::Index(index)) if index < 0);
            let request = match self.request.unwrap() {
                PageRequest::Index(index) if index < 0 => {
                    PageRequest::FromEnd((-index - 1) as usize)
                }
                PageRequest::Anchor(_) => PageRequest::Index(0),
                request => request,
            };
            if self.adjacent_section(!backwards, request, cx) {
                window.request_animation_frame();
                return None;
            }
            // Empty sections at either edge have no page. Return to the
            // nearest readable section instead of displaying a blank edge page.
            if self.adjacent_section(
                backwards,
                if backwards {
                    PageRequest::Index(0)
                } else {
                    PageRequest::FromEnd(0)
                },
                cx,
            ) {
                window.request_animation_frame();
                return None;
            }
        }
        let geometry =
            PageGeometry::new(f32::from(bounds.size.width), f32::from(bounds.size.height));
        if self.layouts.get(&self.href).is_none_or(|layout| {
            layout.geometry != geometry || layout.font_size != self.preferences.font_size
        }) {
            self.layouts.insert(
                self.href.clone(),
                ChapterLayout::new(geometry, self.preferences.font_size),
            );
            if self.request.is_none() {
                self.request = Some(PageRequest::Anchor(self.anchor));
            }
            self.clear_selection();
        }
        if let Some(fragment) = self.fragment.take() {
            let fragment = percent_encoding::percent_decode_str(&fragment).decode_utf8_lossy();
            if let Some(point) = chapter.anchors.get(fragment.as_ref()) {
                self.request = Some(PageRequest::Anchor(*point));
            }
        }
        let request = self
            .request
            .unwrap_or(PageRequest::Index(self.page_index as isize));
        if let PageRequest::Index(index) = request {
            if index < 0 {
                if self.adjacent_section(false, PageRequest::FromEnd((-index - 1) as usize), cx) {
                    window.request_animation_frame();
                    return None;
                }
                self.request = Some(PageRequest::Index(0));
                cx.notify();
                return self.current_page.clone();
            }
        }
        let started = Instant::now();
        let color = self.color();
        let link_color = self.link_color();
        let images = self.images.entry(self.href.clone()).or_default();
        let layout = self.layouts.get_mut(&self.href).unwrap();
        let mut found = None;
        loop {
            let wanted = match request {
                PageRequest::Index(index) if (index as usize) < layout.starts.len() => {
                    Some(index as usize)
                }
                PageRequest::FromEnd(distance) if layout.complete => {
                    Some(layout.starts.len().saturating_sub(distance + 1))
                }
                PageRequest::Anchor(point) => {
                    let point = normalize_point(&chapter, point);
                    let index = layout
                        .starts
                        .partition_point(|start| *start <= point)
                        .saturating_sub(1);
                    (index + 1 < layout.starts.len() || layout.complete).then_some(index)
                }
                _ => None,
            };
            let index = wanted.unwrap_or_else(|| layout.starts.len() - 1);
            let page =
                if let Some((_, page)) = layout.pages.iter().find(|(cached, _)| *cached == index) {
                    page.clone()
                } else {
                    match layout_page(
                        &chapter,
                        layout.starts[index],
                        geometry,
                        &self.font_family,
                        self.preferences.font_size as f32,
                        color,
                        link_color,
                        images,
                        window,
                    ) {
                        Ok(page) => {
                            layout.cache(index, page.clone());
                            page
                        }
                        Err(error) => {
                            self.error = Some(error);
                            cx.notify();
                            return None;
                        }
                    }
                };
            if index == layout.starts.len() - 1 && !layout.complete {
                if page.end.block >= chapter.blocks.len() || page.end == page.start {
                    layout.complete = true;
                } else {
                    layout.starts.push(page.end);
                }
            }
            let contains_anchor = matches!(request, PageRequest::Anchor(point) if normalize_point(&chapter, point) < page.end || layout.complete);
            if let PageRequest::FromEnd(distance) = request {
                if layout.complete
                    && distance < layout.starts.len()
                    && index != layout.starts.len() - distance - 1
                {
                    continue;
                }
            }
            if wanted.is_some() || contains_anchor || layout.complete {
                found = Some((index, page));
                break;
            }
            if started.elapsed().as_millis() >= 4 {
                window.request_animation_frame();
                cx.notify();
                break;
            }
        }
        let complete = layout.complete;
        let count = layout.starts.len();
        if complete {
            match request {
                PageRequest::Index(index) if index as usize >= count => {
                    if self.adjacent_section(true, PageRequest::Index(index - count as isize), cx) {
                        window.request_animation_frame();
                        return None;
                    }
                }
                PageRequest::FromEnd(distance) if distance >= count => {
                    if self.adjacent_section(false, PageRequest::FromEnd(distance - count), cx) {
                        window.request_animation_frame();
                        return None;
                    }
                }
                _ => {}
            }
        }
        if let Some((index, page)) = found {
            let changed = self
                .current_page
                .as_ref()
                .is_none_or(|current| current.start != page.start)
                || self.request.is_some();
            self.page_index = index;
            if let PageRequest::Anchor(point) = request {
                self.anchor = normalize_point(&chapter, point);
            } else if self.request.is_some() || self.current_page.is_none() {
                self.anchor = page.start;
            }
            self.request = None;
            self.current_page = Some(page.clone());
            if changed {
                self.remember(cx);
                cx.notify();
            }
            self.load_images(&page.pending_images, cx);
            self.trim_images();
            self.warm_next_page(&chapter, window);
            Some(page)
        } else {
            None
        }
    }

    fn warm_next_page(&mut self, chapter: &Chapter, window: &mut Window) {
        let color = self.color();
        let link_color = self.link_color();
        let Some(layout) = self.layouts.get_mut(&self.href) else {
            return;
        };
        let index = self.page_index + 1;
        if index >= layout.starts.len() || layout.pages.iter().any(|(cached, _)| *cached == index) {
            return;
        }
        let Some(images) = self.images.get(&self.href) else {
            return;
        };
        if let Ok(page) = layout_page(
            chapter,
            layout.starts[index],
            layout.geometry,
            &self.font_family,
            self.preferences.font_size as f32,
            color,
            link_color,
            images,
            window,
        ) {
            if index == layout.starts.len() - 1 && !layout.complete {
                if page.end.block >= chapter.blocks.len() || page.end == page.start {
                    layout.complete = true;
                } else {
                    layout.starts.push(page.end);
                }
            }
            layout.cache(index, page);
        }
    }

    fn trim_images(&mut self) {
        const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
        let protected = self
            .current_page
            .as_ref()
            .map(|page| {
                page.items
                    .iter()
                    .filter_map(|item| match item {
                        PageItem::Image { block, .. } => Some(*block),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut bytes = self
            .images
            .values()
            .flat_map(|images| images.values())
            .filter_map(|slot| slot.asset.as_ref())
            .map(|asset| asset.image.as_bytes(0).map_or(0, |bytes| bytes.len()))
            .sum::<usize>();
        if bytes <= MAX_DECODED_BYTES {
            return;
        }
        let mut candidates = Vec::new();
        for (href, images) in &self.images {
            for (block, slot) in images {
                if slot.asset.is_some() && (href != &self.href || !protected.contains(block)) {
                    candidates.push((href.clone(), *block));
                }
            }
        }
        candidates
            .sort_by_key(|(href, block)| (href != &self.href, block.abs_diff(self.anchor.block)));
        for (href, block) in candidates.into_iter().rev() {
            if bytes <= MAX_DECODED_BYTES {
                break;
            }
            if let Some(asset) = self
                .images
                .get_mut(&href)
                .and_then(|images| images.get_mut(&block))
                .and_then(|slot| slot.asset.take())
            {
                bytes =
                    bytes.saturating_sub(asset.image.as_bytes(0).map_or(0, |bytes| bytes.len()));
                if let Some(layout) = self.layouts.get_mut(&href) {
                    layout.pages.clear();
                }
            }
        }
    }

    fn follow_link(&mut self, target: String, cx: &mut Context<Self>) {
        if target.starts_with("http://") || target.starts_with("https://") {
            cx.background_executor()
                .spawn(async move {
                    let _ = open::that(target);
                })
                .detach();
            return;
        }
        let (href, fragment) = target
            .split_once('#')
            .map_or((target.as_str(), None), |(href, fragment)| {
                (href, Some(fragment.to_owned()))
            });
        let Some(book) = &self.book else {
            return;
        };
        if !book.readable_location(href) {
            return;
        }
        let href = book.reading_href(href).to_owned();
        self.history.push(Location {
            href: self.href.clone(),
            point: self.anchor,
        });
        self.chapters_open = false;
        self.open_section(href, PageRequest::Index(0), fragment, cx);
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        if let Some(location) = self.history.pop() {
            self.open_section(location.href, PageRequest::Anchor(location.point), None, cx);
        }
    }

    fn copy(&self, cx: &mut Context<Self>) {
        if let (Some((a, b)), Some(chapter)) = (self.selection, self.chapters.get(&self.href)) {
            let text = selected_text(chapter, a, b);
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    fn change_font(&mut self, steps: i16, cx: &mut Context<Self>) {
        self.preferences.font_size =
            (self.preferences.font_size as i16 + steps).clamp(12, 36) as u8;
        state::set_preferences(cx, self.preferences);
        self.request = Some(PageRequest::Anchor(self.anchor));
        self.clear_selection();
        cx.notify();
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.preferences.light = !self.preferences.light;
        state::set_preferences(cx, self.preferences);
        for layout in self.layouts.values_mut() {
            layout.pages.clear();
        }
        cx.notify();
    }

    fn render_titlebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let decorations = window.window_decorations();
        div()
            .id("epub-titlebar")
            .flex()
            .items_center()
            .relative()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(rgb(0xe8e8e8))
            .when(
                cfg!(target_os = "macos") && !window.is_fullscreen(),
                |this| {
                    this.child(
                        div()
                            .h_full()
                            .w(px(MAC_TRAFFIC_LIGHT_PADDING))
                            .flex_none()
                            .occlude(),
                    )
                },
            )
            .child(
                div()
                    .relative()
                    .h_full()
                    .max_w(px(620.0))
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .px(px(12.0))
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .text_color(rgb(0x1f1f1f))
                    .child(self.title.clone())
                    .child(render_titlebar_drag_overlay(
                        "epub-title-drag",
                        decorations,
                        cx,
                    )),
            )
            .child(render_titlebar_drag_region(
                "epub-titlebar-drag",
                decorations,
                cx,
            ))
            .children(render_window_controls(window))
            .into_any_element()
    }

    fn render_surface(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let paint_entity = entity.clone();
        canvas(
            move |bounds, window, cx| {
                entity.update(cx, |reader, cx| reader.prepare_page(bounds, window, cx))
            },
            move |bounds, page, window, cx| {
                let geometry =
                    PageGeometry::new(f32::from(bounds.size.width), f32::from(bounds.size.height));
                let origin = bounds.origin + point(px(geometry.margin_x), px(geometry.margin_y));
                let selection = paint_entity.read(cx).selection;
                let light = paint_entity.read(cx).preferences.light;
                if let Some(page) = &page {
                    window.paint_layer(bounds, |window| {
                        for item in &page.items {
                            match item {
                                PageItem::Text {
                                    block,
                                    range,
                                    line,
                                    x,
                                    y,
                                    height,
                                } => {
                                    let position = origin + point(px(*x), px(*y));
                                    if let Some((a, b)) = selection {
                                        let (a, b) = (a.min(b), a.max(b));
                                        if *block >= a.block && *block <= b.block {
                                            let start = if *block == a.block {
                                                a.offset.max(range.start)
                                            } else {
                                                range.start
                                            };
                                            let end = if *block == b.block {
                                                b.offset.min(range.end)
                                            } else {
                                                range.end
                                            };
                                            if start < end {
                                                let left = line.x_for_index(start - range.start);
                                                let right = line.x_for_index(end - range.start);
                                                window.paint_quad(fill(
                                                    Bounds::new(
                                                        position + point(left, px(0.0)),
                                                        size(
                                                            (right - left).max(px(1.0)),
                                                            px(*height),
                                                        ),
                                                    ),
                                                    rgb(if light { 0xb6d8ff } else { 0x334a70 }),
                                                ));
                                            }
                                        }
                                    }
                                    let _ = line.paint(position, px(*height), window, cx);
                                }
                                PageItem::Image {
                                    asset,
                                    x,
                                    y,
                                    width,
                                    height,
                                    block,
                                } => {
                                    let image_bounds = Bounds::new(
                                        origin + point(px(*x), px(*y)),
                                        size(px(*width), px(*height)),
                                    );
                                    if let Some(asset) = asset {
                                        let _ = window.paint_image(
                                            image_bounds,
                                            Default::default(),
                                            asset.image.clone(),
                                            0,
                                            false,
                                        );
                                    } else {
                                        window.paint_quad(fill(
                                            image_bounds,
                                            rgb(if light { 0xe7e7e7 } else { 0x303030 }),
                                        ));
                                        let reader = paint_entity.read(cx);
                                        let label = reader
                                            .chapters
                                            .get(&reader.href)
                                            .and_then(|chapter| chapter.blocks.get(*block))
                                            .and_then(|block| match block {
                                                Block::Image { alt, .. } if !alt.is_empty() => {
                                                    Some(alt.as_str())
                                                }
                                                _ => None,
                                            })
                                            .unwrap_or("Image");
                                        let run = gpui::TextRun {
                                            len: label.len(),
                                            font: gpui::font(reader.font_family.clone()),
                                            color: reader.color(),
                                            background_color: None,
                                            underline: None,
                                            strikethrough: None,
                                        };
                                        let line = window.text_system().shape_line(
                                            label.to_owned().into(),
                                            px(14.0),
                                            &[run],
                                            None,
                                        );
                                        let _ = line.paint(
                                            image_bounds.origin + point(px(8.0), px(8.0)),
                                            px(21.0),
                                            window,
                                            cx,
                                        );
                                    }
                                }
                            }
                        }
                    });
                }
                let entity = paint_entity.clone();
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || !bounds.contains(&event.position) {
                        return;
                    }
                    entity.update(cx, |reader, cx| {
                        let turns = reader.wheel.turn(
                            normalized_wheel_delta(event.delta),
                            ordinary_lines_per_notch(),
                            Instant::now(),
                        );
                        reader.turn(turns, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    });
                });
                let entity = paint_entity.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble
                        || event.button != MouseButton::Left
                        || !bounds.contains(&event.position)
                    {
                        return;
                    }
                    entity.update(cx, |reader, cx| {
                        if reader.chapters_open {
                            return;
                        }
                        reader.focus_handle.focus(window);
                        if let Some(point) = reader
                            .current_page
                            .as_ref()
                            .and_then(|page| hit_point(page, event.position - origin))
                        {
                            reader.selection = Some((point, point));
                            reader.selecting = true;
                            reader.mouse_down = Some(event.position);
                            cx.stop_propagation();
                            cx.notify();
                        }
                    });
                });
                let entity = paint_entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    entity.update(cx, |reader, cx| {
                        if reader.selecting && event.pressed_button == Some(MouseButton::Left) {
                            if let Some(point) = reader
                                .current_page
                                .as_ref()
                                .and_then(|page| hit_point(page, event.position - origin))
                            {
                                if let Some((anchor, _)) = reader.selection {
                                    reader.selection = Some((anchor, point));
                                    cx.notify();
                                }
                            }
                        }
                    });
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    paint_entity.update(cx, |reader, cx| {
                        if !reader.selecting {
                            return;
                        }
                        reader.selecting = false;
                        let clicked = reader.mouse_down.take().is_some_and(|down| {
                            (f32::from(down.x - event.position.x)).abs() < 4.0
                                && (f32::from(down.y - event.position.y)).abs() < 4.0
                        });
                        if clicked && bounds.contains(&event.position) {
                            let target = reader.current_page.as_ref().and_then(|page| {
                                link_at(
                                    page,
                                    reader.chapters.get(&reader.href)?,
                                    event.position - origin,
                                )
                            });
                            reader.selection = None;
                            if let Some(target) = target {
                                reader.follow_link(target, cx);
                            }
                        }
                        cx.notify();
                    });
                });
            },
        )
        .size_full()
        .cursor(CursorStyle::IBeam)
        .into_any_element()
    }

    fn button(
        &self,
        id: &'static str,
        label: String,
        enabled: bool,
        action: Control,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .h(px(26.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .flex_shrink_0()
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .text_color(if enabled {
                self.color()
            } else {
                rgb(0x808080).into()
            })
            .when(enabled, |button| {
                button.hover(|style| {
                    style.bg(rgb(if self.preferences.light {
                        0xdadada
                    } else {
                        0x383838
                    }))
                })
            })
            .on_click(cx.listener(move |reader, _, _, cx| {
                if !enabled {
                    return;
                }
                match action {
                    Control::Previous => reader.turn(-1, cx),
                    Control::Next => reader.turn(1, cx),
                    Control::Chapters => {
                        reader.chapters_open = !reader.chapters_open;
                        cx.notify();
                    }
                    Control::Smaller => reader.change_font(-2, cx),
                    Control::Larger => reader.change_font(2, cx),
                    Control::Theme => reader.toggle_theme(cx),
                }
            }))
            .child(label)
            .into_any_element()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let ready = self.current_page.is_some();
        let index = self.book.as_ref().and_then(|book| {
            book.sections
                .iter()
                .position(|section| section.href == self.href)
        });
        let previous = ready && !self.at_boundary(false);
        let next = ready && !self.at_boundary(true);
        let progress = match (&self.book, index) {
            (Some(book), Some(_)) if book.is_cover(&self.href) => "Cover".into(),
            (Some(book), Some(index)) => format!(
                "Chapter {}/{} · Page {} · {}",
                book.sections[..=index]
                    .iter()
                    .filter(|section| !book.is_cover(&section.href))
                    .count(),
                book.sections
                    .iter()
                    .filter(|section| !book.is_cover(&section.href))
                    .count(),
                self.page_index + 1,
                book.sections[index].title
            ),
            (Some(_), None) => format!("Supplement · Page {}", self.page_index + 1),
            _ => String::new(),
        };
        div()
            .h(px(34.0))
            .w_full()
            .flex()
            .items_center()
            .px(px(4.0))
            .flex_shrink_0()
            .overflow_hidden()
            .text_size(px(12.0))
            .text_color(self.color())
            .bg(rgb(if self.preferences.light {
                0xeeeeee
            } else {
                0x252525
            }))
            .child(self.button("epub-previous", "‹".into(), previous, Control::Previous, cx))
            .child(self.button("epub-next", "›".into(), next, Control::Next, cx))
            .child(self.button(
                "epub-chapters",
                "Chapters".into(),
                self.book.is_some(),
                Control::Chapters,
                cx,
            ))
            .child(self.button(
                "epub-smaller",
                "A−".into(),
                self.preferences.font_size > 12,
                Control::Smaller,
                cx,
            ))
            .child(
                div()
                    .flex_shrink_0()
                    .child(self.preferences.font_size.to_string()),
            )
            .child(self.button(
                "epub-larger",
                "A+".into(),
                self.preferences.font_size < 36,
                Control::Larger,
                cx,
            ))
            .child(
                self.button(
                    "epub-theme",
                    if self.preferences.light {
                        "Dark"
                    } else {
                        "Light"
                    }
                    .into(),
                    true,
                    Control::Theme,
                    cx,
                ),
            )
            .child(
                div()
                    .px(px(8.0))
                    .min_w(px(0.0))
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(progress),
            )
            .into_any_element()
    }

    fn render_chapters(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = Vec::new();
        if let Some(book) = &self.book {
            for (index, item) in book.toc.iter().enumerate() {
                let target = item.target.clone();
                rows.push(
                    div()
                        .id(("epub-toc", index))
                        .w_full()
                        .min_h(px(30.0))
                        .pl(px(12.0 + item.depth.min(12) as f32 * 16.0))
                        .pr(px(12.0))
                        .py(px(6.0))
                        .cursor(if target.is_some() {
                            CursorStyle::PointingHand
                        } else {
                            CursorStyle::Arrow
                        })
                        .hover(|style| {
                            style.bg(rgb(if self.preferences.light {
                                0xdadada
                            } else {
                                0x383838
                            }))
                        })
                        .on_click(cx.listener(move |reader, _, _, cx| {
                            if let Some(target) = &target {
                                reader.follow_link(target.clone(), cx);
                            }
                        }))
                        .child(item.title.clone())
                        .into_any_element(),
                );
            }
        }
        div()
            .id("epub-chapter-popup")
            .debug_selector(|| "epub-chapter-popup".to_owned())
            .absolute()
            .left(px(12.0))
            .bottom(px(40.0))
            .w(px(320.0))
            .max_h(px(360.0))
            .overflow_y_scroll()
            .occlude()
            .bg(rgb(if self.preferences.light {
                0xf5f5f5
            } else {
                0x252525
            }))
            .text_color(self.color())
            .text_size(px(14.0))
            .border_1()
            .border_color(rgb(0x808080))
            .shadow_md()
            .on_mouse_down_out(cx.listener(|reader, _, _, cx| {
                reader.chapters_open = false;
                cx.notify();
            }))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .children(rows)
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum Control {
    Previous,
    Next,
    Chapters,
    Smaller,
    Larger,
    Theme,
}

impl Render for Reader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loading =
            self.load_task.is_some() || self.chapter_task.is_some() || self.request.is_some();
        let body = div()
            .id("epub-body")
            .debug_selector(|| "epub-body".to_owned())
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(self.render_surface(cx))
            .when(loading && self.error.is_none(), |body| {
                body.child(div().absolute().top(px(0.0)).left(px(0.0)).w_full().child(
                    linear_indeterminate(
                        "epub-loading",
                        LinearProgressStyle {
                            color: 0x62a6ff,
                            track_color: if self.preferences.light {
                                0xdddddd
                            } else {
                                0x333333
                            },
                            height: 3.0,
                        },
                    ),
                ))
            })
            .when(self.error.is_some(), |body| {
                body.child(
                    div()
                        .absolute()
                        .inset_0()
                        .p(px(32.0))
                        .child(self.error.clone().unwrap_or_default()),
                )
            })
            .when(
                loading && self.current_page.is_none() && self.error.is_none(),
                |body| {
                    body.child(
                        div()
                            .absolute()
                            .top(px(32.0))
                            .left(px(48.0))
                            .child("Loading book…"),
                    )
                },
            );
        let content = div()
            .key_context("EpubReader")
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(if self.preferences.light {
                0xfaf9f6
            } else {
                0x1c1c1c
            }))
            .text_color(self.color())
            .on_action(cx.listener(|reader, _: &EpubNext, _, cx| reader.turn(1, cx)))
            .on_action(cx.listener(|reader, _: &EpubPrevious, _, cx| reader.turn(-1, cx)))
            .on_action(cx.listener(|reader, _: &EpubBeginning, _, cx| reader.boundary(false, cx)))
            .on_action(cx.listener(|reader, _: &EpubEnd, _, cx| reader.boundary(true, cx)))
            .on_action(cx.listener(|reader, _: &EpubBack, _, cx| reader.back(cx)))
            .on_action(cx.listener(|reader, _: &EpubCopy, _, cx| reader.copy(cx)))
            .on_action(cx.listener(|reader, _: &EpubDismiss, _, cx| {
                reader.chapters_open = false;
                reader.clear_selection();
                cx.notify();
            }))
            .child(self.render_titlebar(window, cx))
            .child(body)
            .child(self.render_toolbar(cx))
            .when(self.chapters_open, |content| {
                content.child(self.render_chapters(cx))
            })
            .into_any_element();
        render_platform_window_frame(content, window)
    }
}

fn hit_point(page: &Page, position: Point<Pixels>) -> Option<ContentPoint> {
    let mut nearest = None;
    let mut distance = f32::MAX;
    for item in &page.items {
        if let PageItem::Text {
            block,
            range,
            line,
            x,
            y,
            height,
        } = item
        {
            let py = f32::from(position.y);
            let delta = if py < *y {
                *y - py
            } else if py > y + height {
                py - y - height
            } else {
                0.0
            };
            if delta < distance {
                distance = delta;
                let offset = line
                    .closest_index_for_x(position.x - px(*x))
                    .min(range.len());
                nearest = Some(ContentPoint {
                    block: *block,
                    offset: range.start + offset,
                });
            }
        }
    }
    nearest
}

fn link_at(page: &Page, chapter: &Chapter, position: Point<Pixels>) -> Option<String> {
    for item in &page.items {
        if let PageItem::Text {
            block,
            range,
            line,
            x,
            y,
            height,
        } = item
        {
            if position.y < px(*y)
                || position.y >= px(y + height)
                || position.x < px(*x)
                || position.x > px(*x) + line.width
            {
                continue;
            }
            let index = range.start + line.index_for_x(position.x - px(*x))?;
            if let Block::Text { spans, .. } = &chapter.blocks[*block] {
                return spans
                    .iter()
                    .find(|span| span.range.contains(&index))
                    .and_then(|span| span.style.link.clone());
            }
        }
    }
    None
}

fn decode_image(
    book: &Book,
    source: &ImageSource,
    cancel: &AtomicBool,
) -> Result<ImageAsset, String> {
    super::book::check_cancel(cancel)?;
    let (bytes, svg, base_href) = match source {
        ImageSource::Resource(href) => {
            let (bytes, svg) = book.image_data(href)?;
            (bytes, svg, href.as_str())
        }
        ImageSource::Svg { bytes, base_href } => (bytes.clone(), true, base_href.as_str()),
    };
    let (rgba, width, height) = if svg {
        // Raster resolution is independent of the SVG's intended display size.
        let tree = decode_svg_tree(book, &bytes, base_href, cancel)?;
        let width = tree.size().width();
        let height = tree.size().height();
        let raster_size = width.max(height).ceil().clamp(1.0, 2048.0) as u32;
        (
            crate::explorer::load_svg_rgba_from_tree(&tree, raster_size, cancel)?,
            width,
            height,
        )
    } else {
        let image = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .decode()
            .map_err(|e| e.to_string())?;
        let width = image.width() as f32;
        let height = image.height() as f32;
        (image.thumbnail(2048, 2048).into_rgba8(), width, height)
    };
    super::book::check_cancel(cancel)?;
    Ok(ImageAsset {
        width,
        height,
        image: crate::image_viewer::render_image_from_rgba(rgba),
    })
}

fn decode_svg_tree(
    book: &Book,
    bytes: &[u8],
    base_href: &str,
    cancel: &AtomicBool,
) -> Result<usvg::Tree, String> {
    // usvg normally treats image hrefs as local filesystem paths. EPUB images
    // must instead be resolved inside the archive, relative to this document.
    let failure = std::sync::Mutex::new(None);
    let resolve_data = |mime: &str, data: Arc<Vec<u8>>, options: &usvg::Options| {
        let result = (|| {
            super::book::check_cancel(cancel)?;
            let kind = usvg::ImageHrefResolver::default_data_resolver()(mime, data, options)
                .ok_or_else(|| "Unsupported SVG image resource.".to_owned())?;
            // usvg can discard broken images without failing the whole SVG.
            // Validate raster bytes so a broken cover reaches the reader's
            // existing fallback rather than becoming a successful blank image.
            match &kind {
                usvg::ImageKind::JPEG(data)
                | usvg::ImageKind::PNG(data)
                | usvg::ImageKind::GIF(data)
                | usvg::ImageKind::WEBP(data) => {
                    image::load_from_memory(data).map_err(|error| error.to_string())?;
                }
                usvg::ImageKind::SVG(_) => {}
            }
            super::book::check_cancel(cancel)?;
            Ok::<_, String>(kind)
        })();
        match result {
            Ok(kind) => Some(kind),
            Err(error) => {
                *failure.lock().unwrap() = Some(error);
                None
            }
        }
    };
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(&resolve_data),
            resolve_string: Box::new(|reference, options| {
                let result = super::book::resolve_href(base_href, reference)
                    .ok_or_else(|| "SVG image reference is outside the EPUB.".to_owned())
                    .and_then(|href| book.image_data(&href));
                match result {
                    Ok((bytes, svg)) => resolve_data(
                        if svg { "image/svg+xml" } else { "text/plain" },
                        Arc::new(bytes),
                        options,
                    ),
                    Err(error) => {
                        *failure.lock().unwrap() = Some(error);
                        None
                    }
                }
            }),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|error| error.to_string())?;
    super::book::check_cancel(cancel)?;
    if let Some(error) = failure.lock().unwrap().take() {
        return Err(error);
    }
    Ok(tree)
}

#[cfg(target_os = "windows")]
fn ordinary_lines_per_notch() -> f32 {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut lines = 3u32;
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some((&mut lines as *mut u32).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    lines.max(1) as f32
}

#[cfg(target_os = "linux")]
fn ordinary_lines_per_notch() -> f32 {
    3.0
}

#[cfg(target_os = "macos")]
fn ordinary_lines_per_notch() -> f32 {
    1.0
}

fn normalized_wheel_delta(delta: gpui::ScrollDelta) -> gpui::ScrollDelta {
    #[cfg(target_os = "macos")]
    if let gpui::ScrollDelta::Lines(delta) = delta {
        // Discrete macOS events may report multiple lines for a wheel notch.
        // Precise trackpad deltas retain their measured pixel magnitude.
        return gpui::ScrollDelta::Lines(point(delta.x.signum(), delta.y.signum()));
    }
    delta
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, ScrollDelta, TestAppContext};

    fn svg_cover(reference: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100%" height="100%" viewBox="0 0 800 1224" preserveAspectRatio="xMidYMid meet"><image width="800" height="1224" xlink:href="{reference}"/></svg>"#
        )
    }

    fn write_svg_cover_book(
        path: &std::path::Path,
        reference: &str,
        images: &[(&str, &str, &[u8])],
    ) {
        super::super::tests::write_book_with_guide(
            path,
            "2.0",
            "",
            &[
                (
                    "front/cover.xhtml",
                    &format!("<html><body>{}</body></html>", svg_cover(reference)),
                ),
                ("title.xhtml", "<html><body><p>Title page</p></body></html>"),
                ("text.xhtml", "<html><body><p>Main text</p></body></html>"),
            ],
            images,
            "<guide><reference type='cover' title='Cover' href='front/cover.xhtml'/></guide>",
        );
    }

    fn assert_red_cover(asset: &ImageAsset) {
        assert!((asset.width / asset.height - 800.0 / 1224.0).abs() < 0.001);
        let dimensions = asset.image.size(0);
        assert!(dimensions.width.0 <= 2048 && dimensions.height.0 <= 2048);
        let pixels = asset.image.as_bytes(0).unwrap();
        let offset =
            ((dimensions.height.0 / 2 * dimensions.width.0 + dimensions.width.0 / 2) * 4) as usize;
        let center = &pixels[offset..offset + 4];
        // RenderImage stores BGRA. Checking color and opacity catches successful
        // SVG decodes whose referenced raster image was silently discarded.
        assert!(center[0] < 60 && center[1] < 60 && center[2] > 180);
        assert_eq!(center[3], 255);
    }

    #[test]
    fn svg_wrapped_jpeg_cover_renders_pixels_and_preserves_its_location() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            200,
            306,
            image::Rgb([200, 40, 40]),
        ))
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
        write_svg_cover_book(
            &path,
            "../images/cover.jpg",
            &[("images/cover.jpg", "image/jpeg", jpeg.get_ref())],
        );
        let cancel = AtomicBool::new(false);
        let book = Book::open(&path, &cancel).unwrap();
        assert_eq!(book.cover_href.as_deref(), Some("/book/front/cover.xhtml"));
        assert_eq!(book.sections.len(), 3);
        assert_eq!(book.sections[1].href, "/book/title.xhtml");
        assert_eq!(
            book.toc.iter().filter(|item| item.title == "Cover").count(),
            1
        );
        let chapter = book
            .chapter(book.cover_href.as_ref().unwrap(), &cancel)
            .unwrap();
        let [Block::Image { source, .. }] = chapter.blocks.as_slice() else {
            panic!("Expected the inline SVG cover")
        };
        assert_red_cover(&decode_image(&book, source, &cancel).unwrap());
        cancel.store(true, Ordering::Relaxed);
        assert!(decode_image(&book, source, &cancel).is_err());
    }

    #[test]
    fn svg_png_images_resolve_from_inline_and_standalone_document_locations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        let png = super::super::tests::cover_png();
        let standalone = svg_cover("../images/cover.png#image");
        write_svg_cover_book(
            &path,
            "../images/cover.png",
            &[
                ("images/cover.png", "image/png", &png),
                ("graphics/wrapper", "image/svg+xml", standalone.as_bytes()),
            ],
        );
        let cancel = AtomicBool::new(false);
        let book = Book::open(&path, &cancel).unwrap();
        let chapter = book
            .chapter(book.cover_href.as_ref().unwrap(), &cancel)
            .unwrap();
        let Block::Image { source, .. } = &chapter.blocks[0] else {
            panic!()
        };
        assert_red_cover(&decode_image(&book, source, &cancel).unwrap());
        assert_red_cover(
            &decode_image(
                &book,
                &ImageSource::Resource("/book/graphics/wrapper#view".into()),
                &cancel,
            )
            .unwrap(),
        );
        let data_url = format!(
            "data:image/png,{}",
            percent_encoding::percent_encode(&png, percent_encoding::NON_ALPHANUMERIC)
        );
        assert_red_cover(
            &decode_image(
                &book,
                &ImageSource::Svg {
                    bytes: svg_cover(&data_url).into_bytes(),
                    base_href: "/book/front/cover.xhtml".into(),
                },
                &cancel,
            )
            .unwrap(),
        );
    }

    #[test]
    fn svg_images_fail_for_missing_corrupt_and_external_resources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        let png = super::super::tests::cover_png();
        let local_image = dir.path().join("local.png");
        std::fs::write(&local_image, &png).unwrap();
        let local_url = reqwest::Url::from_file_path(&local_image).unwrap();
        // Keep a valid PNG header and truncate the pixel data, so the parser's
        // format/dimension checks alone cannot diagnose the corruption.
        write_svg_cover_book(
            &path,
            "missing.png",
            &[("broken.png", "image/png", &png[..33])],
        );
        let cancel = AtomicBool::new(false);
        let book = Book::open(&path, &cancel).unwrap();
        for reference in [
            "missing.png",
            "../broken.png",
            "https://example.com/cover.png",
            local_url.as_str(),
            local_image.to_str().unwrap(),
        ] {
            let source = ImageSource::Svg {
                bytes: svg_cover(reference).into_bytes(),
                base_href: "/book/front/cover.xhtml".into(),
            };
            assert!(
                decode_image(&book, &source, &cancel).is_err(),
                "{reference}"
            );
        }
    }

    #[test]
    fn svg_images_use_manifest_types_and_allow_fragment_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        super::super::tests::write_book(
            &path,
            "3.0",
            "",
            &[("c.xhtml", "<html><body><img src='diagram#view'/></body></html>")],
            &[("diagram", "image/svg+xml", br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20"/></svg>"#)],
        );
        let cancel = AtomicBool::new(false);
        let book = Book::open(&path, &cancel).unwrap();
        let asset = decode_image(
            &book,
            &ImageSource::Resource("/book/diagram#view".into()),
            &cancel,
        )
        .unwrap();
        assert_eq!(asset.width / asset.height, 2.0);
        assert!(
            decode_image(
                &book,
                &ImageSource::Resource("/book/missing.png".into()),
                &cancel
            )
            .is_err()
        );
    }

    fn settle(reader: &gpui::Entity<Reader>, cx: &mut gpui::VisualTestContext) {
        // The test platform has no native frame callback. Draw the successive
        // frames used for incremental layout and transitions through empty sections.
        for _ in 0..256 {
            cx.run_until_parked();
            let ready = reader.update(cx, |reader, _| {
                reader.request.is_none()
                    && reader.chapter_task.is_none()
                    && reader.load_task.is_none()
                    && reader.image_task.is_none()
            });
            if ready {
                return;
            }
            cx.update(|window, app| {
                window.refresh();
                let _ = window.draw(app);
            });
        }
        panic!("Reader did not finish navigation");
    }

    fn fixture(path: &std::path::Path) {
        let first = format!(
            "<html><body><p>{}</p><p id='finish'>Chapter end</p></body></html>",
            "A readable sentence with café and 字. ".repeat(800)
        );
        let second = format!(
            "<html><body><p>{}</p></body></html>",
            "Second chapter text. ".repeat(200)
        );
        super::super::tests::write_book(
            path,
            "3.0",
            "",
            &[
                ("first.xhtml", &first),
                ("empty.xhtml", "<html><body/></html>"),
                ("second.xhtml", &second),
            ],
            &[(
                "notes.xhtml",
                "application/xhtml+xml",
                b"<html><body><p id='note'>A note</p></body></html>",
            )],
        );
    }

    #[gpui::test]
    fn empty_edge_sections_do_not_add_blank_pages(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        super::super::tests::write_book(
            &path,
            "3.0",
            "",
            &[
                ("first.xhtml", "<html><body/></html>"),
                (
                    "c.xhtml",
                    "<html><body><p>The only readable page.</p></body></html>",
                ),
                ("last.xhtml", "<html><body/></html>"),
            ],
            &[],
        );
        let (reader, cx) =
            cx.add_window_view(|window, cx| Reader::new(path, "fixture".into(), window, cx));
        settle(&reader, cx);
        let location = reader.update(cx, |reader, _| {
            assert_eq!(reader.href, "/book/c.xhtml");
            reader.selection = Some((
                ContentPoint::default(),
                ContentPoint {
                    block: 0,
                    offset: 3,
                },
            ));
            reader.anchor
        });
        cx.dispatch_action(EpubPrevious);
        settle(&reader, cx);
        cx.dispatch_action(EpubNext);
        settle(&reader, cx);
        reader.update(cx, |reader, _| {
            assert_eq!(reader.href, "/book/c.xhtml");
            assert_eq!(reader.anchor, location);
            assert!(reader.selection.is_some());
        });
        cx.dispatch_action(EpubEnd);
        settle(&reader, cx);
        reader.update(cx, |reader, _| assert_eq!(reader.href, "/book/c.xhtml"));
        cx.dispatch_action(EpubBeginning);
        settle(&reader, cx);
        reader.update(cx, |reader, _| assert_eq!(reader.href, "/book/c.xhtml"));
    }

    #[gpui::test]
    fn window_wheel_paging_clamps_boundaries_and_skips_empty_sections(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        fixture(&path);
        let (reader, cx) =
            cx.add_window_view(|window, cx| Reader::new(path, "fixture".into(), window, cx));
        assert_eq!(
            cx.window_title().as_deref(),
            Some("Reader fixture — Explorer")
        );
        let body = cx.debug_bounds("epub-body").unwrap();
        let start = reader.update(cx, |reader, _| {
            assert!(reader.error.is_none(), "{:?}", reader.error);
            reader.anchor
        });
        cx.simulate_event(ScrollWheelEvent {
            position: body.center(),
            delta: ScrollDelta::Lines(point(0.0, -ordinary_lines_per_notch())),
            ..Default::default()
        });
        reader.update(cx, |reader, _| {
            assert_eq!(reader.page_index, 1);
            assert!(reader.anchor > start);
        });
        reader.update(cx, |reader, cx| {
            reader.chapters_open = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, app| {
            window.refresh();
            let _ = window.draw(app);
        });
        let popup = cx.debug_bounds("epub-chapter-popup").unwrap();
        cx.simulate_event(ScrollWheelEvent {
            position: popup.center(),
            delta: ScrollDelta::Lines(point(0.0, -ordinary_lines_per_notch())),
            ..Default::default()
        });
        reader.update(cx, |reader, _| assert_eq!(reader.page_index, 1));
        cx.simulate_event(ScrollWheelEvent {
            position: body.origin + point(body.size.width - px(20.0), px(20.0)),
            delta: ScrollDelta::Lines(point(0.0, -ordinary_lines_per_notch())),
            ..Default::default()
        });
        reader.update(cx, |reader, cx| {
            assert_eq!(reader.page_index, 2);
            reader.chapters_open = false;
            cx.notify();
        });
        cx.dispatch_action(EpubBeginning);
        cx.run_until_parked();
        reader.update(cx, |reader, _| {
            assert_eq!(reader.page_index, 0);
            reader.selection = Some((
                ContentPoint::default(),
                ContentPoint {
                    block: 0,
                    offset: 4,
                },
            ));
        });
        cx.dispatch_action(EpubPrevious);
        cx.run_until_parked();
        reader.update(cx, |reader, _| {
            assert_eq!(reader.anchor, start);
            assert!(reader.selection.is_some(), "boundary must do nothing");
        });
        cx.dispatch_action(EpubEnd);
        cx.run_until_parked();
        let last = reader.update(cx, |reader, _| {
            assert_eq!(reader.href, "/book/second.xhtml");
            let page = reader.current_page.as_ref().unwrap();
            assert_eq!(page.end.block, reader.chapters[&reader.href].blocks.len());
            reader.anchor
        });
        cx.dispatch_action(EpubNext);
        cx.run_until_parked();
        reader.update(cx, |reader, _| assert_eq!(reader.anchor, last));
        reader.update(cx, |reader, cx| {
            reader.open_section("/book/second.xhtml".into(), PageRequest::Index(0), None, cx)
        });
        cx.run_until_parked();
        reader.update(cx, |reader, cx| reader.turn(-3, cx));
        settle(&reader, cx);
        reader.update(cx, |reader, _| {
            assert_eq!(reader.href, "/book/first.xhtml");
            let count = reader.layouts[&reader.href].starts.len();
            assert_eq!(reader.page_index, count - 3);
        });
    }

    #[gpui::test]
    fn links_reflow_themes_and_cached_page_turns_preserve_locations(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        fixture(&path);
        let (reader, cx) =
            cx.add_window_view(|window, cx| Reader::new(path, "fixture".into(), window, cx));
        cx.dispatch_action(EpubNext);
        cx.run_until_parked();
        let location = reader.update(cx, |reader, _| Location {
            href: reader.href.clone(),
            point: reader.anchor,
        });
        reader.update(cx, |reader, cx| {
            reader.follow_link("/book/notes.xhtml#note".into(), cx)
        });
        cx.run_until_parked();
        reader.update(cx, |reader, _| assert_eq!(reader.href, "/book/notes.xhtml"));
        cx.dispatch_action(EpubBack);
        cx.run_until_parked();
        reader.update(cx, |reader, _| {
            assert_eq!(reader.href, location.href);
            assert_eq!(reader.anchor, location.point);
        });
        reader.update(cx, |reader, cx| reader.change_font(2, cx));
        cx.run_until_parked();
        reader.update(cx, |reader, _| assert_eq!(reader.anchor, location.point));
        cx.simulate_resize(size(px(480.0), px(400.0)));
        cx.run_until_parked();
        reader.update(cx, |reader, _| assert_eq!(reader.anchor, location.point));
        cx.simulate_resize(size(px(1200.0), px(900.0)));
        cx.run_until_parked();
        reader.update(cx, |reader, _| assert_eq!(reader.anchor, location.point));
        let old_page = reader.update(cx, |reader, cx| {
            let page = reader.current_page.clone().unwrap();
            reader.toggle_theme(cx);
            page
        });
        cx.run_until_parked();
        reader.update(cx, |reader, _| {
            assert_eq!(reader.current_page.as_ref().unwrap().start, old_page.start);
            assert_eq!(reader.current_page.as_ref().unwrap().end, old_page.end);
        });
        // A page turn must work after the archive is removed: no file read or
        // chapter parse is needed for cached content.
        std::fs::remove_file(dir.path().join("book.epub")).unwrap_or(());
        cx.dispatch_action(EpubNext);
        cx.run_until_parked();
        reader.update(cx, |reader, _| {
            assert!(reader.error.is_none());
            assert_eq!(reader.current_page.as_ref().unwrap().start, old_page.end);
        });
    }

    #[gpui::test]
    fn reopening_and_multiple_windows_share_saved_positions_and_preferences(
        cx: &mut TestAppContext,
    ) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        fixture(&path);
        let other_path = dir.path().join("other.epub");
        fixture(&other_path);
        let location = {
            let (reader, visual_cx) = cx.add_window_view(|window, cx| {
                Reader::new(path.clone(), "fixture".into(), window, cx)
            });
            visual_cx.dispatch_action(EpubNext);
            settle(&reader, visual_cx);
            reader.update(visual_cx, |reader, cx| {
                reader.change_font(2, cx);
                reader.toggle_theme(cx);
            });
            settle(&reader, visual_cx);
            let location = reader.update(visual_cx, |reader, _| Location {
                href: reader.href.clone(),
                point: reader.anchor,
            });
            visual_cx.update(|window, _| window.remove_window());
            location
        };
        {
            let (reader, visual_cx) = cx
                .add_window_view(|window, cx| Reader::new(other_path, "other".into(), window, cx));
            settle(&reader, visual_cx);
            reader.update(visual_cx, |reader, _| {
                assert_eq!(reader.anchor, ContentPoint::default());
                assert_eq!(reader.preferences.font_size, 22);
                assert!(reader.preferences.light);
            });
            visual_cx.dispatch_action(EpubEnd);
            settle(&reader, visual_cx);
        }
        let (reader, visual_cx) =
            cx.add_window_view(|window, cx| Reader::new(path, "reopened".into(), window, cx));
        visual_cx.simulate_resize(size(px(480.0), px(400.0)));
        settle(&reader, visual_cx);
        reader.update(visual_cx, |reader, _| {
            assert_eq!(reader.href, location.href);
            assert_eq!(reader.anchor, location.point);
            assert_eq!(reader.preferences.font_size, 22);
            assert!(reader.preferences.light);
        });
    }

    #[gpui::test]
    fn dragging_selects_visible_text_and_copy_uses_the_clipboard(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        fixture(&path);
        let (reader, cx) =
            cx.add_window_view(|window, cx| Reader::new(path, "fixture".into(), window, cx));
        let bounds = cx.debug_bounds("epub-body").unwrap();
        let geometry =
            PageGeometry::new(f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (a, b) = reader.update(cx, |reader, _| {
            let PageItem::Text { line, x, y, .. } = &reader.current_page.as_ref().unwrap().items[0]
            else {
                panic!()
            };
            let origin =
                bounds.origin + point(px(geometry.margin_x + x), px(geometry.margin_y + y + 8.0));
            (origin, origin + point(line.x_for_index(10), px(0.0)))
        });
        cx.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(b, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(b, MouseButton::Left, Modifiers::default());
        cx.dispatch_action(EpubCopy);
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("A readable")
        );
        cx.dispatch_action(EpubNext);
        cx.run_until_parked();
        reader.update(cx, |reader, _| assert!(reader.selection.is_none()));
    }

    #[gpui::test]
    fn cover_pages_fit_navigate_and_resume_without_skipping_front_matter(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        for (name, mime, bytes) in [
            (
                "cover.svg",
                "image/svg+xml",
                super::super::tests::COVER_SVG.to_vec(),
            ),
            ("cover.png", "image/png", super::super::tests::cover_png()),
        ] {
            let path = dir.path().join(format!("{name}.epub"));
            super::super::tests::write_book(
                &path,
                "3.0",
                "",
                &[
                    ("title.xhtml", "<html><body><p>Title page</p></body></html>"),
                    ("text.xhtml", "<html><body><p>Main text</p></body></html>"),
                ],
                &[(name, mime, &bytes)],
            );
            {
                let (reader, visual_cx) = cx.add_window_view(|window, cx| {
                    Reader::new(path.clone(), "cover".into(), window, cx)
                });
                visual_cx.simulate_resize(size(px(1600.0), px(900.0)));
                settle(&reader, visual_cx);
                let body = visual_cx.debug_bounds("epub-body").unwrap();
                let geometry =
                    PageGeometry::new(f32::from(body.size.width), f32::from(body.size.height));
                reader.update(visual_cx, |reader, _| {
                    assert_eq!(reader.href, "explorer:epub-cover");
                    assert!(reader.at_boundary(false));
                    assert!(!reader.at_boundary(true));
                    let PageItem::Image {
                        x,
                        y,
                        width,
                        height,
                        asset,
                        ..
                    } = &reader.current_page.as_ref().unwrap().items[0]
                    else {
                        panic!()
                    };
                    assert!(asset.is_some());
                    assert!((*width / *height - 2.0 / 3.0).abs() < 0.001);
                    assert_eq!(*width, 400.0);
                    assert_eq!(*height, 600.0);
                    assert_eq!(*x, (geometry.width - width) * 0.5);
                    assert_eq!(*y, (geometry.height - height) * 0.5);
                });
                visual_cx.update(|window, _| window.remove_window());
            }
            {
                let (reader, visual_cx) = cx.add_window_view(|window, cx| {
                    Reader::new(path.clone(), "reopened".into(), window, cx)
                });
                settle(&reader, visual_cx);
                reader.update(visual_cx, |reader, _| {
                    assert_eq!(reader.href, "explorer:epub-cover")
                });
                let body = visual_cx.debug_bounds("epub-body").unwrap();
                visual_cx.simulate_event(ScrollWheelEvent {
                    position: body.center(),
                    delta: ScrollDelta::Lines(point(0.0, -ordinary_lines_per_notch())),
                    ..Default::default()
                });
                settle(&reader, visual_cx);
                reader.update(visual_cx, |reader, _| {
                    assert_eq!(reader.href, "/book/title.xhtml")
                });
                visual_cx.dispatch_action(EpubPrevious);
                settle(&reader, visual_cx);
                reader.update(visual_cx, |reader, _| {
                    assert_eq!(reader.href, "explorer:epub-cover")
                });
                visual_cx.dispatch_action(EpubNext);
                settle(&reader, visual_cx);
                visual_cx.dispatch_action(EpubNext);
                settle(&reader, visual_cx);
                reader.update(visual_cx, |reader, _| {
                    assert_eq!(reader.href, "/book/text.xhtml")
                });
                visual_cx.dispatch_action(EpubBeginning);
                settle(&reader, visual_cx);
                reader.update(visual_cx, |reader, _| {
                    assert_eq!(reader.href, "explorer:epub-cover")
                });
                visual_cx.dispatch_action(EpubEnd);
                settle(&reader, visual_cx);
                reader.update(visual_cx, |reader, cx| {
                    assert_eq!(reader.href, "/book/text.xhtml");
                    let href = reader.book.as_ref().unwrap().toc[0].target.clone().unwrap();
                    reader.follow_link(href, cx);
                });
                settle(&reader, visual_cx);
                reader.update(visual_cx, |reader, _| {
                    assert_eq!(reader.href, "explorer:epub-cover")
                });
                visual_cx.dispatch_action(EpubBack);
                settle(&reader, visual_cx);
                reader.update(visual_cx, |reader, _| {
                    assert_eq!(reader.href, "/book/text.xhtml")
                });
                visual_cx.update(|window, _| window.remove_window());
            }
            let (reader, visual_cx) =
                cx.add_window_view(|window, cx| Reader::new(path, "resumed".into(), window, cx));
            settle(&reader, visual_cx);
            reader.update(visual_cx, |reader, _| {
                assert_eq!(reader.href, "/book/text.xhtml")
            });
            visual_cx.update(|window, _| window.remove_window());
        }
    }

    #[gpui::test]
    fn svg_wrapped_covers_fit_resize_navigate_and_resume(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        let png = super::super::tests::cover_png();
        write_svg_cover_book(
            &path,
            "../images/cover.png",
            &[("images/cover.png", "image/png", &png)],
        );
        let (reader, visual_cx) = cx.add_window_view(|window, cx| {
            Reader::new(path.clone(), "SVG cover".into(), window, cx)
        });
        for dimensions in [size(px(1600.0), px(900.0)), size(px(480.0), px(400.0))] {
            visual_cx.simulate_resize(dimensions);
            settle(&reader, visual_cx);
            let body = visual_cx.debug_bounds("epub-body").unwrap();
            let geometry =
                PageGeometry::new(f32::from(body.size.width), f32::from(body.size.height));
            reader.update(visual_cx, |reader, _| {
                assert_eq!(reader.href, "/book/front/cover.xhtml");
                let PageItem::Image {
                    width,
                    height,
                    asset,
                    ..
                } = &reader.current_page.as_ref().unwrap().items[0]
                else {
                    panic!()
                };
                assert!(*width <= geometry.width && *height <= geometry.height);
                assert!((*width / *height - 800.0 / 1224.0).abs() < 0.001);
                assert_red_cover(asset.as_ref().unwrap());
            });
        }
        visual_cx.dispatch_action(EpubNext);
        settle(&reader, visual_cx);
        reader.update(visual_cx, |reader, _| {
            assert_eq!(reader.href, "/book/title.xhtml")
        });
        visual_cx.dispatch_action(EpubPrevious);
        settle(&reader, visual_cx);
        reader.update(visual_cx, |reader, _| {
            assert_eq!(reader.href, "/book/front/cover.xhtml")
        });
        visual_cx.dispatch_action(EpubNext);
        settle(&reader, visual_cx);
        visual_cx.update(|window, _| window.remove_window());
        let (reader, visual_cx) = cx.add_window_view(|window, cx| {
            Reader::new(path, "Resumed SVG cover".into(), window, cx)
        });
        settle(&reader, visual_cx);
        reader.update(visual_cx, |reader, _| {
            assert_eq!(reader.href, "/book/title.xhtml")
        });
        visual_cx.dispatch_action(EpubBeginning);
        settle(&reader, visual_cx);
        reader.update(visual_cx, |reader, _| {
            assert_eq!(reader.href, "/book/front/cover.xhtml")
        });
        visual_cx.update(|window, _| window.remove_window());
    }

    #[gpui::test]
    fn broken_cover_images_fall_through_to_text_and_home_stays_usable(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let missing = svg_cover("missing.png");
        let corrupt = svg_cover("broken.png");
        let png = super::super::tests::cover_png();
        for (name, mime, bytes) in [
            ("cover.png", "image/png", b"broken image".as_slice()),
            ("cover.svg", "image/svg+xml", b"broken image".as_slice()),
            ("cover.svg", "image/svg+xml", missing.as_bytes()),
            ("cover.svg", "image/svg+xml", corrupt.as_bytes()),
        ] {
            let path = dir.path().join(format!("{name}.epub"));
            super::super::tests::write_book(
                &path,
                "3.0",
                "",
                &[("text.xhtml", "<html><body><p>Main text</p></body></html>")],
                &[(name, mime, bytes), ("broken.png", "image/png", &png[..33])],
            );
            let (reader, visual_cx) = cx
                .add_window_view(|window, cx| Reader::new(path, "broken cover".into(), window, cx));
            settle(&reader, visual_cx);
            reader.update(visual_cx, |reader, _| {
                assert_eq!(reader.href, "/book/text.xhtml");
                assert!(reader.error.is_none());
                assert!(reader.at_boundary(false));
            });
            visual_cx.dispatch_action(EpubBeginning);
            settle(&reader, visual_cx);
            reader.update(visual_cx, |reader, cx| {
                assert_eq!(reader.href, "/book/text.xhtml");
                reader.follow_link("explorer:epub-cover".into(), cx);
            });
            settle(&reader, visual_cx);
            reader.update(visual_cx, |reader, _| {
                assert_eq!(reader.href, "/book/text.xhtml");
                assert!(reader.error.is_none());
            });
            visual_cx.update(|window, _| window.remove_window());
        }
    }

    #[gpui::test]
    fn justified_selection_and_links_follow_expanded_glyph_positions(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        let prose = format!(
            "<html><body><p>café <b>bold</b> <a href='notes.xhtml#note'>linked words</a> {}</p></body></html>",
            "More readable prose. ".repeat(100)
        );
        super::super::tests::write_book(
            &path,
            "3.0",
            "",
            &[("text.xhtml", &prose)],
            &[(
                "notes.xhtml",
                "application/xhtml+xml",
                b"<html><body><p id='note'>A note</p></body></html>",
            )],
        );
        let (reader, visual_cx) =
            cx.add_window_view(|window, cx| Reader::new(path, "justified".into(), window, cx));
        visual_cx.simulate_resize(size(px(1600.0), px(900.0)));
        settle(&reader, visual_cx);
        let bounds = visual_cx.debug_bounds("epub-body").unwrap();
        let geometry =
            PageGeometry::new(f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (a, b, link) = reader.update(visual_cx, |reader, _| {
            let page = reader.current_page.as_ref().unwrap();
            let PageItem::Text { line, x, y, .. } = &page.items[0] else {
                panic!()
            };
            assert_eq!(line.width, px(900.0));
            let origin =
                bounds.origin + point(px(geometry.margin_x + x), px(geometry.margin_y + y + 8.0));
            let end = "café bold linked words".len();
            let link = origin + point(line.x_for_index("café bold ".len()) + px(1.0), px(0.0));
            assert_eq!(
                link_at(
                    page,
                    &reader.chapters[&reader.href],
                    link - bounds.origin - point(px(geometry.margin_x), px(geometry.margin_y))
                ),
                Some("/book/notes.xhtml#note".into())
            );
            (origin, origin + point(line.x_for_index(end), px(0.0)), link)
        });
        visual_cx.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual_cx.simulate_mouse_move(b, MouseButton::Left, Modifiers::default());
        visual_cx.simulate_mouse_up(b, MouseButton::Left, Modifiers::default());
        visual_cx.dispatch_action(EpubCopy);
        visual_cx.run_until_parked();
        assert_eq!(
            visual_cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("café bold linked words")
        );
        visual_cx.simulate_click(link, Modifiers::default());
        settle(&reader, visual_cx);
        reader.update(visual_cx, |reader, _| {
            assert_eq!(reader.href, "/book/notes.xhtml")
        });
    }
}
