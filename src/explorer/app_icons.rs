use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[cfg(any(target_os = "macos", test))]
use std::fs;
#[cfg(any(target_os = "macos", test))]
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{Context, Image, Task};

use crate::explorer::{entry::FileEntry, view::ExplorerView};

const APP_ICON_LOAD_INTERVAL: Duration = Duration::from_millis(16);
#[cfg(any(target_os = "macos", test))]
const APP_ICON_CACHE_VERSION: &str = "app-icon-v2-32px";
#[cfg(target_os = "macos")]
const APP_ICON_PNG_SIZE: f64 = 32.0;
#[cfg(any(target_os = "macos", test))]
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

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

#[cfg(any(target_os = "macos", test))]
fn image_from_png_bytes(bytes: Vec<u8>) -> Arc<Image> {
    Arc::new(Image::from_bytes(gpui::ImageFormat::Png, bytes))
}

#[cfg(target_os = "macos")]
fn load_app_bundle_icon(path: &Path) -> Option<Arc<Image>> {
    load_app_bundle_icon_png_bytes(path).map(image_from_png_bytes)
}

#[cfg(not(target_os = "macos"))]
fn load_app_bundle_icon(_: &Path) -> Option<Arc<Image>> {
    None
}

#[cfg(target_os = "macos")]
fn load_app_bundle_icon_png_bytes(path: &Path) -> Option<Vec<u8>> {
    let icon_path = resolve_bundle_icon_path(path);
    let cache_key = icon_cache_key(path, icon_path.as_deref());

    if let Some(bytes) = read_cached_icon(&cache_key) {
        return Some(bytes);
    }

    let bytes = icon_path
        .as_deref()
        .and_then(load_icon_from_icns)
        .or_else(|| load_icon_from_workspace(path))?;

    write_cached_icon(&cache_key, &bytes);
    Some(bytes)
}

#[cfg(target_os = "macos")]
fn resolve_bundle_icon_path(app_path: &Path) -> Option<PathBuf> {
    let info_path = app_path.join("Contents").join("Info.plist");
    let icon_name = bundle_icon_name_from_plist(&info_path)?;
    bundle_icon_resource_path(app_path, &icon_name)
}

#[cfg(target_os = "macos")]
fn bundle_icon_name_from_plist(info_path: &Path) -> Option<String> {
    let plist = plist::Value::from_file(info_path).ok()?;
    plist
        .as_dictionary()?
        .get("CFBundleIconFile")?
        .as_string()
        .and_then(non_empty_icon_name)
}

#[cfg(any(target_os = "macos", test))]
fn non_empty_icon_name(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(any(target_os = "macos", test))]
fn bundle_icon_resource_path(app_path: &Path, icon_name: &str) -> Option<PathBuf> {
    let icon_name = non_empty_icon_name(icon_name)?;
    let mut icon_path = PathBuf::from(icon_name);
    if icon_path.extension().is_none() {
        icon_path.set_extension("icns");
    }

    Some(app_path.join("Contents").join("Resources").join(icon_path))
}

#[cfg(target_os = "macos")]
fn load_icon_from_icns(icon_path: &Path) -> Option<Vec<u8>> {
    use icns::{IconFamily, IconType};

    let icon_family = IconFamily::read(fs::File::open(icon_path).ok()?).ok()?;
    let icon_type = preferred_icon_type(&icon_family.available_icons())?;
    let image = icon_family.get_icon_with_type(icon_type).ok()?;
    let mut bytes = Vec::new();
    image.write_png(&mut bytes).ok()?;
    valid_png_bytes(bytes)
}

#[cfg(target_os = "macos")]
fn preferred_icon_type(available_icons: &[icns::IconType]) -> Option<icns::IconType> {
    use icns::IconType;

    let preferred = [
        IconType::RGBA32_32x32_2x,
        IconType::RGBA32_32x32,
        IconType::RGBA32_16x16_2x,
        IconType::RGBA32_16x16,
        IconType::RGB24_32x32,
        IconType::RGB24_16x16,
        IconType::RGBA32_64x64,
        IconType::RGBA32_128x128,
        IconType::RGBA32_128x128_2x,
        IconType::RGBA32_256x256,
        IconType::RGBA32_256x256_2x,
        IconType::RGBA32_512x512,
        IconType::RGBA32_512x512_2x,
        IconType::RGB24_48x48,
        IconType::RGB24_128x128,
    ];

    preferred
        .into_iter()
        .find(|icon_type| available_icons.contains(icon_type))
        .or_else(|| {
            available_icons.iter().copied().min_by_key(|icon_type| {
                let width = icon_type.screen_width() as i64;
                ((width - APP_ICON_PNG_SIZE as i64).abs(), width)
            })
        })
}

#[cfg(target_os = "macos")]
fn load_icon_from_workspace(path: &Path) -> Option<Vec<u8>> {
    use cocoa::{
        appkit::{NSCompositingOperation, NSImage},
        base::{NO, YES, id, nil},
        foundation::{NSData, NSPoint, NSRect, NSSize, NSString},
    };
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

            small_png_from_ns_image(icon)
        })();
        let _: () = msg_send![pool, drain];
        result
    }
}

#[cfg(target_os = "macos")]
unsafe fn small_png_from_ns_image(icon: cocoa::base::id) -> Option<Vec<u8>> {
    use cocoa::{
        appkit::{NSCompositingOperation, NSImage},
        base::{NO, YES, id, nil},
        foundation::{NSData, NSPoint, NSRect, NSSize, NSString},
    };
    use objc::{class, msg_send, sel, sel_impl};

    let size = APP_ICON_PNG_SIZE as isize;
    let color_space = NSString::alloc(nil).init_str("NSDeviceRGBColorSpace");
    let _: id = msg_send![color_space, autorelease];

    let bitmap_rep: id = msg_send![class!(NSBitmapImageRep), alloc];
    let bitmap_rep: id = msg_send![
        bitmap_rep,
        initWithBitmapDataPlanes: nil
        pixelsWide: size
        pixelsHigh: size
        bitsPerSample: 8isize
        samplesPerPixel: 4isize
        hasAlpha: YES
        isPlanar: NO
        colorSpaceName: color_space
        bitmapFormat: 0usize
        bytesPerRow: 0isize
        bitsPerPixel: 0isize
    ];
    if bitmap_rep == nil {
        return None;
    }
    let _: id = msg_send![bitmap_rep, autorelease];

    let graphics_context: id =
        msg_send![class!(NSGraphicsContext), graphicsContextWithBitmapImageRep: bitmap_rep];
    if graphics_context == nil {
        return None;
    }

    let _: () = msg_send![class!(NSGraphicsContext), saveGraphicsState];
    let _: () = msg_send![class!(NSGraphicsContext), setCurrentContext: graphics_context];

    let destination = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(APP_ICON_PNG_SIZE, APP_ICON_PNG_SIZE),
    );
    let source = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
    icon.drawInRect_fromRect_operation_fraction_(
        destination,
        source,
        NSCompositingOperation::NSCompositeSourceOver,
        1.0,
    );

    let _: () = msg_send![class!(NSGraphicsContext), restoreGraphicsState];

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
    valid_png_bytes(bytes)
}

#[cfg(any(target_os = "macos", test))]
fn valid_png_bytes(bytes: Vec<u8>) -> Option<Vec<u8>> {
    bytes.starts_with(PNG_SIGNATURE).then_some(bytes)
}

#[cfg(target_os = "macos")]
fn read_cached_icon(cache_key: &str) -> Option<Vec<u8>> {
    read_cached_icon_from_dir(icon_cache_dir().as_deref(), cache_key)
}

#[cfg(any(target_os = "macos", test))]
fn read_cached_icon_from_dir(cache_dir: Option<&Path>, cache_key: &str) -> Option<Vec<u8>> {
    let path = icon_cache_file_path_from_dir(cache_dir, cache_key)?;
    fs::read(path).ok().and_then(valid_png_bytes)
}

#[cfg(target_os = "macos")]
fn write_cached_icon(cache_key: &str, bytes: &[u8]) {
    write_cached_icon_to_dir(icon_cache_dir().as_deref(), cache_key, bytes);
}

#[cfg(any(target_os = "macos", test))]
fn write_cached_icon_to_dir(cache_dir: Option<&Path>, cache_key: &str, bytes: &[u8]) {
    let Some(path) = icon_cache_file_path_from_dir(cache_dir, cache_key) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };

    if fs::create_dir_all(parent).is_ok() {
        let _ = fs::write(path, bytes);
    }
}

#[cfg(any(target_os = "macos", test))]
fn icon_cache_file_path_from_dir(cache_dir: Option<&Path>, cache_key: &str) -> Option<PathBuf> {
    cache_dir.map(|cache_dir| cache_dir.join(format!("{cache_key}.png")))
}

#[cfg(target_os = "macos")]
fn icon_cache_dir() -> Option<PathBuf> {
    icon_cache_dir_for(|name| std::env::var_os(name).map(PathBuf::from))
}

#[cfg(any(target_os = "macos", test))]
fn icon_cache_dir_for(mut env_path: impl FnMut(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    env_path("HOME").map(|home| {
        home.join(".config")
            .join("explorer")
            .join("cache")
            .join("icons")
    })
}

#[cfg(any(target_os = "macos", test))]
fn icon_cache_key(app_path: &Path, icon_path: Option<&Path>) -> String {
    let mut hash = StableHash::new();
    hash.write_str(APP_ICON_CACHE_VERSION);
    hash.write_path(app_path);
    hash.write_metadata(app_path);

    let info_path = app_path.join("Contents").join("Info.plist");
    hash.write_path(&info_path);
    hash.write_metadata(&info_path);

    match icon_path {
        Some(icon_path) => {
            hash.write_u8(1);
            hash.write_path(icon_path);
            hash.write_metadata(icon_path);
        }
        None => hash.write_u8(0),
    }

    format!("{:016x}", hash.finish())
}

#[cfg(any(target_os = "macos", test))]
struct StableHash(u64);

#[cfg(any(target_os = "macos", test))]
impl StableHash {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn finish(self) -> u64 {
        self.0
    }

    fn write_u8(&mut self, value: u8) {
        self.write_bytes(&[value]);
    }

    fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_le_bytes());
    }

    fn write_str(&mut self, value: &str) {
        self.write_bytes(value.as_bytes());
        self.write_u8(0xff);
    }

    fn write_path(&mut self, path: &Path) {
        self.write_str(&path.to_string_lossy());
    }

    fn write_metadata(&mut self, path: &Path) {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                self.write_u8(1);
                self.write_u64(metadata.len());
                write_system_time(self, metadata.modified().ok());
            }
            Err(_) => self.write_u8(0),
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn write_system_time(hash: &mut StableHash, time: Option<SystemTime>) {
    match time {
        Some(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => {
                hash.write_u8(1);
                hash.write_u64(duration.as_secs());
                hash.write_u64(u64::from(duration.subsec_nanos()));
            }
            Err(error) => {
                let duration = error.duration();
                hash.write_u8(2);
                hash.write_u64(duration.as_secs());
                hash.write_u64(u64::from(duration.subsec_nanos()));
            }
        },
        None => hash.write_u8(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;

    fn one_pixel_png_bytes() -> Vec<u8> {
        vec![
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0,
            5, 0, 1, 13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ]
    }

    fn one_pixel_png() -> Arc<Image> {
        image_from_png_bytes(one_pixel_png_bytes())
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

    #[test]
    fn icon_resource_path_adds_icns_extension_when_missing() {
        assert_eq!(
            bundle_icon_resource_path(Path::new("/Applications/Preview.app"), "Preview"),
            Some(PathBuf::from(
                "/Applications/Preview.app/Contents/Resources/Preview.icns"
            ))
        );
    }

    #[test]
    fn icon_resource_path_keeps_existing_icns_extension() {
        assert_eq!(
            bundle_icon_resource_path(Path::new("/Applications/Preview.app"), "Preview.icns"),
            Some(PathBuf::from(
                "/Applications/Preview.app/Contents/Resources/Preview.icns"
            ))
        );
    }

    #[test]
    fn icon_resource_path_ignores_empty_icon_names() {
        assert_eq!(
            bundle_icon_resource_path(Path::new("Preview.app"), ""),
            None
        );
        assert_eq!(
            bundle_icon_resource_path(Path::new("Preview.app"), "  "),
            None
        );
    }

    #[test]
    fn cache_dir_uses_requested_home_location() {
        assert_eq!(
            icon_cache_dir_for(|name| (name == "HOME").then(|| PathBuf::from("/home/test"))),
            Some(PathBuf::from("/home/test/.config/explorer/cache/icons"))
        );
    }

    #[test]
    fn cache_dir_is_disabled_without_home() {
        assert_eq!(icon_cache_dir_for(|_| None), None);
    }

    #[test]
    fn cached_icon_reads_valid_png_bytes() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("icons");
        fs::create_dir(&cache_dir).expect("create cache dir");
        fs::write(cache_dir.join("key.png"), one_pixel_png_bytes()).expect("write cached png");

        assert!(read_cached_icon_from_dir(Some(&cache_dir), "key").is_some());
    }

    #[test]
    fn cached_icon_ignores_missing_or_invalid_png_bytes() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("icons");
        fs::create_dir(&cache_dir).expect("create cache dir");
        fs::write(cache_dir.join("invalid.png"), b"not png").expect("write invalid png");

        assert!(read_cached_icon_from_dir(Some(&cache_dir), "missing").is_none());
        assert!(read_cached_icon_from_dir(Some(&cache_dir), "invalid").is_none());
    }

    #[test]
    fn cached_icon_write_creates_parent_directory() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("cache").join("icons");
        let bytes = one_pixel_png_bytes();

        write_cached_icon_to_dir(Some(&cache_dir), "key", &bytes);

        assert_eq!(fs::read(cache_dir.join("key.png")).ok(), Some(bytes));
    }

    #[test]
    fn cached_icon_write_is_no_op_without_cache_dir() {
        write_cached_icon_to_dir(None, "key", &one_pixel_png_bytes());
    }

    #[test]
    fn cache_key_changes_when_icon_metadata_changes() {
        let temp = TempDir::new();
        let app = temp.path().join("Preview.app");
        let info = app.join("Contents").join("Info.plist");
        let resources = app.join("Contents").join("Resources");
        let icon = resources.join("Preview.icns");
        fs::create_dir_all(&resources).expect("create resources");
        fs::write(&info, b"info").expect("write info");
        fs::write(&icon, b"icon").expect("write icon");

        let before = icon_cache_key(&app, Some(&icon));
        fs::write(&icon, b"changed icon").expect("change icon");
        let after = icon_cache_key(&app, Some(&icon));

        assert_ne!(before, after);
    }

    #[test]
    fn cache_key_changes_when_info_metadata_changes() {
        let temp = TempDir::new();
        let app = temp.path().join("Preview.app");
        let info = app.join("Contents").join("Info.plist");
        fs::create_dir_all(info.parent().expect("info parent")).expect("create contents");
        fs::write(&info, b"info").expect("write info");

        let before = icon_cache_key(&app, None);
        fs::write(&info, b"changed info").expect("change info");
        let after = icon_cache_key(&app, None);

        assert_ne!(before, after);
    }

    #[test]
    fn valid_png_bytes_rejects_non_png_data() {
        assert!(valid_png_bytes(b"not png".to_vec()).is_none());
        assert!(valid_png_bytes(PNG_SIGNATURE.to_vec()).is_some());
    }
}
