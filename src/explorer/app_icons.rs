use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use gpui::{Context, Image, Task};

use crate::explorer::{entry::FileEntry, view::ExplorerView};

const APP_ICON_LOAD_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Default)]
pub(super) struct AppIconCache {
    icons: HashMap<PathBuf, AppIconState>,
    pending: VecDeque<PathBuf>,
    loader_running: bool,
    loader_task: Option<Task<()>>,
}

impl AppIconCache {
    fn icon_for_entry(&mut self, entry: &FileEntry) -> Option<Arc<Image>> {
        if !entry.uses_app_bundle_icon() {
            return None;
        }

        let path = entry.path.clone();

        match self.icons.get(&path) {
            Some(AppIconState::Ready(icon)) => Some(icon.clone()),
            Some(AppIconState::Pending | AppIconState::Loading | AppIconState::Failed) => None,
            None => {
                self.icons.insert(path.clone(), AppIconState::Pending);
                self.pending.push_back(path);
                None
            }
        }
    }

    fn start_loader(&mut self) -> bool {
        if self.loader_running || self.pending.is_empty() {
            return false;
        }

        self.loader_running = true;
        true
    }

    fn store_loader_task(&mut self, task: Task<()>) {
        self.loader_task = Some(task);
    }

    fn finish_loader(&mut self) {
        self.loader_running = false;
    }

    fn next_pending_path(&mut self) -> Option<PathBuf> {
        while let Some(path) = self.pending.pop_front() {
            if matches!(self.icons.get(&path), Some(AppIconState::Pending)) {
                self.icons.insert(path.clone(), AppIconState::Loading);
                return Some(path);
            }
        }

        None
    }

    fn finish_icon(&mut self, path: PathBuf, icon: Option<Arc<Image>>) {
        self.pending.retain(|pending_path| pending_path != &path);

        let state = match icon {
            Some(icon) => AppIconState::Ready(icon),
            None => AppIconState::Failed,
        };

        self.icons.insert(path, state);
    }
}

enum AppIconState {
    Pending,
    Loading,
    Ready(Arc<Image>),
    Failed,
}

impl ExplorerView {
    pub(super) fn app_icon_for_entry(
        &mut self,
        entry: &FileEntry,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Image>> {
        let icon = self.app_icon_cache.icon_for_entry(entry);

        if self.app_icon_cache.start_loader() {
            let task = cx.spawn(async move |this, cx| {
                loop {
                    let should_continue = this
                        .update(cx, |explorer, cx| explorer.load_next_app_icon(cx))
                        .unwrap_or(false);

                    if !should_continue {
                        break;
                    }

                    cx.background_executor().timer(APP_ICON_LOAD_INTERVAL).await;
                }
            });
            self.app_icon_cache.store_loader_task(task);
        }

        icon
    }

    fn load_next_app_icon(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(path) = self.app_icon_cache.next_pending_path() else {
            self.app_icon_cache.finish_loader();
            return false;
        };

        let icon = load_app_bundle_icon(&path);
        self.app_icon_cache.finish_icon(path, icon);
        cx.notify();
        true
    }
}

#[cfg(target_os = "macos")]
fn load_app_bundle_icon(path: &Path) -> Option<Arc<Image>> {
    use cocoa::{
        appkit::NSImage,
        base::{id, nil},
        foundation::{NSData, NSString},
    };
    use gpui::ImageFormat;
    use objc::{class, msg_send, sel, sel_impl};

    let path = path.to_str()?;

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let result = (|| {
            let ns_path = NSString::alloc(nil).init_str(path);
            let _: id = msg_send![ns_path, autorelease];

            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            if workspace == nil {
                return None;
            }

            let icon: id = msg_send![workspace, iconForFile: ns_path];
            if icon == nil {
                return None;
            }

            let data = icon.TIFFRepresentation();
            if data == nil {
                return None;
            }

            let bitmap_rep: id = msg_send![class!(NSBitmapImageRep), alloc];
            let bitmap_rep: id = msg_send![bitmap_rep, initWithData: data];
            if bitmap_rep == nil {
                return None;
            }
            let _: id = msg_send![bitmap_rep, autorelease];

            let png_file_type = 4usize;
            let data: id = msg_send![
                bitmap_rep,
                representationUsingType: png_file_type
                properties: nil
            ];
            if data == nil {
                return None;
            }

            let length = data.length();
            if length == 0 {
                return None;
            }

            let bytes = data.bytes().cast::<u8>();
            let bytes = std::slice::from_raw_parts(bytes, length as usize).to_vec();
            Some(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)))
        })();
        let _: () = msg_send![pool, drain];
        result
    }
}

#[cfg(not(target_os = "macos"))]
fn load_app_bundle_icon(_: &Path) -> Option<Arc<Image>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::ImageFormat;

    fn one_pixel_png() -> Arc<Image> {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            vec![
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0,
                1, 0, 0, 5, 0, 1, 13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
            ],
        ))
    }

    #[test]
    fn cache_schedules_app_bundle_once() {
        let mut cache = AppIconCache::default();
        let entry = FileEntry::test("Preview.app", true, None, None);

        assert!(cache.icon_for_entry(&entry).is_none());
        assert_eq!(cache.pending.len(), 1);

        assert!(cache.icon_for_entry(&entry).is_none());
        assert_eq!(cache.pending.len(), 1);

        assert_eq!(
            cache.next_pending_path(),
            Some(PathBuf::from("Preview.app"))
        );
        assert_eq!(cache.pending.len(), 0);
        assert!(cache.icon_for_entry(&entry).is_none());
    }

    #[test]
    fn failed_icon_loads_fall_back_without_rescheduling() {
        let mut cache = AppIconCache::default();
        let entry = FileEntry::test("Preview.app", true, None, None);

        assert!(cache.icon_for_entry(&entry).is_none());
        cache.finish_icon(PathBuf::from("Preview.app"), None);

        assert!(cache.icon_for_entry(&entry).is_none());
        assert!(cache.pending.is_empty());
    }

    #[test]
    fn ready_icon_loads_are_reused() {
        let mut cache = AppIconCache::default();
        let entry = FileEntry::test("Preview.app", true, None, None);
        let icon = one_pixel_png();

        assert!(cache.icon_for_entry(&entry).is_none());
        cache.finish_icon(PathBuf::from("Preview.app"), Some(icon));

        assert!(cache.icon_for_entry(&entry).is_some());
        assert!(cache.pending.is_empty());
    }

    #[test]
    fn non_app_directories_do_not_schedule_icon_loads() {
        let mut cache = AppIconCache::default();
        let entry = FileEntry::test("Folder", true, None, None);

        assert!(cache.icon_for_entry(&entry).is_none());
        assert!(cache.pending.is_empty());
    }
}
