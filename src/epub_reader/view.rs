use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, Entity,
    FocusHandle, Focusable, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ScrollHandle, ScrollWheelEvent, Task, TitlebarOptions, Window,
    WindowBounds, WindowDecorations, WindowOptions, canvas, div, fill, point, prelude::*, px, rgb,
    size,
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
    EpubActivate, EpubBack, EpubBeginning, EpubBookmark, EpubContents, EpubCopy, EpubDismiss,
    EpubEnd, EpubFind, EpubFocusNext, EpubFocusPrevious, EpubFullscreen, EpubNext, EpubPrevious,
    EpubSettings,
    book::{Block, Book, Chapter, ContentPoint, ImageSource},
    input::{InputEvent, ReaderInput},
    pagination::{
        ImageAsset, ImageSlot, Page, PageGeometry, PageItem, WheelPager,
        layout_page_with_preferences, normalize_point, selected_text,
    },
    search::{BookIndex, SearchMatch},
    state::{self, Bookmark, Fingerprint, Location, Preferences},
    theme::ReaderPalette,
};

const WINDOW_STATE_OPTIONS: WindowStateOptions = WindowStateOptions {
    min_width: 360.0,
    min_height: 240.0,
    include_fullscreen: true,
};
const PAGE_CACHE_SIZE: usize = 8;
const PANEL_INSET: f32 = 12.0;
const PANEL_WIDTH: f32 = 320.0;
const LOADING_DELAY: Duration = Duration::from_millis(200);

pub(crate) fn open_epub_window(path: PathBuf, cx: &mut App) -> Result<(), String> {
    let source_path = normalized_source_path(&path);
    for handle in cx.windows() {
        let Some(handle) = handle.downcast::<Reader>() else {
            continue;
        };
        let matches = handle
            .read(cx)
            .is_ok_and(|reader| reader.source_path == source_path);
        if matches
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            cx.activate(true);
            return Ok(());
        }
    }
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

fn normalized_source_path(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    // Resolve existing paths before removing `..`, which may follow a symlink.
    if let Ok(canonical) = std::fs::canonicalize(&absolute) {
        return canonical;
    }
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if normalized.file_name().is_some() => {
                normalized.pop();
            }
            Component::ParentDir if normalized.has_root() => {}
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[derive(Clone, Copy, Debug)]
enum PageRequest {
    Index(isize),
    FromEnd(usize),
    Anchor(ContentPoint),
}

struct ChapterLayout {
    geometry: PageGeometry,
    preferences: Preferences,
    starts: Vec<ContentPoint>,
    complete: bool,
    pages: VecDeque<(usize, Page)>,
}

impl ChapterLayout {
    fn new(geometry: PageGeometry, preferences: Preferences) -> Self {
        Self {
            geometry,
            preferences,
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
    source_path: PathBuf,
    should_move_window: bool,
    book: Option<Arc<Book>>,
    fingerprint: Option<Fingerprint>,
    resume_location: Option<Location>,
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
    settings_open: bool,
    contents_cursor: usize,
    contents_scroll: ScrollHandle,
    input: Entity<ReaderInput>,
    bookmark_input: Entity<ReaderInput>,
    search_open: bool,
    search_results: Vec<SearchMatch>,
    search_busy: bool,
    search_highlight: Option<SearchMatch>,
    search_return: Option<Location>,
    index: Option<Arc<BookIndex>>,
    index_task: Option<Task<()>>,
    search_task: Option<Task<()>>,
    search_cancel: Arc<AtomicBool>,
    bookmark_open: bool,
    note: Option<(String, String)>,
    note_task: Option<Task<()>>,
    note_cancel: Arc<AtomicBool>,
    enlarged_image: Option<ImageAsset>,
    copy_menu: Option<Point<Pixels>>,
    fullscreen: bool,
    controls_revealed: bool,
    panel_trigger_bounds: Vec<Bounds<Pixels>>,
    history: Vec<Location>,
    selection: Option<(ContentPoint, ContentPoint)>,
    selecting: bool,
    mouse_down: Option<Point<Pixels>>,
    wheel: WheelPager,
    load_cancel: Arc<AtomicBool>,
    chapter_cancel: Arc<AtomicBool>,
    loading_delay_task: Option<Task<()>>,
    loading_visible: bool,
    loading_generation: u64,
    refresh_pending: bool,
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
        let preferences = state::preferences(cx);
        let families: &[&str] = if preferences.sans {
            &[
                "Segoe UI",
                "Helvetica",
                "Noto Sans",
                "DejaVu Sans",
                "Liberation Sans",
            ]
        } else {
            &[
                "Georgia",
                "Noto Serif",
                "DejaVu Serif",
                "Liberation Serif",
                "Times New Roman",
            ]
        };
        let font_family = families
            .iter()
            .find(|family| names.iter().any(|name| name == *family))
            .map(|family| (*family).to_owned())
            .unwrap_or_else(|| window.text_style().font_family.to_string());
        let mut reader = Self {
            focus_handle,
            title,
            source_path: normalized_source_path(&path),
            should_move_window: false,
            book: None,
            fingerprint: None,
            resume_location: None,
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
            preferences,
            font_family,
            error: None,
            chapters_open: false,
            settings_open: false,
            contents_cursor: 0,
            contents_scroll: ScrollHandle::new(),
            input: cx.new(|cx| ReaderInput::new("Search this book", cx)),
            bookmark_input: cx.new(|cx| ReaderInput::new("Bookmark name", cx)),
            search_open: false,
            search_results: Vec::new(),
            search_busy: false,
            search_highlight: None,
            search_return: None,
            index: None,
            index_task: None,
            search_task: None,
            search_cancel: Arc::new(AtomicBool::new(false)),
            bookmark_open: false,
            note: None,
            note_task: None,
            note_cancel: Arc::new(AtomicBool::new(false)),
            enlarged_image: None,
            copy_menu: None,
            fullscreen: window.is_fullscreen(),
            controls_revealed: false,
            panel_trigger_bounds: Vec::new(),
            history: Vec::new(),
            selection: None,
            selecting: false,
            mouse_down: None,
            wheel: WheelPager::default(),
            load_cancel: Arc::new(AtomicBool::new(false)),
            chapter_cancel: Arc::new(AtomicBool::new(false)),
            loading_delay_task: None,
            loading_visible: false,
            loading_generation: 0,
            refresh_pending: false,
            load_task: None,
            chapter_task: None,
            prefetch_task: None,
            image_task: None,
        };
        cx.on_release(|reader, _| {
            reader.load_cancel.store(true, Ordering::Relaxed);
            reader.chapter_cancel.store(true, Ordering::Relaxed);
            reader.search_cancel.store(true, Ordering::Relaxed);
            reader.note_cancel.store(true, Ordering::Relaxed);
        })
        .detach();
        cx.subscribe_in(
            &reader.input,
            window,
            |reader, _, event: &InputEvent, window, cx| match event {
                InputEvent::Changed => reader.search(cx),
                InputEvent::Submit => {
                    if !reader.search_results.is_empty() {
                        reader.open_search_result(0, window, cx);
                    }
                }
                InputEvent::Dismiss => reader.dismiss(window, cx),
            },
        )
        .detach();
        cx.subscribe_in(
            &reader.bookmark_input,
            window,
            |reader, _, event: &InputEvent, window, cx| match event {
                InputEvent::Submit => reader.save_bookmark(window, cx),
                InputEvent::Dismiss => reader.dismiss(window, cx),
                _ => {
                    cx.notify();
                }
            },
        )
        .detach();
        #[cfg(not(test))]
        cx.observe_window_bounds(window, |reader, window, cx| {
            if reader.fullscreen != window.is_fullscreen() {
                reader.fullscreen = window.is_fullscreen();
                cx.notify();
            }
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
                    let legacy = state::book_key(&path);
                    let key = state::content_key(&path, &cancel)?;
                    let book = Book::open(&path, &cancel)?;
                    Ok::<_, String>((Arc::new(book), key, legacy, fingerprint))
                })
                .await;
            let _ = this.update_in(cx, |reader, window, cx| {
                reader.load_task = None;
                match result {
                    Ok((book, key, legacy, fingerprint)) => {
                        state::migrate(cx, &key, &legacy, &fingerprint);
                        let saved = state::resume(cx, &key, &fingerprint);
                        let location = saved
                            .filter(|location| book.readable_location(&location.href))
                            .unwrap_or_else(|| Location {
                                href: book.sections[0].href.clone(),
                                point: ContentPoint::default(),
                                ..Location::default()
                            });
                        reader.title = format!("{} — Explorer", book.title);
                        window.set_window_title(&reader.title);
                        reader.book = Some(book);
                        reader.key = key;
                        reader.fingerprint = Some(fingerprint);
                        reader.build_index(cx);
                        let href = reader
                            .book
                            .as_ref()
                            .unwrap()
                            .reading_href(&location.href)
                            .to_owned();
                        reader.resume_location = Some(location.clone());
                        reader.open_section(href, PageRequest::Anchor(location.point), None, cx);
                    }
                    Err(error) => reader.error = Some(error),
                }
                reader.sync_loading_feedback(cx);
                cx.notify();
            });
        }));
        reader.sync_loading_feedback(cx);
        reader
    }

    fn page_pending(&self) -> bool {
        self.error.is_none()
            && (self.load_task.is_some() || self.chapter_task.is_some() || self.request.is_some())
    }

    fn sync_loading_feedback(&mut self, cx: &mut Context<Self>) {
        if !self.page_pending() {
            if self.loading_delay_task.is_some() || self.loading_visible {
                self.loading_generation = self.loading_generation.wrapping_add(1);
                self.loading_delay_task = None;
                self.loading_visible = false;
            }
        } else if self.loading_delay_task.is_none() {
            self.loading_generation = self.loading_generation.wrapping_add(1);
            let generation = self.loading_generation;
            self.loading_delay_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(LOADING_DELAY).await;
                let _ = this.update(cx, |reader, cx| {
                    if reader.loading_generation != generation {
                        return;
                    }
                    if reader.page_pending() {
                        reader.loading_visible = true;
                        cx.notify();
                    } else {
                        reader.sync_loading_feedback(cx);
                    }
                });
            }));
        }
    }

    fn refresh_after_frame(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.refresh_pending {
            return;
        }
        self.refresh_pending = true;
        // Pagination changes state during canvas prepaint. GPUI deliberately
        // ignores redraw invalidation in that phase, so notify after drawing.
        cx.defer_in(window, |reader, _, cx| {
            reader.refresh_pending = false;
            cx.notify();
        });
    }

    fn color(&self) -> Hsla {
        self.palette().text
    }

    fn palette(&self) -> ReaderPalette {
        ReaderPalette::new(self.preferences.light)
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
        self.sync_loading_feedback(cx);
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
                reader.sync_loading_feedback(cx);
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
        if self.chapters_open
            || self.settings_open
            || self.search_open
            || self.bookmark_open
            || self.note.is_some()
            || self.enlarged_image.is_some()
            || self.copy_menu.is_some()
        {
            return;
        }
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
        self.sync_loading_feedback(cx);
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
        if self.chapters_open {
            self.contents_cursor = if end {
                self.book
                    .as_ref()
                    .map_or(0, |book| book.toc.len().saturating_sub(1))
            } else {
                0
            };
            self.contents_scroll.scroll_to_item(self.contents_cursor);
            cx.notify();
            return;
        }
        if self.settings_open
            || self.search_open
            || self.bookmark_open
            || self.note.is_some()
            || self.enlarged_image.is_some()
        {
            return;
        }
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
            state::remember(cx, self.key.clone(), fingerprint.clone(), self.location());
        }
    }

    fn location(&self) -> Location {
        Location::at(
            self.href.clone(),
            self.anchor,
            self.chapters.get(&self.href).map(Arc::as_ref),
        )
    }

    fn build_index(&mut self, cx: &mut Context<Self>) {
        let Some(book) = self.book.clone() else {
            return;
        };
        let cancel = self.load_cancel.clone();
        self.index_task = Some(cx.spawn(async move |this, cx| {
            let index = cx
                .background_executor()
                .spawn(async move { BookIndex::build(&book, &cancel) })
                .await;
            let _ = this.update(cx, |reader, cx| {
                reader.index_task = None;
                if let Ok(index) = index {
                    reader.index = Some(Arc::new(index));
                }
                if reader.search_open {
                    reader.search(cx);
                }
                cx.notify();
            });
        }));
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_cancel = Arc::new(AtomicBool::new(false));
        self.search_task = None;
        self.search_results.clear();
        let query = self.input.read(cx).text.content.clone();
        self.search_busy = !query.trim().is_empty();
        let Some(index) = self.index.clone() else {
            cx.notify();
            return;
        };
        if query.trim().is_empty() {
            self.search_busy = false;
            cx.notify();
            return;
        }
        let cancel = self.search_cancel.clone();
        let worker_cancel = cancel.clone();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            let results = cx
                .background_executor()
                .spawn(async move { index.search(&query, &worker_cancel) })
                .await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = this.update(cx, |reader, cx| {
                if let Ok(results) = results {
                    reader.search_results = results;
                    reader.search_busy = false;
                    reader.search_task = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn open_search_result(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(result) = self.search_results.get(index).cloned() else {
            return;
        };
        self.history.push(self.location());
        self.search_highlight = Some(result.clone());
        self.search_open = false;
        self.open_section(result.href, PageRequest::Anchor(result.start), None, cx);
        self.focus_handle.focus(window);
    }

    fn current_toc(&self) -> Option<usize> {
        self.book
            .as_ref()?
            .toc
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, item)| {
                let target = item.target.as_deref()?;
                let (href, fragment) = target
                    .split_once('#')
                    .map_or((target, None), |(href, fragment)| (href, Some(fragment)));
                if self.book.as_ref()?.reading_href(href) != self.href {
                    return None;
                }
                let applies = fragment.is_none_or(|fragment| {
                    let fragment =
                        percent_encoding::percent_decode_str(fragment).decode_utf8_lossy();
                    self.chapters
                        .get(&self.href)
                        .and_then(|chapter| chapter.anchors.get(fragment.as_ref()))
                        .is_some_and(|point| *point <= self.anchor)
                });
                applies.then_some(index)
            })
    }

    fn heading(&self) -> String {
        self.current_toc()
            .and_then(|index| self.book.as_ref().and_then(|book| book.toc.get(index)))
            .map(|item| item.title.clone())
            .or_else(|| {
                self.book
                    .as_ref()
                    .and_then(|book| {
                        book.sections
                            .iter()
                            .find(|section| section.href == self.href)
                    })
                    .map(|section| section.title.clone())
            })
            .unwrap_or_else(|| "Supplement".into())
    }

    fn clear_previews(&mut self) {
        self.note_cancel.store(true, Ordering::Relaxed);
        self.note_task = None;
        self.note = None;
        self.enlarged_image = None;
        self.copy_menu = None;
    }

    fn toggle_contents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_previews();
        self.chapters_open = !self.chapters_open;
        self.settings_open = false;
        self.search_open = false;
        self.bookmark_open = false;
        if self.chapters_open {
            self.contents_cursor = self.current_toc().unwrap_or(0);
            self.scroll_to_contents_cursor(cx);
        }
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_previews();
        if self.search_open {
            self.search_open = false;
            self.focus_handle.focus(window);
        } else {
            self.search_open = true;
            self.settings_open = false;
            self.chapters_open = false;
            self.bookmark_open = false;
            if self.search_return.is_none() {
                self.search_return = Some(self.location());
            }
            self.input.update(cx, |input, cx| {
                input.light = self.preferences.light;
                input.text.select_all();
                cx.notify();
            });
            self.input.read(cx).focus_handle(cx).focus(window);
            self.search(cx);
        }
        cx.notify();
    }

    fn open_bookmark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bookmark_open {
            self.bookmark_open = false;
            self.focus_handle.focus(window);
            cx.notify();
            return;
        }
        if self.current_page.is_none() {
            return;
        }
        self.remember(cx);
        self.clear_previews();
        self.bookmark_open = true;
        self.search_open = false;
        self.chapters_open = false;
        self.settings_open = false;
        let name = self.heading();
        self.bookmark_input.update(cx, |input, cx| {
            input.light = self.preferences.light;
            input.text = crate::explorer::text_input::EditableTextState::new(name);
            cx.notify();
        });
        self.bookmark_input.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    fn save_bookmark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.bookmark_input.read(cx).text.content.trim().to_owned();
        if name.is_empty() {
            return;
        }
        let location = self.location();
        let mut bookmarks = state::bookmarks(cx, &self.key);
        bookmarks.retain(|bookmark| {
            bookmark.location.href != location.href || bookmark.location.point != location.point
        });
        bookmarks.push(Bookmark { name, location });
        state::set_bookmarks(cx, &self.key, bookmarks);
        self.bookmark_open = false;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had_overlay = self.chapters_open
            || self.settings_open
            || self.search_open
            || self.bookmark_open
            || self.note.is_some()
            || self.enlarged_image.is_some()
            || self.copy_menu.is_some()
            || self.selection.is_some();
        self.chapters_open = false;
        self.settings_open = false;
        self.search_open = false;
        self.bookmark_open = false;
        self.note = None;
        self.enlarged_image = None;
        self.copy_menu = None;
        self.note_cancel.store(true, Ordering::Relaxed);
        self.note_task = None;
        self.clear_selection();
        self.focus_handle.focus(window);
        cx.notify();
        if !had_overlay && self.fullscreen {
            self.control(Control::Fullscreen, window, cx);
        }
    }

    fn apply_preferences(&mut self, cx: &mut Context<Self>) {
        let names = cx.text_system().all_font_names();
        let families: &[&str] = if self.preferences.sans {
            &[
                "Segoe UI",
                "Helvetica",
                "Noto Sans",
                "DejaVu Sans",
                "Liberation Sans",
            ]
        } else {
            &[
                "Georgia",
                "Noto Serif",
                "DejaVu Serif",
                "Liberation Serif",
                "Times New Roman",
            ]
        };
        if let Some(family) = families
            .iter()
            .find(|family| names.iter().any(|name| name == *family))
        {
            self.font_family = (*family).to_owned();
        }
        state::set_preferences(cx, self.preferences);
        self.request = Some(PageRequest::Anchor(self.anchor));
        self.sync_loading_feedback(cx);
        self.clear_selection();
        cx.notify();
    }

    fn contents_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.chapters_open {
            return;
        }
        let count = self.book.as_ref().map_or(0, |book| book.toc.len());
        if count == 0 {
            return;
        }
        match event.keystroke.key.as_str() {
            "up" => self.contents_cursor = self.contents_cursor.saturating_sub(1),
            "down" => self.contents_cursor = (self.contents_cursor + 1).min(count - 1),
            "enter" => {
                if let Some(target) = self
                    .book
                    .as_ref()
                    .and_then(|book| book.toc.get(self.contents_cursor))
                    .and_then(|item| item.target.clone())
                {
                    self.follow_link(target, cx);
                }
                self.focus_handle.focus(window);
            }
            _ => return,
        }
        self.scroll_to_contents_cursor(cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn scroll_to_contents_cursor(&self, cx: &App) {
        let bookmarks = state::bookmarks(cx, &self.key);
        let preceding_rows = if bookmarks.is_empty() {
            0
        } else {
            bookmarks.len() + 2
        };
        self.contents_scroll
            .scroll_to_item(preceding_rows + self.contents_cursor);
    }

    fn is_panel_trigger(&self, event: &MouseDownEvent) -> bool {
        self.panel_trigger_bounds
            .iter()
            .any(|bounds| bounds.contains(&event.position))
    }

    fn control(&mut self, action: Control, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Control::Previous => self.turn(-1, cx),
            Control::Next => self.turn(1, cx),
            Control::Chapters => self.toggle_contents(window, cx),
            Control::Smaller => self.change_font(-2, cx),
            Control::Larger => self.change_font(2, cx),
            Control::Theme => self.toggle_theme(cx),
            Control::Back => {
                if self.history.is_empty() {
                    self.adjacent_section(false, PageRequest::FromEnd(0), cx);
                } else {
                    self.back(cx);
                }
            }
            Control::Retry => {
                self.chapters.remove(&self.href);
                self.layouts.remove(&self.href);
                self.open_section(
                    self.href.clone(),
                    PageRequest::Anchor(self.anchor),
                    self.fragment.clone(),
                    cx,
                );
            }
            Control::Skip => {
                self.adjacent_section(true, PageRequest::Index(0), cx);
            }
            Control::Settings => {
                self.clear_previews();
                self.settings_open = !self.settings_open;
                self.chapters_open = false;
                self.search_open = false;
                self.bookmark_open = false;
                self.focus_handle.focus(window);
                cx.notify();
            }
            Control::Find => self.toggle_search(window, cx),
            Control::Bookmark => self.open_bookmark(window, cx),
            Control::SaveBookmark => self.save_bookmark(window, cx),
            Control::Return => {
                if let Some(location) = self.search_return.take() {
                    self.resume_location = Some(location.clone());
                    self.search_open = false;
                    self.search_highlight = None;
                    self.open_section(location.href, PageRequest::Anchor(location.point), None, cx);
                    self.focus_handle.focus(window);
                }
            }
            Control::Fullscreen => {
                window.toggle_fullscreen();
                self.fullscreen = !self.fullscreen;
                self.controls_revealed = false;
                cx.notify();
            }
            Control::Dismiss => self.dismiss(window, cx),
            Control::Copy => {
                self.copy(cx);
                self.copy_menu = None;
                cx.notify();
            }
            Control::Sans => {
                self.preferences.sans = !self.preferences.sans;
                self.apply_preferences(cx);
            }
            Control::Spacing(change) => {
                self.preferences.line_spacing =
                    (self.preferences.line_spacing as i16 + change).clamp(12, 22) as u8;
                self.apply_preferences(cx);
            }
            Control::Width(change) => {
                self.preferences.reading_width =
                    (self.preferences.reading_width as i16 + change).clamp(400, 1000) as u16;
                self.apply_preferences(cx);
            }
            Control::Alignment => {
                self.preferences.justify = !self.preferences.justify;
                self.apply_preferences(cx);
            }
            Control::Publisher => {
                self.preferences.publisher_styles = !self.preferences.publisher_styles;
                self.apply_preferences(cx);
            }
        }
    }

    fn prepare_page(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Page> {
        let before = (
            self.page_label(),
            self.current_page.is_some(),
            self.page_pending(),
            self.loading_visible,
        );
        let page = self.prepare_page_inner(bounds, window, cx);
        self.sync_loading_feedback(cx);
        let after = (
            self.page_label(),
            self.current_page.is_some(),
            self.page_pending(),
            self.loading_visible,
        );
        if before != after {
            self.refresh_after_frame(window, cx);
        }
        page
    }

    fn prepare_page_inner(
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
        let geometry = PageGeometry::with_width(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            self.preferences.reading_width as f32,
        );
        if self.layouts.get(&self.href).is_none_or(|layout| {
            layout.geometry != geometry || layout.preferences != self.preferences
        }) {
            self.layouts.insert(
                self.href.clone(),
                ChapterLayout::new(geometry, self.preferences),
            );
            if self.request.is_none() {
                self.request = Some(PageRequest::Anchor(self.anchor));
            }
            self.clear_selection();
        }
        if let Some(location) = self.resume_location.take() {
            if self
                .book
                .as_ref()
                .is_some_and(|book| book.reading_href(&location.href) == self.href)
            {
                self.request = Some(PageRequest::Anchor(location.recover(&chapter)));
            }
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
                    match layout_page_with_preferences(
                        &chapter,
                        layout.starts[index],
                        geometry,
                        &self.font_family,
                        self.preferences.font_size as f32,
                        color,
                        link_color,
                        images,
                        &self.preferences,
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
            self.warm_pages(&chapter, window, cx);
            Some(page)
        } else {
            None
        }
    }

    fn warm_pages(&mut self, chapter: &Chapter, window: &mut Window, cx: &mut Context<Self>) {
        let started = Instant::now();
        let color = self.color();
        let link_color = self.link_color();
        let Some(layout) = self.layouts.get_mut(&self.href) else {
            return;
        };
        let Some(images) = self.images.get(&self.href) else {
            return;
        };
        let was_complete = layout.complete;
        let mut pending_images = Vec::new();
        // Warm the next reading page first, then advance the chapter's page
        // boundaries without retaining every shaped page. Text shaping uses
        // Window, so this work yields between frames on the UI thread.
        let next = self.page_index + 1;
        let mut warm_next =
            next < layout.starts.len() && !layout.pages.iter().any(|(cached, _)| *cached == next);
        while warm_next || !layout.complete {
            let index = if warm_next {
                next
            } else {
                layout.starts.len() - 1
            };
            warm_next = false;
            let page = match layout_page_with_preferences(
                chapter,
                layout.starts[index],
                layout.geometry,
                &self.font_family,
                self.preferences.font_size as f32,
                color,
                link_color,
                images,
                &self.preferences,
                window,
            ) {
                Ok(page) => page,
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                    return;
                }
            };
            pending_images.extend(
                page.pending_images
                    .iter()
                    .copied()
                    .filter(|index| !images.contains_key(index)),
            );
            if !pending_images.is_empty() {
                // Image dimensions can change page breaks. Resolve these before
                // publishing boundaries for the remainder of the chapter.
                break;
            }
            if index == layout.starts.len() - 1 && !layout.complete {
                if page.end.block >= chapter.blocks.len() || page.end == page.start {
                    layout.complete = true;
                } else {
                    layout.starts.push(page.end);
                }
            }
            if index == next {
                layout.cache(index, page);
            }
            if started.elapsed().as_millis() >= 4 {
                break;
            }
        }
        let complete = layout.complete;
        if complete != was_complete || (!complete && pending_images.is_empty()) {
            self.refresh_after_frame(window, cx);
        }
        self.load_images(&pending_images, cx);
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
        self.history.push(self.location());
        self.chapters_open = false;
        self.open_section(href, PageRequest::Index(0), fragment, cx);
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        if let Some(location) = self.history.pop() {
            self.resume_location = Some(location.clone());
            self.open_section(location.href, PageRequest::Anchor(location.point), None, cx);
        }
    }

    fn open_note(&mut self, target: String, cx: &mut Context<Self>) {
        let Some((href, fragment)) = target.split_once('#') else {
            self.follow_link(target, cx);
            return;
        };
        let Some(book) = self
            .book
            .clone()
            .filter(|book| book.readable_location(href))
        else {
            return;
        };
        let href = href.to_owned();
        let fragment = percent_encoding::percent_decode_str(fragment)
            .decode_utf8_lossy()
            .into_owned();
        self.note_cancel.store(true, Ordering::Relaxed);
        self.note_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.note_cancel.clone();
        let worker_cancel = cancel.clone();
        self.note = Some(("Footnote".into(), "Loading note…".into()));
        self.note_task = Some(cx.spawn(async move |this, cx| {
            let text = cx
                .background_executor()
                .spawn(async move {
                    let chapter = book.note_chapter(&href, &worker_cancel)?;
                    let range = chapter
                        .notes
                        .get(&fragment)
                        .cloned()
                        .or_else(|| {
                            chapter
                                .anchors
                                .get(&fragment)
                                .map(|point| point.block..point.block + 1)
                        })
                        .ok_or("This note could not be found.".to_owned())?;
                    let text = chapter
                        .blocks
                        .get(range)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|block| match block {
                            Block::Text { text, .. } => Some(text.as_str()),
                            Block::Image { alt, .. } => Some(alt.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    Ok::<_, String>(text)
                })
                .await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = this.update(cx, |reader, cx| {
                reader.note_task = None;
                reader.note = Some(("Footnote".into(), text.unwrap_or_else(|error| error)));
                cx.notify();
            });
        }));
        cx.notify();
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
        self.sync_loading_feedback(cx);
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
                let geometry = PageGeometry::with_width(
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                    paint_entity.read(cx).preferences.reading_width as f32,
                );
                let origin = bounds.origin + point(px(geometry.margin_x), px(geometry.margin_y));
                let selection = paint_entity.read(cx).selection;
                let search_selection = paint_entity
                    .read(cx)
                    .search_highlight
                    .as_ref()
                    .filter(|result| result.href == paint_entity.read(cx).href)
                    .map(|result| (result.start, result.end));
                let light = paint_entity.read(cx).preferences.light;
                if let Some(page) = &page {
                    window.paint_layer(bounds, |window| {
                        for item in &page.items {
                            match item {
                                PageItem::Rule {
                                    x,
                                    y,
                                    width,
                                    height,
                                } => {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            origin + point(px(*x), px(*y)),
                                            size(px(*width), px(*height)),
                                        ),
                                        rgb(if light { 0xaaaaaa } else { 0x666666 }),
                                    ));
                                }
                                PageItem::Text {
                                    block,
                                    range,
                                    line,
                                    x,
                                    y,
                                    height,
                                } => {
                                    let position = origin + point(px(*x), px(*y));
                                    if let Some((a, b)) = selection.or(search_selection) {
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
                                                    rgb(if selection.is_none() {
                                                        if light { 0xffe8a3 } else { 0x6b5526 }
                                                    } else if light {
                                                        0xb6d8ff
                                                    } else {
                                                        0x334a70
                                                    }),
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
                        if turns != 0 {
                            reader.chapters_open = false;
                        }
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
                        if reader.settings_open
                            || reader.search_open
                            || reader.bookmark_open
                            || reader.note.is_some()
                            || reader.enlarged_image.is_some()
                        {
                            return;
                        }
                        reader.focus_handle.focus(window);
                        if event.click_count >= 2 {
                            if let Some(asset) = reader.current_page.as_ref().and_then(|page| {
                                page.items.iter().find_map(|item| {
                                    if let PageItem::Image {
                                        asset: Some(asset),
                                        x,
                                        y,
                                        width,
                                        height,
                                        ..
                                    } = item
                                    {
                                        let position = event.position - origin;
                                        if position.x >= px(*x)
                                            && position.x <= px(x + width)
                                            && position.y >= px(*y)
                                            && position.y <= px(y + height)
                                        {
                                            return Some(asset.clone());
                                        }
                                    }
                                    None
                                })
                            }) {
                                reader.enlarged_image = Some(asset);
                                cx.notify();
                                cx.stop_propagation();
                                return;
                            }
                        }
                        if let Some(point) = reader
                            .current_page
                            .as_ref()
                            .and_then(|page| hit_point(page, event.position - origin))
                        {
                            reader.selection = if event.click_count >= 3 {
                                reader
                                    .chapters
                                    .get(&reader.href)
                                    .and_then(|chapter| chapter.blocks.get(point.block))
                                    .and_then(|block| match block {
                                        Block::Text { text, .. } => Some((
                                            ContentPoint {
                                                block: point.block,
                                                offset: 0,
                                            },
                                            ContentPoint {
                                                block: point.block,
                                                offset: text.len(),
                                            },
                                        )),
                                        _ => None,
                                    })
                            } else if event.click_count == 2 {
                                reader
                                    .chapters
                                    .get(&reader.href)
                                    .and_then(|chapter| word_selection(chapter, point))
                            } else {
                                Some((point, point))
                            };
                            reader.selecting = true;
                            reader.mouse_down = (event.click_count == 1).then_some(event.position);
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
                let mouse_up_entity = paint_entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    mouse_up_entity.update(cx, |reader, cx| {
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
                                let is_note = reader
                                    .current_page
                                    .as_ref()
                                    .and_then(|page| hit_point(page, event.position - origin))
                                    .and_then(|point| {
                                        reader
                                            .chapters
                                            .get(&reader.href)
                                            .and_then(|chapter| chapter.blocks.get(point.block))
                                            .and_then(|block| match block {
                                                Block::Text { spans, .. } => spans
                                                    .iter()
                                                    .find(|span| span.range.contains(&point.offset))
                                                    .map(|span| span.style.noteref),
                                                _ => None,
                                            })
                                    })
                                    .unwrap_or(false);
                                if is_note {
                                    reader.open_note(target, cx);
                                } else {
                                    reader.follow_link(target, cx);
                                }
                            }
                        }
                        cx.notify();
                    });
                });
                let entity = paint_entity.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble
                        && event.button == MouseButton::Right
                        && bounds.contains(&event.position)
                    {
                        entity.update(cx, |reader, cx| {
                            if reader.selection.is_some_and(|(a, b)| a != b) {
                                reader.copy_menu = Some(event.position - bounds.origin);
                                cx.notify();
                                cx.stop_propagation();
                            }
                        });
                    }
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
        let palette = self.palette();
        let active = match action {
            Control::Chapters => self.chapters_open,
            Control::Settings => self.settings_open,
            Control::Find => self.search_open,
            Control::Bookmark => self.bookmark_open,
            _ => false,
        };
        let setting = matches!(
            action,
            Control::Smaller
                | Control::Larger
                | Control::Sans
                | Control::Spacing(_)
                | Control::Width(_)
                | Control::Alignment
                | Control::Publisher
                | Control::Theme
        );
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .key_context("EpubControl")
            .h(px(32.0))
            .min_w(px(32.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(gpui::transparent_black())
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .when(
                matches!(action, Control::Previous | Control::Next),
                |button| button.text_size(px(22.0)),
            )
            .when(setting, |button| button.bg(palette.control))
            .when(active, |button| button.bg(palette.selected))
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .text_color(if enabled { self.color() } else { palette.muted })
            .when(!enabled, |button| button.opacity(0.5))
            .when(enabled, |button| {
                button.hover(|style| {
                    style.bg(if active {
                        palette.selected
                    } else {
                        palette.hover
                    })
                })
            })
            .when(enabled, |button| {
                button
                    .tab_index(0)
                    .focus(|style| style.bg(palette.selected).border_color(palette.accent))
            })
            .tooltip(reader_tooltip(control_tooltip(action)))
            .on_click(cx.listener(move |reader, _, window, cx| {
                if enabled {
                    reader.control(action, window, cx);
                }
            }))
            .on_action(cx.listener(move |reader, _: &EpubActivate, window, cx| {
                if enabled {
                    reader.control(action, window, cx);
                }
                cx.stop_propagation();
            }))
            .child(label)
            .into_any_element()
    }

    fn page_label(&self) -> String {
        let total = self
            .layouts
            .get(&self.href)
            .filter(|layout| layout.complete)
            .map(|layout| layout.starts.len());
        total.map_or_else(
            || format!("Page {} of …", self.page_index + 1),
            |total| format!("Page {} of {}", self.page_index + 1, total),
        )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        let entity = cx.entity();
        let ready = self.current_page.is_some();
        let cover = self
            .book
            .as_ref()
            .is_some_and(|book| book.is_cover(&self.href));
        let percentage = self.index.as_ref().and_then(|index| {
            index.progress(
                &self.href,
                if self.at_boundary(true) {
                    self.current_page
                        .as_ref()
                        .map_or(self.anchor, |page| page.end)
                } else {
                    self.anchor
                },
            )
        });
        let page_label = if cover {
            "Cover".to_owned()
        } else if ready {
            self.page_label()
        } else {
            String::new()
        };
        let label_selector = format!("epub-page-label: {page_label}");
        div()
            .id("epub-toolbar")
            .debug_selector(|| "epub-toolbar".to_owned())
            .w_full()
            .flex()
            .flex_col()
            .px(px(12.0))
            .py(px(8.0))
            .gap(px(6.0))
            .flex_shrink_0()
            .border_t_1()
            .border_color(palette.border)
            .text_size(px(12.0))
            .text_color(palette.text)
            .bg(palette.surface)
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(10.0))
                    .min_h(px(20.0))
                    .text_color(palette.muted)
                    .child(
                        div()
                            .id("epub-progress-title")
                            .debug_selector(|| "epub-progress-title".to_owned())
                            .flex_1()
                            .min_w(px(0.0))
                            .text_ellipsis()
                            .child(if cover {
                                "Cover".into()
                            } else {
                                self.heading()
                            }),
                    )
                    .when(!cover, |row| {
                        row.when_some(percentage, |row, percentage| {
                            row.child(
                                div()
                                    .id("epub-progress-percentage")
                                    .debug_selector(|| "epub-progress-percentage".to_owned())
                                    .flex_shrink_0()
                                    .child(format!("{percentage}% read")),
                            )
                        })
                    }),
            )
            .child(
                div()
                    .on_children_prepainted(move |bounds, _, cx| {
                        entity.update(cx, |reader, _| {
                            reader.panel_trigger_bounds = bounds.iter().take(4).copied().collect();
                        });
                    })
                    .id("epub-reading-tools")
                    .debug_selector(|| "epub-reading-tools".to_owned())
                    .w_full()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.0))
                    .child(self.button(
                        "epub-chapters",
                        "Contents".into(),
                        self.book.is_some(),
                        Control::Chapters,
                        cx,
                    ))
                    .child(self.button("epub-settings", "Aa".into(), true, Control::Settings, cx))
                    .child(self.button(
                        "epub-find",
                        "Search".into(),
                        self.book.is_some(),
                        Control::Find,
                        cx,
                    ))
                    .child(self.button(
                        "epub-bookmark",
                        "Bookmark".into(),
                        ready,
                        Control::Bookmark,
                        cx,
                    ))
                    .child(
                        self.button(
                            "epub-fullscreen",
                            if self.fullscreen {
                                "Exit fullscreen"
                            } else {
                                "Fullscreen"
                            }
                            .into(),
                            true,
                            Control::Fullscreen,
                            cx,
                        ),
                    )
                    .when(
                        self.search_return.is_some() && self.search_highlight.is_some(),
                        |row| {
                            row.child(self.button(
                                "epub-return",
                                "Return to reading".into(),
                                true,
                                Control::Return,
                                cx,
                            ))
                        },
                    )
                    .child(
                        div()
                            .id("epub-page-controls")
                            .debug_selector(|| "epub-page-controls".to_owned())
                            .flex_1()
                            .min_w(px(180.0))
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(4.0))
                            .flex_shrink_0()
                            .child(self.button(
                                "epub-previous",
                                "‹".into(),
                                ready && !self.at_boundary(false),
                                Control::Previous,
                                cx,
                            ))
                            .child(
                                div()
                                    .id("epub-progress-pages")
                                    .debug_selector(|| "epub-progress-pages".to_owned())
                                    .min_w(px(100.0))
                                    .text_center()
                                    .text_color(palette.muted)
                                    .child(
                                        div()
                                            .debug_selector(move || label_selector.clone())
                                            .child(page_label),
                                    ),
                            )
                            .child(self.button(
                                "epub-next",
                                "›".into(),
                                ready && !self.at_boundary(true),
                                Control::Next,
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_chapters(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        let mut rows = Vec::new();
        let bookmarks = state::bookmarks(cx, &self.key);
        if !bookmarks.is_empty() {
            rows.push(
                div()
                    .px(px(12.0))
                    .py(px(8.0))
                    .child("Bookmarks")
                    .into_any_element(),
            );
            for (index, bookmark) in bookmarks.into_iter().enumerate() {
                let location = bookmark.location;
                let key_location = location.clone();
                rows.push(
                    div()
                        .id(("epub-bookmark-row", index))
                        .debug_selector(move || format!("epub-bookmark-row-{index}"))
                        .key_context("EpubControl")
                        .tab_index(0)
                        .focus(|style| {
                            style.bg(rgb(if self.preferences.light {
                                0xdce9f5
                            } else {
                                0x335a80
                            }))
                        })
                        .flex()
                        .items_center()
                        .px(px(12.0))
                        .py(px(6.0))
                        .cursor(CursorStyle::PointingHand)
                        .on_click(cx.listener(move |reader, _, window, cx| {
                            reader.history.push(reader.location());
                            reader.resume_location = Some(location.clone());
                            reader.chapters_open = false;
                            reader.open_section(
                                location.href.clone(),
                                PageRequest::Anchor(location.point),
                                None,
                                cx,
                            );
                            reader.focus_handle.focus(window);
                        }))
                        .on_action(cx.listener(move |reader, _: &EpubActivate, window, cx| {
                            reader.history.push(reader.location());
                            reader.resume_location = Some(key_location.clone());
                            reader.chapters_open = false;
                            reader.open_section(
                                key_location.href.clone(),
                                PageRequest::Anchor(key_location.point),
                                None,
                                cx,
                            );
                            reader.focus_handle.focus(window);
                            cx.stop_propagation();
                        }))
                        .child(div().flex_1().child(bookmark.name))
                        .child(
                            div()
                                .id(("epub-delete-bookmark", index))
                                .debug_selector(move || format!("epub-delete-bookmark-{index}"))
                                .key_context("EpubControl")
                                .tab_index(0)
                                .focus(|style| {
                                    style.bg(rgb(if self.preferences.light {
                                        0xdce9f5
                                    } else {
                                        0x335a80
                                    }))
                                })
                                .px(px(8.0))
                                .child("×")
                                .tooltip(reader_tooltip("Remove bookmark"))
                                .on_action(cx.listener(move |reader, _: &EpubActivate, _, cx| {
                                    let mut bookmarks = state::bookmarks(cx, &reader.key);
                                    if index < bookmarks.len() {
                                        bookmarks.remove(index);
                                        state::set_bookmarks(cx, &reader.key, bookmarks);
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                }))
                                .on_click(cx.listener(move |reader, _, _, cx| {
                                    let mut bookmarks = state::bookmarks(cx, &reader.key);
                                    if index < bookmarks.len() {
                                        bookmarks.remove(index);
                                        state::set_bookmarks(cx, &reader.key, bookmarks);
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),
                );
            }
            rows.push(
                div()
                    .id("epub-contents-separator")
                    .debug_selector(|| "epub-contents-separator".to_owned())
                    .h(px(1.0))
                    .flex_shrink_0()
                    .mx(px(12.0))
                    .my(px(8.0))
                    .bg(palette.border)
                    .into_any_element(),
            );
        }
        if let Some(book) = &self.book {
            for (index, item) in book.toc.iter().enumerate() {
                let target = item.target.clone();
                rows.push(
                    div()
                        .id(("epub-toc", index))
                        .debug_selector(move || format!("epub-toc-{index}"))
                        .key_context("EpubControl")
                        .tab_index(0)
                        .focus(|style| {
                            style.bg(rgb(if self.preferences.light {
                                0xdce9f5
                            } else {
                                0x335a80
                            }))
                        })
                        .when(self.current_toc() == Some(index), |row| {
                            row.bg(rgb(if self.preferences.light {
                                0xdce9f5
                            } else {
                                0x293b4c
                            }))
                        })
                        .when(self.contents_cursor == index, |row| {
                            row.border_l_2().border_color(palette.accent)
                        })
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
                        .on_click(cx.listener(move |reader, _, window, cx| {
                            if let Some(target) = &target {
                                reader.follow_link(target.clone(), cx);
                                reader.focus_handle.focus(window);
                            }
                        }))
                        .on_action(cx.listener(move |reader, _: &EpubActivate, window, cx| {
                            if let Some(target) = reader
                                .book
                                .as_ref()
                                .and_then(|book| book.toc.get(index))
                                .and_then(|item| item.target.clone())
                            {
                                reader.follow_link(target, cx);
                                reader.focus_handle.focus(window);
                            }
                            cx.stop_propagation();
                        }))
                        .child(item.title.clone())
                        .into_any_element(),
                );
            }
        }
        div()
            .id("epub-chapter-popup")
            .debug_selector(|| "epub-chapter-popup".to_owned())
            .w_full()
            .min_h(px(0.0))
            .max_h_full()
            .flex()
            .flex_col()
            .occlude()
            .bg(palette.surface)
            .text_color(self.color())
            .text_size(px(14.0))
            .border_1()
            .border_color(palette.border)
            .rounded(px(8.0))
            .overflow_hidden()
            .on_mouse_down_out(cx.listener(|reader, event, window, cx| {
                if reader.is_panel_trigger(event) {
                    return;
                }
                reader.chapters_open = false;
                reader.focus_handle.focus(window);
                cx.notify();
            }))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(palette.border)
                    .child(div().flex_1().child("Contents"))
                    .child(self.button(
                        "epub-contents-close",
                        "×".into(),
                        true,
                        Control::Dismiss,
                        cx,
                    )),
            )
            .child(
                div()
                    .id("epub-contents-list")
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.contents_scroll)
                    .children(rows),
            )
            .into_any_element()
    }

    fn panel_overlay(&self, panel: AnyElement) -> AnyElement {
        div()
            .absolute()
            .top(px(PANEL_INSET))
            .bottom(px(PANEL_INSET))
            .left(px(PANEL_INSET))
            .right(px(PANEL_INSET))
            .max_w(px(PANEL_WIDTH))
            .flex()
            .flex_col()
            .justify_end()
            .child(panel)
            .into_any_element()
    }

    fn panel(&self, id: &'static str) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .w_full()
            .min_h(px(0.0))
            .max_h(px(320.0))
            .overflow_y_scroll()
            .occlude()
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .bg(self.palette().surface)
            .text_color(self.color())
            .text_size(px(14.0))
            .border_1()
            .border_color(self.palette().border)
            .rounded(px(8.0))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        self.panel("epub-reading-settings")
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().child("Reading settings"))
                    .child(self.button(
                        "epub-settings-close",
                        "×".into(),
                        true,
                        Control::Dismiss,
                        cx,
                    )),
            )
            .child(
                self.button(
                    "epub-family",
                    if self.preferences.sans {
                        "Font: Sans serif"
                    } else {
                        "Font: Serif"
                    }
                    .into(),
                    true,
                    Control::Sans,
                    cx,
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .child(format!("Font size: {}", self.preferences.font_size)),
                    )
                    .child(self.button(
                        "epub-smaller",
                        "A−".into(),
                        self.preferences.font_size > 12,
                        Control::Smaller,
                        cx,
                    ))
                    .child(self.button(
                        "epub-larger",
                        "A+".into(),
                        self.preferences.font_size < 36,
                        Control::Larger,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().child(format!(
                        "Line spacing: {:.1}",
                        self.preferences.line_spacing as f32 / 10.0
                    )))
                    .child(self.button(
                        "epub-spacing-less",
                        "−".into(),
                        self.preferences.line_spacing > 12,
                        Control::Spacing(-1),
                        cx,
                    ))
                    .child(self.button(
                        "epub-spacing-more",
                        "+".into(),
                        self.preferences.line_spacing < 22,
                        Control::Spacing(1),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .child(format!("Reading width: {}", self.preferences.reading_width)),
                    )
                    .child(self.button(
                        "epub-width-less",
                        "−".into(),
                        self.preferences.reading_width > 400,
                        Control::Width(-100),
                        cx,
                    ))
                    .child(self.button(
                        "epub-width-more",
                        "+".into(),
                        self.preferences.reading_width < 1000,
                        Control::Width(100),
                        cx,
                    )),
            )
            .child(
                self.button(
                    "epub-alignment",
                    if self.preferences.justify {
                        "Alignment: Justified"
                    } else {
                        "Alignment: Left"
                    }
                    .into(),
                    true,
                    Control::Alignment,
                    cx,
                ),
            )
            .child(
                self.button(
                    "epub-publisher",
                    if self.preferences.publisher_styles {
                        "Publisher layout: On"
                    } else {
                        "Publisher layout: Reader settings"
                    }
                    .into(),
                    true,
                    Control::Publisher,
                    cx,
                ),
            )
            .child(
                self.button(
                    "epub-theme",
                    if self.preferences.light {
                        "Theme: Light"
                    } else {
                        "Theme: Dark"
                    }
                    .into(),
                    true,
                    Control::Theme,
                    cx,
                ),
            )
            .on_mouse_down_out(cx.listener(|reader, event, _, cx| {
                if reader.is_panel_trigger(event) {
                    return;
                }
                reader.settings_open = false;
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let status = if self.index_task.is_some() {
            "Preparing book search…".to_owned()
        } else if self.search_busy {
            "Searching…".to_owned()
        } else if self.input.read(cx).text.content.trim().is_empty() {
            String::new()
        } else if self.search_results.len() == 200 {
            "First 200 matches".to_owned()
        } else {
            format!("{} matches", self.search_results.len())
        };
        self.panel("epub-search-panel")
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().child("Search book"))
                    .child(self.button(
                        "epub-search-close",
                        "×".into(),
                        true,
                        Control::Dismiss,
                        cx,
                    )),
            )
            .child(self.input.clone())
            .when(!status.is_empty(), |panel| panel.child(status))
            .when(
                self.index.as_ref().is_some_and(|index| index.incomplete),
                |panel| panel.child("Some sections could not be searched."),
            )
            .children(
                self.search_results
                    .iter()
                    .enumerate()
                    .map(|(index, result)| {
                        div()
                            .id(("epub-search-result", index))
                            .key_context("EpubControl")
                            .tab_index(0)
                            .focus(|style| {
                                style.bg(rgb(if self.preferences.light {
                                    0xdce9f5
                                } else {
                                    0x335a80
                                }))
                            })
                            .cursor(CursorStyle::PointingHand)
                            .py(px(6.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(result.title.clone())
                            .child(result.excerpt.clone())
                            .on_click(cx.listener(move |reader, _, window, cx| {
                                reader.open_search_result(index, window, cx)
                            }))
                            .on_action(cx.listener(move |reader, _: &EpubActivate, window, cx| {
                                reader.open_search_result(index, window, cx);
                                cx.stop_propagation();
                            }))
                    }),
            )
            .on_mouse_down_out(cx.listener(|reader, event, window, cx| {
                if reader.is_panel_trigger(event) {
                    return;
                }
                reader.search_open = false;
                reader.focus_handle.focus(window);
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_bookmark_input(&self, cx: &mut Context<Self>) -> AnyElement {
        self.panel("epub-bookmark-panel")
            .child("Name this bookmark")
            .child(self.bookmark_input.clone())
            .child(
                div()
                    .flex()
                    .child(self.button(
                        "epub-bookmark-save",
                        "Save".into(),
                        !self.bookmark_input.read(cx).text.content.trim().is_empty(),
                        Control::SaveBookmark,
                        cx,
                    ))
                    .child(self.button(
                        "epub-bookmark-cancel",
                        "Cancel".into(),
                        true,
                        Control::Dismiss,
                        cx,
                    )),
            )
            .on_mouse_down_out(cx.listener(|reader, event, window, cx| {
                if !reader.is_panel_trigger(event) {
                    reader.dismiss(window, cx);
                }
            }))
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
    Back,
    Retry,
    Skip,
    Settings,
    Find,
    Bookmark,
    SaveBookmark,
    Return,
    Fullscreen,
    Dismiss,
    Copy,
    Sans,
    Spacing(i16),
    Width(i16),
    Alignment,
    Publisher,
}

fn control_tooltip(action: Control) -> &'static str {
    match action {
        Control::Previous => "Previous page (Left / Page Up)",
        Control::Next => "Next page (Right / Page Down)",
        Control::Chapters => "Contents and bookmarks (Ctrl/Cmd+T)",
        Control::Settings => "Reading settings (Ctrl/Cmd+,)",
        Control::Find => "Search book (Ctrl/Cmd+F)",
        Control::Bookmark => "Bookmark passage (Ctrl/Cmd+D)",
        Control::Fullscreen => "Toggle fullscreen (F11)",
        Control::Back => "Back (Alt+Left)",
        Control::Theme => "Switch light/dark theme",
        Control::Sans => "Choose serif or sans serif",
        Control::Smaller => "Smaller text",
        Control::Larger => "Larger text",
        Control::Spacing(_) => "Adjust line spacing",
        Control::Width(_) => "Adjust reading width",
        Control::Alignment => "Choose left or justified alignment",
        Control::Publisher => "Use supported publisher layout or override with reader settings",
        Control::Retry => "Retry this section",
        Control::Skip => "Skip unreadable section",
        Control::Return => "Return to the passage before search",
        Control::SaveBookmark => "Save bookmark",
        Control::Copy => "Copy selected text (Ctrl/Cmd+C)",
        Control::Dismiss => "Close (Escape)",
    }
}

struct ReaderTooltip(gpui::SharedString);
impl Render for ReaderTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(px(6.0))
            .max_w(px(260.0))
            .bg(rgb(0xf5f5f5))
            .text_color(rgb(0x202020))
            .text_size(px(12.0))
            .border_1()
            .border_color(rgb(0x808080))
            .child(self.0.clone())
    }
}
fn reader_tooltip(
    label: impl Into<gpui::SharedString>,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView {
    let label = label.into();
    move |_, cx| cx.new(|_| ReaderTooltip(label.clone())).into()
}

impl Render for Reader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_loading_feedback(cx);
        let loading = self.loading_visible && self.page_pending();
        let body = div()
            .id("epub-body")
            .debug_selector(|| "epub-body".to_owned())
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(self.render_surface(cx))
            .when(loading, |body| {
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
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            self.book
                                .as_ref()
                                .and_then(|book| {
                                    book.sections
                                        .iter()
                                        .find(|section| section.href == self.href)
                                })
                                .map(|section| format!("Could not read {}", section.title))
                                .unwrap_or_else(|| "Could not open book".into()),
                        )
                        .child(self.error.clone().unwrap_or_default())
                        .when(self.book.is_some(), |panel| {
                            panel.child(
                                div()
                                    .flex()
                                    .gap(px(8.0))
                                    .child(self.button(
                                        "epub-retry",
                                        "Retry".into(),
                                        true,
                                        Control::Retry,
                                        cx,
                                    ))
                                    .child(self.button(
                                        "epub-error-back",
                                        "Back".into(),
                                        !self.history.is_empty()
                                            || self.book.as_ref().is_some_and(|book| {
                                                book.sections.first().is_some_and(|section| {
                                                    section.href != self.href
                                                })
                                            }),
                                        Control::Back,
                                        cx,
                                    ))
                                    .child(self.button(
                                        "epub-skip",
                                        "Skip section".into(),
                                        self.book.as_ref().is_some_and(|book| {
                                            book.sections
                                                .iter()
                                                .position(|section| section.href == self.href)
                                                .is_some_and(|index| {
                                                    index + 1 < book.sections.len()
                                                })
                                        }),
                                        Control::Skip,
                                        cx,
                                    )),
                            )
                        }),
                )
            })
            .when(
                loading && self.current_page.is_none() && self.error.is_none(),
                |body| {
                    body.child(
                        div()
                            .absolute()
                            .id("epub-loading-message")
                            .debug_selector(|| "epub-loading-message".to_owned())
                            .top(px(32.0))
                            .left(px(48.0))
                            .child("Loading book…"),
                    )
                },
            )
            .when(self.chapters_open, |content| {
                content.child(self.panel_overlay(self.render_chapters(cx)))
            })
            .when(self.settings_open, |content| {
                content.child(self.panel_overlay(self.render_settings(cx)))
            })
            .when(self.search_open, |content| {
                content.child(self.panel_overlay(self.render_search(cx)))
            })
            .when(self.bookmark_open, |content| {
                content.child(self.panel_overlay(self.render_bookmark_input(cx)))
            })
            .when(self.note.is_some(), |content| {
                let (title, text) = self.note.as_ref().unwrap();
                content.child(
                    self.panel_overlay(
                        self.panel("epub-note-preview")
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(div().flex_1().child(title.clone()))
                                    .child(self.button(
                                        "epub-note-close",
                                        "×".into(),
                                        true,
                                        Control::Dismiss,
                                        cx,
                                    )),
                            )
                            .child(text.clone())
                            .on_mouse_down_out(
                                cx.listener(|reader, _, window, cx| reader.dismiss(window, cx)),
                            )
                            .into_any_element(),
                    ),
                )
            });
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
            .on_action(
                cx.listener(|reader, _: &EpubDismiss, window, cx| reader.dismiss(window, cx)),
            )
            .on_action(cx.listener(|reader, _: &EpubContents, window, cx| {
                reader.toggle_contents(window, cx)
            }))
            .on_action(cx.listener(|reader, _: &EpubSettings, window, cx| {
                reader.control(Control::Settings, window, cx)
            }))
            .on_action(
                cx.listener(|reader, _: &EpubFind, window, cx| reader.toggle_search(window, cx)),
            )
            .on_action(
                cx.listener(|reader, _: &EpubBookmark, window, cx| {
                    reader.open_bookmark(window, cx)
                }),
            )
            .on_action(cx.listener(|reader, _: &EpubFullscreen, window, cx| {
                reader.control(Control::Fullscreen, window, cx)
            }))
            .on_action(cx.listener(|_, _: &EpubFocusNext, window, cx| {
                window.focus_next();
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|_, _: &EpubFocusPrevious, window, cx| {
                window.focus_prev();
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(Self::contents_key))
            .on_action(cx.listener(|reader, _: &EpubActivate, window, cx| {
                if reader.chapters_open {
                    if let Some(target) = reader
                        .book
                        .as_ref()
                        .and_then(|book| book.toc.get(reader.contents_cursor))
                        .and_then(|item| item.target.clone())
                    {
                        reader.follow_link(target, cx);
                        reader.focus_handle.focus(window);
                    }
                }
                cx.stop_propagation();
            }))
            .on_mouse_move(cx.listener(|reader, event: &MouseMoveEvent, window, cx| {
                if reader.fullscreen {
                    let revealed = event.position.y >= window.viewport_size().height - px(90.0);
                    if reader.controls_revealed != revealed {
                        reader.controls_revealed = revealed;
                        cx.notify();
                    }
                }
            }))
            .when(!self.fullscreen, |content| {
                content.child(self.render_titlebar(window, cx))
            })
            .child(body)
            .when(
                !self.fullscreen
                    || self.controls_revealed
                    || self.chapters_open
                    || self.settings_open
                    || self.search_open
                    || self.bookmark_open,
                |content| content.child(self.render_toolbar(cx)),
            )
            .when(self.enlarged_image.is_some(), |content| {
                content.child(
                    div()
                        .id("epub-enlarged-image")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(rgb(if self.preferences.light {
                            0xfaf9f6
                        } else {
                            0x1c1c1c
                        }))
                        .p(px(32.0))
                        .flex()
                        .flex_col()
                        .child(self.button(
                            "epub-image-close",
                            "Close image (Escape)".into(),
                            true,
                            Control::Dismiss,
                            cx,
                        ))
                        .child(
                            gpui::img(self.enlarged_image.as_ref().unwrap().image.clone())
                                .object_fit(gpui::ObjectFit::Contain)
                                .w_full()
                                .flex_1()
                                .min_h(px(0.0)),
                        ),
                )
            })
            .when(self.copy_menu.is_some(), |content| {
                content.child(
                    div()
                        .id("epub-copy-menu")
                        .absolute()
                        .occlude()
                        .left(px(f32::from(self.copy_menu.unwrap().x).clamp(
                            0.0,
                            (f32::from(window.viewport_size().width) - 180.0).max(0.0),
                        )))
                        .top(px((f32::from(self.copy_menu.unwrap().y)
                            + if self.fullscreen {
                                0.0
                            } else {
                                TITLEBAR_HEIGHT
                            })
                        .clamp(
                            0.0,
                            (f32::from(window.viewport_size().height) - 40.0).max(0.0),
                        )))
                        .bg(rgb(if self.preferences.light {
                            0xf5f5f5
                        } else {
                            0x252525
                        }))
                        .border_1()
                        .border_color(rgb(0x808080))
                        .w(px(180.0))
                        .child(self.button(
                            "epub-copy-selection",
                            "Copy".into(),
                            true,
                            Control::Copy,
                            cx,
                        ))
                        .on_mouse_down_out(cx.listener(|reader, _, _, cx| {
                            reader.copy_menu = None;
                            cx.notify();
                        })),
                )
            })
            .into_any_element();
        render_platform_window_frame(content, window)
    }
}

fn word_selection(chapter: &Chapter, point: ContentPoint) -> Option<(ContentPoint, ContentPoint)> {
    let Block::Text { text, .. } = chapter.blocks.get(point.block)? else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    let point = normalize_point(chapter, point);
    let mut selected = point.offset.min(text.len());
    if selected == text.len() {
        selected = text.char_indices().last()?.0;
    }
    let ch = text[selected..].chars().next()?;
    let word = ch.is_alphanumeric() || ch == '_';
    let same = |ch: char| {
        if word {
            ch.is_alphanumeric() || ch == '_' || ch == '\'' || ch == '’'
        } else {
            ch == text[selected..].chars().next().unwrap()
        }
    };
    let mut start = selected;
    for (offset, ch) in text[..selected].char_indices().rev() {
        if !same(ch) {
            break;
        }
        start = offset;
    }
    let mut end = selected;
    for (offset, ch) in text[selected..].char_indices() {
        if !same(ch) {
            break;
        }
        end = selected + offset + ch.len_utf8();
    }
    Some((
        ContentPoint {
            block: point.block,
            offset: start,
        },
        ContentPoint {
            block: point.block,
            offset: end,
        },
    ))
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

    #[gpui::test]
    fn repeated_epub_opens_reuse_loading_and_loaded_reader(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        fixture(&path);
        cx.update(|cx| open_epub_window(path.clone(), cx).unwrap());
        let handle = cx.windows()[0].downcast::<Reader>().unwrap();
        let reader = handle.root(cx).unwrap();
        // Do not run the executor between opens: the book is still loading.
        assert!(reader.read_with(cx, |reader, _| reader.book.is_none()));
        cx.update(|cx| open_epub_window(path.clone(), cx).unwrap());
        assert!(cx.windows() == vec![handle.into()]);
        assert_eq!(handle.root(cx).unwrap(), reader);

        let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
        settle(&reader, &mut visual);
        reader.update(cx, |reader, cx| reader.turn(1, cx));
        settle(&reader, &mut visual);
        let location = reader.read_with(cx, |reader, _| {
            assert!(reader.book.is_some());
            assert_ne!(reader.anchor, ContentPoint::default());
            (reader.href.clone(), reader.anchor, reader.page_index)
        });
        cx.update(|cx| open_epub_window(path, cx).unwrap());
        assert!(cx.windows() == vec![handle.into()]);
        assert_eq!(handle.root(cx).unwrap(), reader);
        reader.read_with(cx, |reader, _| {
            assert_eq!(
                (reader.href.clone(), reader.anchor, reader.page_index),
                location
            );
            assert!(reader.load_task.is_none());
        });
    }

    #[gpui::test]
    fn distinct_epub_files_keep_separate_windows_and_focus_the_requested_file(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("book.epub");
        let copy = dir.path().join("copy.epub");
        let other = dir.path().join("other.epub");
        fixture(&first);
        std::fs::copy(&first, &copy).unwrap();
        super::super::tests::write_book(
            &other,
            "3.0",
            "",
            &[(
                "other.xhtml",
                "<html><body><p>Another book.</p></body></html>",
            )],
            &[],
        );
        // Include a non-reader window to exercise root-type filtering.
        cx.add_empty_window();
        let unrelated = cx.windows()[0];
        let mut handles = Vec::new();
        for path in [&first, &copy, &other] {
            cx.update(|cx| open_epub_window(path.clone(), cx).unwrap());
            let handle = cx
                .windows()
                .into_iter()
                .find(|handle| *handle != unrelated && !handles.contains(handle))
                .unwrap();
            handles.push(handle);
        }
        assert_eq!(cx.windows().len(), 4);
        for (path, handle) in [&first, &copy, &other].into_iter().zip(handles) {
            unrelated
                .update(cx, |_, window, _| window.activate_window())
                .unwrap();
            cx.update(|cx| open_epub_window(path.clone(), cx).unwrap());
            assert_eq!(cx.windows().len(), 4);
            assert!(cx.update(|cx| cx.active_window()) == Some(handle));
        }
    }

    #[gpui::test]
    fn epub_path_aliases_reuse_the_same_window(cx: &mut TestAppContext) {
        let cwd = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir_in(&cwd).unwrap();
        let path = dir.path().join("book.epub");
        fixture(&path);
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        cx.update(|cx| open_epub_window(path.clone(), cx).unwrap());
        let handle = cx.windows()[0];
        for alias in [
            path.strip_prefix(&cwd).unwrap().to_path_buf(),
            dir.path().join("subdir/.././book.epub"),
            std::fs::canonicalize(&path).unwrap(),
        ] {
            cx.update(|cx| open_epub_window(alias, cx).unwrap());
            assert!(cx.windows() == vec![handle]);
        }
    }

    #[cfg(unix)]
    #[gpui::test]
    fn epub_symlink_alias_reuses_the_same_window(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        let alias = dir.path().join("alias.epub");
        fixture(&path);
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        cx.update(|cx| open_epub_window(alias, cx).unwrap());
        let handle = cx.windows()[0];
        cx.update(|cx| open_epub_window(path, cx).unwrap());
        assert!(cx.windows() == vec![handle]);
    }

    #[gpui::test]
    fn errored_epub_reuses_its_window_and_can_reopen_after_close(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.epub");
        cx.update(|cx| open_epub_window(path.clone(), cx).unwrap());
        let handle = cx.windows()[0].downcast::<Reader>().unwrap();
        cx.run_until_parked();
        let error = handle
            .read_with(cx, |reader, _| reader.error.clone())
            .unwrap();
        assert!(error.is_some());
        cx.update(|cx| open_epub_window(dir.path().join("absent/../missing.epub"), cx).unwrap());
        assert!(cx.windows() == vec![handle.into()]);
        assert_eq!(
            handle
                .read_with(cx, |reader, _| reader.error.clone())
                .unwrap(),
            error
        );
        handle
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        assert!(cx.windows().is_empty());
        cx.update(|cx| open_epub_window(path, cx).unwrap());
        assert_eq!(cx.windows().len(), 1);
        assert!(cx.windows()[0] != handle.into());
    }

    #[gpui::test]
    fn epub_windows_respect_case_sensitive_filesystems(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let lower = dir.path().join("book.epub");
        let upper = dir.path().join("BOOK.EPUB");
        fixture(&lower);
        let case_sensitive = !upper.exists();
        if case_sensitive {
            std::fs::copy(&lower, &upper).unwrap();
        }
        cx.update(|cx| {
            open_epub_window(lower, cx).unwrap();
            open_epub_window(upper, cx).unwrap();
        });
        assert_eq!(cx.windows().len(), if case_sensitive { 2 } else { 1 });
    }

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

    fn finish_page_count(reader: &Entity<Reader>, cx: &mut gpui::VisualTestContext) -> usize {
        for _ in 0..512 {
            settle(reader, cx);
            let total = reader.update(cx, |reader, _| {
                assert!(reader.error.is_none(), "{:?}", reader.error);
                reader
                    .layouts
                    .get(&reader.href)
                    .filter(|layout| layout.complete)
                    .map(|layout| layout.starts.len())
            });
            if let Some(total) = total {
                return total;
            }
            cx.update(|window, app| {
                window.refresh();
                let _ = window.draw(app);
            });
        }
        panic!("Chapter page count did not finish");
    }

    #[gpui::test]
    fn toolbar_panels_toggle_on_full_clicks_and_keyboard_actions(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        fixture(&path);
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Panels".into(), window, cx));
        settle(&reader, visual);
        assert!(visual.debug_bounds("epub-back").is_none());
        let panels = [
            ("epub-chapters", "epub-chapter-popup"),
            ("epub-settings", "epub-reading-settings"),
            ("epub-find", "epub-search-panel"),
            ("epub-bookmark", "epub-bookmark-panel"),
        ];
        for (index, (button, panel)) in panels.into_iter().enumerate() {
            let position = visual.debug_bounds(button).unwrap().center();
            visual.simulate_click(position, Modifiers::default());
            reader.update(visual, |reader, _| {
                assert!(
                    [
                        reader.chapters_open,
                        reader.settings_open,
                        reader.search_open,
                        reader.bookmark_open
                    ][index]
                );
            });
            assert!(visual.debug_bounds(panel).is_some(), "{button} should open");
            visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
            assert!(
                visual.debug_bounds(panel).is_some(),
                "{button} must stay open on mouse down"
            );
            visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
            reader.update(visual, |reader, _| {
                assert!(
                    !reader.chapters_open
                        && !reader.settings_open
                        && !reader.search_open
                        && !reader.bookmark_open,
                    "{button} state should close"
                )
            });
        }
        for (index, (button, _)) in panels.into_iter().enumerate() {
            let position = visual.debug_bounds(button).unwrap().center();
            visual.simulate_click(position, Modifiers::default());
            reader.update(visual, |reader, _| {
                let open = [
                    reader.chapters_open,
                    reader.settings_open,
                    reader.search_open,
                    reader.bookmark_open,
                ];
                assert!(open[index]);
                assert_eq!(open.into_iter().filter(|open| *open).count(), 1);
            });
        }
        let body = visual.debug_bounds("epub-body").unwrap();
        visual.simulate_click(
            body.top_right() + point(px(-16.0), px(16.0)),
            Modifiers::default(),
        );
        reader.update(visual, |reader, _| assert!(!reader.bookmark_open));
        // The same actions used by keyboard shortcuts must toggle both ways,
        // including actions dispatched while an input owns focus.
        for _ in 0..2 {
            visual.dispatch_action(EpubContents);
        }
        for _ in 0..2 {
            visual.dispatch_action(EpubSettings);
        }
        for _ in 0..2 {
            visual.dispatch_action(EpubFind);
        }
        for _ in 0..2 {
            visual.dispatch_action(EpubBookmark);
        }
        reader.update(visual, |reader, _| {
            assert!(
                !reader.chapters_open
                    && !reader.settings_open
                    && !reader.search_open
                    && !reader.bookmark_open
            );
        });
        let previous = visual.debug_bounds("epub-previous").unwrap();
        let next = visual.debug_bounds("epub-next").unwrap();
        let pages = visual.debug_bounds("epub-progress-pages").unwrap();
        assert!(previous.right() < pages.left());
        assert!(pages.right() < next.left());
        assert_eq!(previous.center().y, pages.center().y);
        assert_eq!(pages.center().y, next.center().y);
    }

    #[gpui::test]
    fn reading_controls_fit_narrow_windows_and_keep_panel_triggers_working(
        cx: &mut TestAppContext,
    ) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controls.epub");
        fixture(&path);
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Controls".into(), window, cx));
        settle(&reader, visual);
        for (width, height, light, fullscreen) in [
            (900.0, 820.0, false, false),
            (360.0, 240.0, true, false),
            (360.0, 600.0, false, true),
        ] {
            reader.update(visual, |reader, cx| {
                reader.preferences.light = light;
                reader.fullscreen = fullscreen;
                reader.controls_revealed = true;
                cx.notify();
            });
            visual.simulate_resize(size(px(width), px(height)));
            settle(&reader, visual);
            let toolbar = visual.debug_bounds("epub-toolbar").unwrap();
            let body = visual.debug_bounds("epub-body").unwrap();
            assert!(body.size.height > px(0.0));
            for id in [
                "epub-previous",
                "epub-next",
                "epub-chapters",
                "epub-settings",
                "epub-find",
                "epub-bookmark",
                "epub-fullscreen",
            ] {
                let bounds = visual.debug_bounds(id).unwrap();
                assert!(
                    bounds.left() >= toolbar.left() && bounds.right() <= toolbar.right(),
                    "{id}: {bounds:?} {toolbar:?}"
                );
                assert!(
                    bounds.top() >= toolbar.top() && bounds.bottom() <= toolbar.bottom(),
                    "{id}: {bounds:?} {toolbar:?}"
                );
                assert!(bounds.size.width >= px(32.0) && bounds.size.height >= px(32.0));
            }
            let position = visual.debug_bounds("epub-chapters").unwrap().center();
            visual.simulate_click(position, Modifiers::default());
            let close = visual.debug_bounds("epub-contents-close").unwrap().center();
            visual.simulate_click(close, Modifiers::default());
            reader.update(visual, |reader, _| assert!(!reader.chapters_open));
            // The grouped tools must still prevent outside-click dismissal on
            // mouse down, so a second click on the trigger closes the panel.
            let position = visual.debug_bounds("epub-settings").unwrap().center();
            visual.simulate_click(position, Modifiers::default());
            visual.simulate_click(position, Modifiers::default());
            reader.update(visual, |reader, _| assert!(!reader.settings_open));
        }
    }

    #[gpui::test]
    fn menus_share_insets_and_toolbar_keeps_tools_left_and_navigation_right(
        cx: &mut TestAppContext,
    ) {
        cx.update(state::initialize);
        cx.update(|cx| cx.bind_keys(crate::app::platform_key_bindings()));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("menus.epub");
        fixture(&path);
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Menus".into(), window, cx));
        settle(&reader, visual);
        for (width, height, fullscreen) in [
            (900.0, 820.0, false),
            (360.0, 240.0, false),
            (360.0, 600.0, true),
        ] {
            reader.update(visual, |reader, cx| {
                reader.fullscreen = fullscreen;
                reader.controls_revealed = true;
                cx.notify();
            });
            visual.simulate_resize(size(px(width), px(height)));
            settle(&reader, visual);
            let toolbar = visual.debug_bounds("epub-toolbar").unwrap();
            let contents = visual.debug_bounds("epub-chapters").unwrap();
            let next = visual.debug_bounds("epub-next").unwrap();
            assert_eq!(contents.left(), toolbar.left() + px(PANEL_INSET));
            assert_eq!(
                next.right(),
                toolbar.right() - px(PANEL_INSET),
                "tools={:?} navigation={:?} next={next:?}",
                visual.debug_bounds("epub-reading-tools"),
                visual.debug_bounds("epub-page-controls")
            );
            let ids = [
                "epub-chapters",
                "epub-settings",
                "epub-find",
                "epub-bookmark",
                "epub-fullscreen",
            ];
            let mut previous = None;
            for id in ids {
                let bounds = visual.debug_bounds(id).unwrap();
                if let Some(previous) = previous {
                    let previous: Bounds<Pixels> = previous;
                    assert!(bounds.top() > previous.top() || bounds.left() > previous.left());
                }
                previous = Some(bounds);
            }
            let fullscreen_bounds = visual.debug_bounds("epub-fullscreen").unwrap();
            reader.update(visual, |reader, _| {
                assert_eq!(reader.panel_trigger_bounds.len(), 4);
                assert!(
                    !reader
                        .panel_trigger_bounds
                        .iter()
                        .any(|bounds| bounds.contains(&fullscreen_bounds.center()))
                );
            });
            for (action, selector) in [
                (Control::Chapters, "epub-chapter-popup"),
                (Control::Settings, "epub-reading-settings"),
                (Control::Find, "epub-search-panel"),
                (Control::Bookmark, "epub-bookmark-panel"),
            ] {
                visual.update(|window, cx| {
                    reader.update(cx, |reader, cx| reader.control(action, window, cx))
                });
                let panel = visual.debug_bounds(selector).unwrap();
                let body = visual.debug_bounds("epub-body").unwrap();
                assert_eq!(panel.left() - body.left(), px(PANEL_INSET));
                assert_eq!(body.bottom() - panel.bottom(), px(PANEL_INSET));
                assert!(panel.top() >= body.top() + px(PANEL_INSET) - px(0.01));
                assert!(panel.right() <= body.right() - px(PANEL_INSET));
                assert!(panel.size.width <= px(PANEL_WIDTH));
                visual.dispatch_action(EpubDismiss);
            }
        }
        visual.simulate_keystrokes("tab enter");
        reader.update(visual, |reader, _| assert!(reader.chapters_open));
    }

    #[gpui::test]
    fn completed_pagination_notifies_and_updates_the_rendered_label_without_input(
        cx: &mut TestAppContext,
    ) {
        cx.update(state::initialize);
        // Exercise completion in both the visible page and background warming.
        for long in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("total.epub");
            if long {
                fixture(&path);
            } else {
                super::super::tests::write_book(
                    &path,
                    "3.0",
                    "",
                    &[("c.xhtml", "<html><body><p>One page.</p></body></html>")],
                    &[],
                );
            }
            let (reader, visual) =
                cx.add_window_view(|window, cx| Reader::new(path, "Total".into(), window, cx));
            let expected_total = finish_page_count(&reader, visual);
            assert_eq!(expected_total > 1, long);
            let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
            let observed = notifications.clone();
            visual.update(|_, cx| {
                cx.observe(&reader, move |_, _| observed.set(observed.get() + 1))
                    .detach();
            });
            reader.update(visual, |reader, cx| {
                let layout = reader.layouts.get_mut(&reader.href).unwrap();
                layout.complete = false;
                // Keep the requested page boundary known. Discovering it can
                // exceed the 4 ms budget and yield to a native frame callback,
                // which the test platform does not provide. Background warming
                // still has to discover the remaining boundaries and completion.
                layout.starts.truncate(if long { 2 } else { 1 });
                layout.pages.clear();
                // The long chapter's second-page label has never been drawn,
                // so stale debug bounds cannot satisfy the rendered-label check.
                reader.request = Some(PageRequest::Index(if long { 1 } else { 0 }));
                cx.notify(); // Start one normal frame; completion must schedule its own follow-up.
            });
            visual.run_until_parked();
            assert!(
                notifications.get() >= 2,
                "completion must notify after prepaint"
            );
            let selector = Box::leak(
                format!(
                    "epub-page-label: Page {} of {expected_total}",
                    if long { 2 } else { 1 }
                )
                .into_boxed_str(),
            );
            assert!(visual.debug_bounds(selector).is_some());
            reader.update(visual, |reader, _| {
                assert!(!reader.loading_visible && reader.loading_delay_task.is_none())
            });
            let count = notifications.get();
            visual.run_until_parked();
            assert_eq!(
                notifications.get(),
                count,
                "completed pagination must become idle"
            );
        }
    }

    #[gpui::test]
    fn loading_feedback_waits_for_200ms_and_spans_book_and_chapter_loading(
        cx: &mut TestAppContext,
    ) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delayed.epub");
        fixture(&path);
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Loading".into(), window, cx));
        settle(&reader, visual);
        reader.update(visual, |reader, cx| {
            reader.chapters.clear();
            reader.current_page = None;
            reader.request = None;
            reader.load_task = Some(cx.spawn(async |_, _| futures::future::pending::<()>().await));
            reader.sync_loading_feedback(cx);
            cx.notify();
        });
        visual.run_until_parked();
        visual
            .background_executor
            .advance_clock(Duration::from_millis(100));
        reader.update(visual, |reader, cx| {
            let generation = reader.loading_generation;
            reader.load_task = None;
            reader.chapter_task =
                Some(cx.spawn(async |_, _| futures::future::pending::<()>().await));
            reader.sync_loading_feedback(cx);
            assert_eq!(reader.loading_generation, generation);
            cx.notify();
        });
        visual
            .background_executor
            .advance_clock(Duration::from_millis(99));
        visual.run_until_parked();
        reader.update(visual, |reader, _| assert!(!reader.loading_visible));
        assert!(visual.debug_bounds("epub-loading").is_none());
        assert!(visual.debug_bounds("epub-loading-message").is_none());
        visual
            .background_executor
            .advance_clock(Duration::from_millis(2));
        visual.run_until_parked();
        reader.update(visual, |reader, _| assert!(reader.loading_visible));
        assert!(visual.debug_bounds("epub-loading").is_some());
        assert!(visual.debug_bounds("epub-loading-message").is_some());
        reader.update(visual, |reader, cx| {
            reader.error = Some("Loading failed".into());
            reader.sync_loading_feedback(cx);
            assert!(!reader.loading_visible && reader.loading_delay_task.is_none());
            cx.notify();
        });
        // Debug selectors can retain old bounds after nodes disappear; the
        // presentation gate and cancellation state verify feedback removal.
        reader.update(visual, |reader, _| assert!(!reader.page_pending()));
    }

    #[gpui::test]
    fn quick_loads_cancel_feedback_and_successive_requests_get_a_fresh_delay(
        cx: &mut TestAppContext,
    ) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quick.epub");
        fixture(&path);
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Quick".into(), window, cx));
        settle(&reader, visual);
        reader.update(visual, |reader, cx| {
            reader.chapters.clear();
            reader.current_page = None;
            reader.request = Some(PageRequest::Index(0));
            reader.sync_loading_feedback(cx);
            cx.notify();
        });
        visual.run_until_parked();
        visual
            .background_executor
            .advance_clock(Duration::from_millis(100));
        let old_generation = reader.update(visual, |reader, cx| {
            let generation = reader.loading_generation;
            reader.request = None;
            reader.sync_loading_feedback(cx);
            assert!(!reader.loading_visible && reader.loading_delay_task.is_none());
            reader.request = Some(PageRequest::Index(1));
            reader.sync_loading_feedback(cx);
            assert_ne!(reader.loading_generation, generation);
            cx.notify();
            generation
        });
        visual.run_until_parked();
        visual
            .background_executor
            .advance_clock(Duration::from_millis(101));
        visual.run_until_parked();
        reader.update(visual, |reader, _| {
            assert!(!reader.loading_visible);
            assert_ne!(reader.loading_generation, old_generation);
        });
        visual
            .background_executor
            .advance_clock(Duration::from_millis(100));
        visual.run_until_parked();
        reader.update(visual, |reader, cx| {
            assert!(reader.loading_visible);
            reader.request = None;
            reader.sync_loading_feedback(cx);
            assert!(!reader.loading_visible && reader.loading_delay_task.is_none());
            cx.notify();
        });
        visual
            .background_executor
            .advance_clock(Duration::from_secs(1));
        visual.run_until_parked();
        reader.update(visual, |reader, _| assert!(!reader.loading_visible));
    }

    #[gpui::test]
    fn contents_bookmarks_precede_chapters_and_keep_navigation_and_deletion(
        cx: &mut TestAppContext,
    ) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        fixture(&path);
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Bookmarks".into(), window, cx));
        settle(&reader, visual);
        reader.update(visual, |reader, cx| {
            let mut location = reader.location();
            location.href = "/book/second.xhtml".into();
            location.point = ContentPoint::default();
            state::set_bookmarks(
                cx,
                &reader.key,
                vec![Bookmark {
                    name: "Saved passage".into(),
                    location,
                }],
            );
        });
        visual.dispatch_action(EpubContents);
        let bookmark = visual.debug_bounds("epub-bookmark-row-0").unwrap();
        let separator = visual.debug_bounds("epub-contents-separator").unwrap();
        let chapter = visual.debug_bounds("epub-toc-0").unwrap();
        assert!(bookmark.bottom() < separator.top());
        assert!(separator.bottom() < chapter.top());
        let popup = visual.debug_bounds("epub-chapter-popup").unwrap();
        let body = visual.debug_bounds("epub-body").unwrap();
        assert!(
            popup.size.height < body.size.height - px(16.0),
            "short contents should retain natural height"
        );
        visual.simulate_click(
            bookmark.origin + point(px(20.0), px(10.0)),
            Modifiers::default(),
        );
        settle(&reader, visual);
        reader.update(visual, |reader, _| {
            assert_eq!(reader.href, "/book/second.xhtml")
        });
        visual.dispatch_action(EpubContents);
        let delete = visual
            .debug_bounds("epub-delete-bookmark-0")
            .unwrap()
            .center();
        visual.simulate_click(delete, Modifiers::default());
        reader.update(visual, |reader, cx| {
            assert!(
                state::bookmarks(cx, &reader.key).is_empty(),
                "delete should remove bookmark"
            )
        });
        reader.update(visual, |reader, cx| {
            assert!(reader.chapters_open);
            assert!(state::bookmarks(cx, &reader.key).is_empty());
            // GPUI keeps old debug selectors after removing nodes; inspect the
            // actual scroll children to check that the heading and divider left.
            assert_eq!(
                reader.contents_scroll.children_count(),
                reader.book.as_ref().unwrap().toc.len()
            );
        });
    }

    #[gpui::test]
    fn long_contents_fill_the_reading_area_above_wrapped_and_fullscreen_toolbars(
        cx: &mut TestAppContext,
    ) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        let links = (0..80)
            .map(|index| format!("<li><a href='c.xhtml#p{index}'>Chapter {index}</a></li>"))
            .collect::<String>();
        let nav = format!(
            "<html xmlns:epub='http://www.idpf.org/2007/ops'><body><nav epub:type='toc'><ol>{links}</ol></nav></body></html>"
        );
        super::super::tests::write_book(
            &path,
            "3.0",
            "",
            &[("c.xhtml", "<html><body><p>Readable</p></body></html>")],
            &[("nav.xhtml", "application/xhtml+xml", nav.as_bytes())],
        );
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Contents".into(), window, cx));
        settle(&reader, visual);
        reader.update(visual, |reader, cx| {
            state::set_bookmarks(
                cx,
                &reader.key,
                vec![Bookmark {
                    name: "Saved".into(),
                    location: reader.location(),
                }],
            );
        });
        visual.dispatch_action(EpubContents);
        for (width, height, fullscreen) in [
            (900.0, 820.0, false),
            (360.0, 240.0, false),
            (360.0, 600.0, false),
            (900.0, 820.0, true),
        ] {
            reader.update(visual, |reader, cx| {
                reader.fullscreen = fullscreen;
                cx.notify();
            });
            visual.simulate_resize(size(px(width), px(height)));
            settle(&reader, visual);
            let popup = visual.debug_bounds("epub-chapter-popup").unwrap();
            let body = visual.debug_bounds("epub-body").unwrap();
            let toolbar = visual.debug_bounds("epub-toolbar").unwrap();
            assert!(
                popup.top() >= body.top() + px(PANEL_INSET - 1.0),
                "{popup:?} {body:?}"
            );
            assert!(popup.bottom() <= toolbar.top() - px(PANEL_INSET - 1.0));
            assert!(
                (f32::from(popup.size.height) - f32::from(body.size.height) + PANEL_INSET * 2.0)
                    .abs()
                    < 1.0
            );
            if height == 820.0 {
                assert!(popup.size.height > px(320.0));
            }
        }
        // The bookmark heading, bookmark, and separator precede TOC index 79.
        reader.update(visual, |reader, cx| {
            reader.contents_cursor = 79;
            reader.scroll_to_contents_cursor(cx);
            cx.notify();
        });
        visual.run_until_parked();
        visual.update(|window, app| {
            window.refresh();
            let _ = window.draw(app);
        });
        let chapter = visual.debug_bounds("epub-toc-79").unwrap();
        let popup = visual.debug_bounds("epub-chapter-popup").unwrap();
        // ScrollHandle includes the panel border in its viewport calculation.
        assert!(
            chapter.top() >= popup.top() && chapter.bottom() <= popup.bottom() + px(1.0),
            "{chapter:?} {popup:?}"
        );
    }

    #[gpui::test]
    fn chapter_totals_finish_without_turning_and_recalculate_after_reflow(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        fixture(&path);
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Page counts".into(), window, cx));
        settle(&reader, visual);
        let selection = (
            ContentPoint::default(),
            ContentPoint {
                block: 0,
                offset: 8,
            },
        );
        reader.update(visual, |reader, _| reader.selection = Some(selection));
        let total = finish_page_count(&reader, visual);
        assert!(total > 2);
        reader.update(visual, |reader, _| {
            assert_eq!(reader.page_index, 0);
            assert_eq!(reader.anchor, ContentPoint::default());
            assert_eq!(reader.selection, Some(selection));
            assert_eq!(reader.page_label(), format!("Page 1 of {total}"));
            assert!(reader.layouts[&reader.href].pages.len() <= PAGE_CACHE_SIZE);
        });
        visual.simulate_resize(size(px(500.0), px(400.0)));
        let resized_total = finish_page_count(&reader, visual);
        assert!(resized_total > total);
        visual.update(|window, cx| {
            reader.update(cx, |reader, cx| reader.control(Control::Larger, window, cx))
        });
        let larger_total = finish_page_count(&reader, visual);
        assert!(larger_total > resized_total);
        // Restart counting and navigate while the layout is incomplete.
        reader.update(visual, |reader, cx| {
            let layout = reader.layouts.get_mut(&reader.href).unwrap();
            layout.starts.truncate(1);
            layout.complete = false;
            layout.pages.clear();
            assert_eq!(reader.page_label(), "Page 1 of …");
            reader.turn(1, cx);
        });
        assert_eq!(finish_page_count(&reader, visual), larger_total);
        reader.update(visual, |reader, _| {
            assert_eq!(reader.page_index, 1);
            assert_eq!(reader.page_label(), format!("Page 2 of {larger_total}"));
            assert!(reader.layouts[&reader.href].pages.len() <= PAGE_CACHE_SIZE);
        });
    }

    #[gpui::test]
    fn counting_resolves_images_beyond_the_reading_page(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        let text = format!(
            "<html><body><p>{}</p><img src='tall.png'/><p>After image</p></body></html>",
            "A readable sentence. ".repeat(800)
        );
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            80,
            800,
            image::Rgb([40, 80, 120]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        super::super::tests::write_book(
            &path,
            "3.0",
            "",
            &[("c.xhtml", &text)],
            &[("tall.png", "image/png", png.get_ref())],
        );
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Images".into(), window, cx));
        let total = finish_page_count(&reader, visual);
        assert!(total > 2);
        reader.update(visual, |reader, _| {
            assert_eq!(reader.page_index, 0);
            assert_eq!(reader.anchor, ContentPoint::default());
            let chapter = &reader.chapters[&reader.href];
            let image = chapter
                .blocks
                .iter()
                .position(|block| matches!(block, Block::Image { .. }))
                .unwrap();
            let slot = &reader.images[&reader.href][&image];
            assert_eq!(slot.width, 80.0);
            assert_eq!(slot.height, 800.0);
            assert!(!slot.failed);
        });
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
        // Incremental pagination can span native frames, which the test platform
        // does not schedule. Finish each operation before inspecting its page.
        settle(&reader, cx);
        cx.dispatch_action(EpubNext);
        settle(&reader, cx);
        let location = reader.update(cx, |reader, _| Location {
            href: reader.href.clone(),
            point: reader.anchor,
            ..Location::default()
        });
        reader.update(cx, |reader, cx| {
            reader.follow_link("/book/notes.xhtml#note".into(), cx)
        });
        settle(&reader, cx);
        reader.update(cx, |reader, _| assert_eq!(reader.href, "/book/notes.xhtml"));
        cx.dispatch_action(EpubBack);
        settle(&reader, cx);
        reader.update(cx, |reader, _| {
            assert_eq!(reader.href, location.href);
            assert_eq!(reader.anchor, location.point);
        });
        reader.update(cx, |reader, cx| reader.change_font(2, cx));
        settle(&reader, cx);
        reader.update(cx, |reader, _| assert_eq!(reader.anchor, location.point));
        cx.simulate_resize(size(px(480.0), px(400.0)));
        settle(&reader, cx);
        reader.update(cx, |reader, _| assert_eq!(reader.anchor, location.point));
        cx.simulate_resize(size(px(1200.0), px(900.0)));
        settle(&reader, cx);
        reader.update(cx, |reader, _| assert_eq!(reader.anchor, location.point));
        let old_page = reader.update(cx, |reader, cx| {
            assert!(reader.error.is_none(), "{:?}", reader.error);
            assert!(reader.request.is_none());
            let page = reader.current_page.clone().unwrap();
            let layout = &reader.layouts[&reader.href];
            assert_eq!(layout.preferences, reader.preferences);
            assert_eq!(layout.starts[reader.page_index], page.start);
            reader.toggle_theme(cx);
            page
        });
        settle(&reader, cx);
        reader.update(cx, |reader, _| {
            assert_eq!(reader.current_page.as_ref().unwrap().start, old_page.start);
            assert_eq!(reader.current_page.as_ref().unwrap().end, old_page.end);
        });
        // A page turn must work after the archive is removed: no file read or
        // chapter parse is needed for cached content.
        std::fs::remove_file(dir.path().join("book.epub")).unwrap_or(());
        cx.dispatch_action(EpubNext);
        settle(&reader, cx);
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
        super::super::tests::write_book(
            &other_path,
            "3.0",
            "",
            &[(
                "other.xhtml",
                "<html><body><p>A different book.</p></body></html>",
            )],
            &[],
        );
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
                ..Location::default()
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
        let geometry = PageGeometry::with_width(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            900.0,
        );
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
                assert!(
                    *width <= geometry.width && *height <= geometry.height,
                    "image {width}x{height}, geometry {geometry:?}, cached {:?}",
                    reader
                        .layouts
                        .get(&reader.href)
                        .map(|layout| layout.geometry)
                );
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
        let geometry = PageGeometry::with_width(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            900.0,
        );
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

    #[gpui::test]
    fn settings_and_keyboard_contents_preserve_reading_positions(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        cx.update(|cx| cx.bind_keys(crate::app::platform_key_bindings()));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        fixture(&path);
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Settings".into(), window, cx));
        settle(&reader, visual);
        visual.dispatch_action(EpubNext);
        settle(&reader, visual);
        let original = reader.update(visual, |reader, _| reader.anchor);
        visual.update(|window, cx| {
            reader.update(cx, |reader, cx| {
                reader.control(Control::Sans, window, cx);
                reader.control(Control::Spacing(2), window, cx);
                reader.control(Control::Width(-200), window, cx);
                reader.control(Control::Alignment, window, cx);
            })
        });
        settle(&reader, visual);
        reader.update(visual, |reader, _| {
            assert_eq!(reader.anchor, original);
            assert!(!reader.preferences.sans);
            assert_eq!(reader.preferences.line_spacing, 18);
            assert_eq!(reader.preferences.reading_width, 700);
            assert!(!reader.preferences.justify);
        });
        visual.dispatch_action(EpubContents);
        visual.run_until_parked();
        assert!(visual.debug_bounds("epub-chapter-popup").is_some());
        visual.simulate_keystrokes("down enter");
        settle(&reader, visual);
        reader.update(visual, |reader, _| {
            assert!(!reader.chapters_open);
            assert!(!reader.history.is_empty());
        });
        visual.dispatch_action(EpubBack);
        settle(&reader, visual);
        reader.update(visual, |reader, _| assert_eq!(reader.anchor, original));
        visual.dispatch_action(EpubSettings);
        visual.simulate_resize(size(px(360.0), px(240.0)));
        settle(&reader, visual);
        let popup = visual.debug_bounds("epub-reading-settings").unwrap();
        assert!(popup.top() >= px(0.0));
        assert!(popup.left() >= px(0.0));
        assert!(popup.right() <= px(360.0));
        visual.dispatch_action(EpubDismiss);
        reader.update(visual, |reader, _| assert!(!reader.settings_open));
    }

    #[gpui::test]
    fn native_search_input_results_and_return_keep_the_original_passage(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        cx.update(|cx| cx.bind_keys(crate::app::platform_key_bindings()));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("search.epub");
        super::super::tests::write_book(
            &path,
            "3.0",
            "",
            &[
                (
                    "a.xhtml",
                    "<html><body><p>Original passage.</p></body></html>",
                ),
                (
                    "b.xhtml",
                    "<html><body><p>A café in a different chapter.</p></body></html>",
                ),
            ],
            &[],
        );
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Search".into(), window, cx));
        settle(&reader, visual);
        visual.run_until_parked();
        let original = reader.update(visual, |reader, _| reader.location());
        visual.dispatch_action(EpubFind);
        visual.simulate_input("CAFÉ");
        visual.run_until_parked();
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(150));
        visual.run_until_parked();
        reader.update(visual, |reader, cx| {
            assert_eq!(reader.input.read(cx).text.content, "CAFÉ");
        });
        reader.update(visual, |reader, _| {
            assert_eq!(reader.search_results.len(), 1)
        });
        visual.simulate_keystrokes("enter");
        settle(&reader, visual);
        reader.update(visual, |reader, _| {
            assert_eq!(reader.href, "/book/b.xhtml");
            assert!(reader.search_highlight.is_some());
        });
        visual.update(|window, cx| {
            reader.update(cx, |reader, cx| reader.control(Control::Return, window, cx))
        });
        settle(&reader, visual);
        reader.update(visual, |reader, _| {
            assert_eq!(reader.href, original.href);
            assert_eq!(reader.anchor, original.point);
            assert!(reader.search_highlight.is_none());
        });
    }

    #[gpui::test]
    fn named_bookmarks_and_note_previews_keep_the_reader_at_its_passage(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        cx.update(|cx| cx.bind_keys(crate::app::platform_key_bindings()));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.epub");
        super::super::tests::write_book(&path,"3.0","",&[("c.xhtml",r##"<html xmlns:epub="http://www.idpf.org/2007/ops"><body><p>Read <a epub:type="noteref" href="notes.xhtml#n">1</a> here.</p><p>More prose.</p></body></html>"##)],&[("notes.xhtml","application/xhtml+xml",br#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body><aside epub:type="footnote" id="n" style="display:none"><p>The full note.</p><p>Second paragraph.</p></aside></body></html>"#)]);
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path.clone(), "Notes".into(), window, cx));
        settle(&reader, visual);
        let original = reader.update(visual, |reader, _| reader.location());
        visual.dispatch_action(EpubBookmark);
        visual.simulate_input("My passage");
        visual.simulate_keystrokes("enter");
        reader.update(visual, |reader, cx| {
            let bookmarks = state::bookmarks(cx, &reader.key);
            assert_eq!(bookmarks.len(), 1);
            assert_eq!(bookmarks[0].name, "My passage");
            assert!(!bookmarks[0].location.quote.is_empty());
        });
        visual.run_until_parked();
        let bounds = visual.debug_bounds("epub-body").unwrap();
        let link = reader.update(visual, |reader, _| {
            let geometry = reader.layouts[&reader.href].geometry;
            let PageItem::Text { line, x, y, .. } = &reader.current_page.as_ref().unwrap().items[0]
            else {
                panic!()
            };
            bounds.origin
                + point(
                    px(geometry.margin_x + x) + line.x_for_index(5) + px(1.0),
                    px(geometry.margin_y + y + 8.0),
                )
        });
        visual.simulate_click(link, Modifiers::default());
        visual.run_until_parked();
        reader.update(visual, |reader, _| {
            assert_eq!(reader.href, original.href);
            assert_eq!(reader.anchor, original.point);
            let text = &reader.note.as_ref().unwrap().1;
            assert!(text.contains("The full note.") && text.contains("Second paragraph."));
        });
        visual.dispatch_action(EpubDismiss);
        visual.update(|window, _| window.remove_window());
        let (reopened, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Reopened".into(), window, cx));
        settle(&reopened, visual);
        reopened.update(visual, |reader, cx| {
            assert_eq!(state::bookmarks(cx, &reader.key).len(), 1);
            assert_eq!(reader.anchor, original.point);
        });
    }

    #[test]
    fn double_click_selection_handles_unicode_and_word_boundaries() {
        let chapter = super::super::book::parse_chapter(
            "<html><body><p>one café two</p></body></html>",
            "c",
            &AtomicBool::new(false),
        )
        .unwrap();
        let (a, b) = word_selection(
            &chapter,
            ContentPoint {
                block: 0,
                offset: 6,
            },
        )
        .unwrap();
        assert_eq!(selected_text(&chapter, a, b), "café");
    }
    #[gpui::test]
    fn tab_enter_and_space_activate_controls_without_turning_twice(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        cx.update(|cx| cx.bind_keys(crate::app::platform_key_bindings()));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keyboard.epub");
        fixture(&path);
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Keyboard".into(), window, cx));
        settle(&reader, visual);
        visual.simulate_keystrokes("tab tab tab tab tab tab enter");
        settle(&reader, visual);
        reader.update(visual, |reader, _| assert_eq!(reader.page_index, 1));
        visual.simulate_keystrokes("space");
        settle(&reader, visual);
        reader.update(visual, |reader, _| assert_eq!(reader.page_index, 2));
        visual
            .update(|window, cx| reader.update(cx, |reader, _| reader.focus_handle.focus(window)));
        visual.dispatch_action(EpubFullscreen);
        visual.run_until_parked();
        reader.update(visual, |reader, _| assert!(reader.fullscreen));
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        reader.update(visual, |reader, _| assert!(!reader.fullscreen));
    }

    #[gpui::test]
    fn broken_sections_can_retry_skip_and_return_to_reading(cx: &mut TestAppContext) {
        cx.update(state::initialize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.epub");
        super::super::tests::write_book(
            &path,
            "3.0",
            "",
            &[
                ("a.xhtml", "<html><body><p>Before</p></body></html>"),
                ("b.xhtml", "<html><body><p>Broken"),
                ("c.xhtml", "<html><body><p>After</p></body></html>"),
            ],
            &[],
        );
        let (reader, visual) =
            cx.add_window_view(|window, cx| Reader::new(path, "Recovery".into(), window, cx));
        settle(&reader, visual);
        reader.update(visual, |reader, cx| {
            reader.follow_link("/book/b.xhtml".into(), cx)
        });
        visual.run_until_parked();
        reader.update(visual, |reader, _| {
            assert!(reader.error.is_some());
            assert_eq!(reader.heading(), "Section 2");
        });
        visual.update(|window, cx| {
            reader.update(cx, |reader, cx| {
                reader.control(Control::Retry, window, cx);
                assert!(reader.error.is_none());
            })
        });
        visual.run_until_parked();
        reader.update(visual, |reader, _| assert!(reader.error.is_some()));
        visual.update(|window, cx| {
            reader.update(cx, |reader, cx| reader.control(Control::Skip, window, cx))
        });
        settle(&reader, visual);
        reader.update(visual, |reader, _| {
            assert_eq!(reader.href, "/book/c.xhtml");
            assert!(reader.error.is_none());
        });
        visual.update(|window, cx| {
            reader.update(cx, |reader, cx| reader.control(Control::Back, window, cx))
        });
        settle(&reader, visual);
        reader.update(visual, |reader, _| {
            assert_eq!(reader.href, "/book/a.xhtml");
            assert!(reader.error.is_none());
        });
    }
}
