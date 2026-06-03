use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::Image;

use crate::explorer::entry::FileEntry;

#[derive(Default)]
pub(super) struct AppIconCache {
    icons: HashMap<PathBuf, Option<Arc<Image>>>,
}

impl AppIconCache {
    pub(super) fn icon_for_entry(&mut self, entry: &FileEntry) -> Option<Arc<Image>> {
        if !entry.uses_app_bundle_icon() {
            return None;
        }

        let path = entry.path.clone();
        self.icons
            .entry(path.clone())
            .or_insert_with(|| load_app_bundle_icon(&path))
            .clone()
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

            let length = data.length();
            if length == 0 {
                return None;
            }

            let bytes = data.bytes().cast::<u8>();
            let bytes = std::slice::from_raw_parts(bytes, length as usize).to_vec();
            Some(Arc::new(Image::from_bytes(ImageFormat::Tiff, bytes)))
        })();
        let _: () = msg_send![pool, drain];
        result
    }
}

#[cfg(not(target_os = "macos"))]
fn load_app_bundle_icon(_: &Path) -> Option<Arc<Image>> {
    None
}
