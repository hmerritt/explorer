//! Limits for decoded images, shared by the thumbnail and native icon caches.
use std::{cell::RefCell, collections::HashMap, sync::Arc, time::Duration};

use gpui::{App, Global, RenderImage};

#[derive(Default)]
struct RetiredImages(RefCell<HashMap<gpui::ImageId, Arc<RenderImage>>>);
impl Global for RetiredImages {}

#[derive(Default)]
struct MemoryCacheSession {
    epoch: u64,
    initialized: bool,
    window_cleanup_initialized: bool,
}
impl Global for MemoryCacheSession {}

pub(super) fn session(cx: &App) -> u64 {
    cx.try_global::<MemoryCacheSession>()
        .map_or(0, |session| session.epoch)
}

pub(crate) fn initialize_window_cleanup(cx: &mut App) {
    if cx
        .default_global::<MemoryCacheSession>()
        .window_cleanup_initialized
    {
        return;
    }
    cx.default_global::<MemoryCacheSession>()
        .window_cleanup_initialized = true;
    cx.on_window_closed(|cx| {
        if cx.windows().is_empty() {
            cx.defer(|cx| {
                // A launch request can reopen a window before teardown finishes.
                if cx.windows().is_empty() {
                    super::clear_memory_caches(cx);
                }
            });
        }
    })
    .detach();
}

pub(crate) fn clear_memory_caches(cx: &mut App) {
    let session = cx.default_global::<MemoryCacheSession>();
    session.epoch = session
        .epoch
        .checked_add(1)
        .expect("memory cache epoch exhausted");
    super::image_thumbnails::clear_memory(cx);
    super::app_icons::clear_memory(cx);
    super::resource_images::clear_memory(cx);
    super::folder_size::clear_memory(cx);
    super::properties::clear_checksum_memory(cx);
    super::remote_directory_cache::clear_memory();
    cx.clear_image_assets();
    trim_memory(cx);
}

pub(super) fn image_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count())
        .filter_map(|frame| image.as_bytes(frame))
        .map(<[u8]>::len)
        .sum()
}

/// Entries owned by a visible element or another consumer cannot be evicted.
pub(super) struct ImageRetention {
    entries: HashMap<String, (u64, usize)>,
    clock: u64,
    bytes: usize,
    max_bytes: usize,
    max_entries: usize,
}

impl ImageRetention {
    pub(super) fn new(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
            max_entries,
        }
    }

    pub(super) fn insert(&mut self, key: String, bytes: usize) {
        self.remove(&key);
        self.clock += 1;
        self.entries.insert(key, (self.clock, bytes));
        self.bytes += bytes;
    }

    pub(super) fn touch(&mut self, key: &str) {
        if let Some((last_used, _)) = self.entries.get_mut(key) {
            self.clock += 1;
            *last_used = self.clock;
        }
    }

    pub(super) fn remove(&mut self, key: &str) {
        if let Some((_, bytes)) = self.entries.remove(key) {
            self.bytes -= bytes;
        }
    }

    pub(super) fn clear(&mut self) {
        self.entries = HashMap::new();
        self.clock = 0;
        self.bytes = 0;
    }

    pub(super) fn evict(&mut self, mut is_pinned: impl FnMut(&str) -> bool) -> Vec<String> {
        if self.bytes <= self.max_bytes && self.entries.len() <= self.max_entries {
            return Vec::new();
        }
        let mut candidates = self
            .entries
            .iter()
            .filter(|(key, _)| !is_pinned(key))
            .map(|(key, (age, _))| (*age, key.clone()))
            .collect::<Vec<_>>();
        candidates.sort_unstable();
        let mut removed = Vec::new();
        for (_, key) in candidates {
            if self.bytes <= self.max_bytes && self.entries.len() <= self.max_entries {
                break;
            }
            self.remove(&key);
            removed.push(key);
        }
        removed
    }

    #[cfg(test)]
    pub(super) fn retained_bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub(super) fn set_limits(&mut self, max_bytes: usize, max_entries: usize) {
        self.max_bytes = max_bytes;
        self.max_entries = max_entries;
    }
}

pub(super) fn retire(images: impl IntoIterator<Item = Arc<RenderImage>>, cx: &mut App) {
    let retired = cx.default_global::<RetiredImages>();
    let mut pending = retired.0.borrow_mut();
    for image in images {
        // Multiple caches may relinquish the same shared image.
        pending.entry(image.id).or_insert(image);
    }
}

pub(super) fn initialize(cx: &mut App) {
    if cx.default_global::<MemoryCacheSession>().initialized {
        return;
    }
    cx.default_global::<MemoryCacheSession>().initialized = true;
    cx.default_global::<RetiredImages>();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if cx.update(trim_memory).is_err() {
                break;
            }
        }
    })
    .detach();
}

/// Call outside a window update so GPU cleanup includes every window.
pub(super) fn trim_memory(cx: &mut App) {
    super::image_thumbnails::trim_memory(cx);
    super::app_icons::trim_memory(cx);
    super::resource_images::trim_memory(cx);
    let pending = std::mem::take(&mut *cx.global::<RetiredImages>().0.borrow_mut());
    let mut still_live = HashMap::new();
    for image in pending.into_values() {
        if Arc::strong_count(&image) == 1 {
            cx.drop_image(image, None);
        } else {
            still_live.insert(image.id, image);
        }
    }
    cx.global::<RetiredImages>()
        .0
        .borrow_mut()
        .extend(still_live);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, IntoElement, ParentElement, Render, Window};

    struct ImageConsumers(Vec<Arc<RenderImage>>);

    fn seed_folder_size(cx: &mut App) {
        super::super::folder_size::initialize(cx);
        cx.global::<super::super::folder_size::FolderSizeCache>()
            .insert("cached-folder".into(), 42);
    }

    fn cached_folder_size(cx: &App) -> Option<u64> {
        cx.global::<super::super::folder_size::FolderSizeCache>()
            .get(std::path::Path::new("cached-folder"))
    }

    #[gpui::test]
    fn final_window_close_clears_caches_but_other_windows_keep_them(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            initialize_window_cleanup(cx);
            initialize_window_cleanup(cx);
            seed_folder_size(cx);
        });
        let explorer = cx.add_window(|_, _| ImageConsumers(Vec::new()));
        let dialog = cx.add_window(|_, _| ImageConsumers(Vec::new()));
        explorer
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.read(|cx| {
            assert_eq!(session(cx), 0);
            assert_eq!(cached_folder_size(cx), Some(42));
        });
        dialog
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.read(|cx| {
            assert!(cx.windows().is_empty());
            assert_eq!(session(cx), 1);
            assert_eq!(cached_folder_size(cx), None);
        });
        let reopened = cx.add_window(|_, _| ImageConsumers(Vec::new()));
        cx.update(seed_folder_size);
        assert_eq!(cx.read(cached_folder_size), Some(42));
        reopened
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        assert_eq!(cx.read(cached_folder_size), None);
        assert_eq!(cx.read(session), 2);
    }

    #[gpui::test]
    fn reopening_during_teardown_prevents_cache_cleanup(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        cx.update(|cx| {
            initialize_window_cleanup(cx);
            seed_folder_size(cx);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.open_window(gpui::WindowOptions::default(), |_, cx| {
                        cx.new(|_| ImageConsumers(Vec::new()))
                    })
                    .unwrap();
                }
            })
            .detach();
        });
        let window = cx.add_window(|_, _| ImageConsumers(Vec::new()));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.read(|cx| {
            assert_eq!(cx.windows().len(), 1);
            assert_eq!(session(cx), 0);
            assert_eq!(cached_folder_size(cx), Some(42));
        });
    }

    #[test]
    fn clearing_retention_releases_capacity_and_preserves_limits() {
        let mut retention = ImageRetention::new(4, 1);
        retention.insert("old".into(), 4);
        retention.clear();
        assert_eq!(retention.retained_bytes(), 0);
        assert_eq!(retention.entries.capacity(), 0);
        retention.insert("new".into(), 8);
        assert_eq!(retention.evict(|_| false), ["new"]);
    }

    impl Render for ImageConsumers {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div().children(self.0.iter().cloned().map(gpui::img))
        }
    }

    #[gpui::test]
    fn rendered_split_panes_and_another_window_keep_retired_images_live(
        cx: &mut gpui::TestAppContext,
    ) {
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::new(128, 128),
        )]));
        let weak = Arc::downgrade(&image);
        let pane_image = image.clone();
        let dialog_image = image.clone();
        let panes = cx.add_window(move |_, _| ImageConsumers(vec![pane_image.clone(), pane_image]));
        let dialog = cx.add_window(move |_, _| ImageConsumers(vec![dialog_image]));
        cx.background_executor.run_until_parked();
        cx.update(|cx| retire([image], cx));
        cx.update(trim_memory);
        assert!(weak.upgrade().is_some());
        panes
            .update(cx, |view, window, cx| {
                view.0.clear();
                window.refresh();
                cx.notify();
            })
            .unwrap();
        cx.background_executor.run_until_parked();
        cx.update(trim_memory);
        assert!(
            weak.upgrade().is_some(),
            "the other window still displays the image"
        );
        dialog
            .update(cx, |view, window, cx| {
                view.0.clear();
                window.refresh();
                cx.notify();
            })
            .unwrap();
        cx.background_executor.run_until_parked();
        cx.update(trim_memory);
        assert!(weak.upgrade().is_none());
    }

    #[gpui::test]
    fn retired_images_are_deduplicated_and_released_after_last_consumer(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let image = Arc::new(RenderImage::new(vec![image::Frame::new(
                image::RgbaImage::new(1, 1),
            )]));
            let window = image.clone();
            let dialog = image.clone();
            let weak = Arc::downgrade(&image);
            retire([image.clone(), image], cx);
            assert_eq!(cx.global::<RetiredImages>().0.borrow().len(), 1);
            trim_memory(cx);
            drop(window);
            trim_memory(cx);
            assert!(weak.upgrade().is_some());
            drop(dialog);
            trim_memory(cx);
            assert!(weak.upgrade().is_none());
            assert!(cx.global::<RetiredImages>().0.borrow().is_empty());
        });
    }

    #[test]
    fn least_recently_used_entries_and_failures_are_bounded() {
        let mut retention = ImageRetention::new(8, 2);
        retention.insert("first".into(), 4);
        retention.insert("second".into(), 4);
        retention.touch("first");
        retention.insert("failure".into(), 0);
        assert_eq!(retention.evict(|_| false), ["second"]);
        assert_eq!(retention.retained_bytes(), 4);
        for i in 0..5000 {
            retention.insert(i.to_string(), 0);
            retention.evict(|_| false);
        }
        assert_eq!(retention.entries.len(), 2);
    }

    #[test]
    fn live_images_can_exceed_budget_then_are_trimmed() {
        let mut retention = ImageRetention::new(4, 1);
        retention.insert("first".into(), 4);
        retention.insert("second".into(), 4);
        assert!(retention.evict(|_| true).is_empty());
        assert_eq!(retention.retained_bytes(), 8);
        assert_eq!(retention.evict(|key| key == "second"), ["first"]);
        assert_eq!(retention.retained_bytes(), 4);
    }
}
