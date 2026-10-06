//! Dynamic resources must not use GPUI's application-wide retain-all asset map.
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
};

use futures::{FutureExt, future::Shared};
use gpui::{
    App, AppContext, Asset, AssetLogger, Entity, Global, ImageAssetLoader, ImageCache,
    ImageCacheError, ImageCacheItem, RenderImage, Resource, Task, Window, hash,
};

use super::image_memory::{ImageRetention, image_bytes, retire};

#[derive(Default)]
struct ResourceImages(Option<Entity<BoundedImageCache>>);
impl Global for ResourceImages {}

pub(super) struct BoundedImageCache {
    items: HashMap<u64, ImageCacheItem>,
    retention: ImageRetention,
    retired_images: Vec<Arc<RenderImage>>,
}

pub(super) fn cache(cx: &mut App) -> Entity<BoundedImageCache> {
    if let Some(cache) = cx.default_global::<ResourceImages>().0.as_ref() {
        return cache.clone();
    }
    let cache = cx.new(|_| BoundedImageCache {
        items: HashMap::new(),
        retention: ImageRetention::new(8 * 1024 * 1024, 512),
        retired_images: Vec::new(),
    });
    cx.default_global::<ResourceImages>().0 = Some(cache.clone());
    cache
}

/// The element and its owning view hold the resolved image, keeping a menu pinned
/// when GPUI reuses its scene without calling the loader again.
pub(super) fn source(
    resource: Resource,
    owner: gpui::WeakEntity<super::view::ExplorerView>,
    cx: &mut App,
) -> gpui::ImageSource {
    let cache = cache(cx);
    let resolved = RefCell::new(None::<Arc<RenderImage>>);
    gpui::ImageSource::from(move |window: &mut Window, cx: &mut App| {
        if let Some(image) = resolved.borrow().as_ref() {
            return Some(Ok(image.clone()));
        }
        let result = cache.update(cx, |cache, cx| cache.load(&resource, window, cx));
        if let Some(Ok(image)) = &result {
            let _ = owner.update(cx, |view, _| {
                view.resource_image_leases
                    .borrow_mut()
                    .insert(image.id, image.clone());
            });
            *resolved.borrow_mut() = Some(image.clone());
        }
        result
    })
}

impl BoundedImageCache {
    fn trim(&mut self) {
        // Resolve finished tasks even if their requesting menu has closed.
        let mut just_loaded = HashSet::new();
        for (key, item) in &mut self.items {
            if matches!(item, ImageCacheItem::Loading(_)) {
                if let Some(result) = item.get() {
                    just_loaded.insert(*key);
                    self.retention.insert(
                        key.to_string(),
                        result.as_ref().map_or(0, |image| image_bytes(image)),
                    );
                }
            }
        }
        let items = &self.items;
        let keys = self.retention.evict(|key| {
            if key
                .parse()
                .ok()
                .is_some_and(|key| just_loaded.contains(&key))
            {
                return true;
            }
            key.parse().ok().and_then(|key| items.get(&key)).is_some_and(|item| {
                matches!(item, ImageCacheItem::Loaded(Ok(image)) if Arc::strong_count(image) > 1)
            })
        });
        for key in keys {
            if let Some(ImageCacheItem::Loaded(Ok(image))) =
                self.items.remove(&key.parse().unwrap())
            {
                self.retired_images.push(image);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn clearing_detaches_cache_releases_images_and_reopening_uses_new_entity(
        cx: &mut gpui::TestAppContext,
    ) {
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::new(1, 1),
        )]));
        let held = image.clone();
        let weak = Arc::downgrade(&image);
        let old = cx.update(|cx| {
            let cache = cache(cx);
            cache.update(cx, |cache, _| {
                cache.retention.insert("1".into(), 4);
                cache.items.insert(1, ImageCacheItem::Loaded(Ok(image)));
            });
            clear_memory(cx);
            cache
        });
        cx.update(|cx| {
            old.update(cx, |cache, _| {
                assert_eq!(cache.items.capacity(), 0);
                assert_eq!(cache.retention.retained_bytes(), 0);
            });
            assert_ne!(cache(cx).entity_id(), old.entity_id());
            super::super::image_memory::trim_memory(cx);
        });
        assert!(weak.upgrade().is_some());
        drop(held);
        cx.update(super::super::image_memory::trim_memory);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn dynamic_resource_pixels_and_failure_entries_are_bounded() {
        let mut cache = BoundedImageCache {
            items: HashMap::new(),
            retention: ImageRetention::new(8 * 1024 * 1024, 512),
            retired_images: Vec::new(),
        };
        for i in 0..5000u64 {
            let image = Arc::new(RenderImage::new(vec![image::Frame::new(
                image::RgbaImage::new(128, 128),
            )]));
            cache.retention.insert(i.to_string(), image_bytes(&image));
            cache.items.insert(i, ImageCacheItem::Loaded(Ok(image)));
            cache.trim();
            cache.retired_images.clear();
            assert!(cache.retention.retained_bytes() <= 8 * 1024 * 1024);
            assert!(cache.items.len() <= 512);
        }
        for i in 5000..10000u64 {
            cache.retention.insert(i.to_string(), 0);
            cache.items.insert(
                i,
                ImageCacheItem::Loaded(Err(std::io::Error::other("missing").into())),
            );
            cache.trim();
            cache.retired_images.clear();
            assert!(cache.items.len() <= 512);
        }
    }
}

impl ImageCache for BoundedImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let key = hash(resource);
        self.retention.touch(&key.to_string());
        if let Some(item) = self.items.get_mut(&key) {
            let was_loading = matches!(item, ImageCacheItem::Loading(_));
            let result = item.get();
            if was_loading {
                if let Some(result) = &result {
                    self.retention.insert(
                        key.to_string(),
                        result.as_ref().map_or(0, |image| image_bytes(image)),
                    );
                }
            }
            self.trim();
            retire(std::mem::take(&mut self.retired_images), cx);
            return result;
        }
        let future = AssetLogger::<ImageAssetLoader>::load(resource.clone(), cx);
        let task: Shared<Task<_>> = cx.background_executor().spawn(future).shared();
        self.items
            .insert(key, ImageCacheItem::Loading(task.clone()));
        let entity = window.current_view();
        window
            .spawn(cx, async move |cx| {
                _ = task.await;
                cx.on_next_frame(move |_, cx| cx.notify(entity));
            })
            .detach();
        None
    }
}

pub(super) fn trim_memory(cx: &mut App) {
    let cache = cx
        .try_global::<ResourceImages>()
        .and_then(|global| global.0.clone());
    if let Some(cache) = cache {
        let images = cache.update(cx, |cache, _| {
            cache.trim();
            std::mem::take(&mut cache.retired_images)
        });
        retire(images, cx);
    }
}

pub(super) fn clear_memory(cx: &mut App) {
    let cache = cx
        .try_global::<ResourceImages>()
        .and_then(|global| global.0.clone());
    if let Some(cache) = cache {
        cx.global_mut::<ResourceImages>().0 = None;
        let images = cache.update(cx, |cache, _| {
            cache.retention.clear();
            for mut item in std::mem::take(&mut cache.items).into_values() {
                if let Some(Ok(image)) = item.get() {
                    cache.retired_images.push(image);
                }
            }
            std::mem::take(&mut cache.retired_images)
        });
        retire(images, cx);
    }
}
