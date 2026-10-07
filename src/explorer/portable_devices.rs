use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

use bytes::Bytes;
use chrono::{TimeZone, Utc};
use futures::{Stream, executor::block_on, stream};
use mtp_rs::mtp::{
    ByteRange, Capabilities, MtpDevice, MtpDeviceInfo, NewObjectInfo, ObjectHandle, StorageId,
};
use xxhash_rust::xxh3::xxh3_64;

use crate::explorer::entry::FileEntry;

const PORTABLE_OPEN_CACHE_DIRECTORY: &str = "portable-open-v1";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct LocationCapabilities {
    pub(super) can_upload: bool,
    pub(super) can_delete: bool,
    pub(super) can_rename: bool,
    pub(super) can_move: bool,
    pub(super) can_copy: bool,
    pub(super) can_create_folder: bool,
    pub(super) supports_partial_download: bool,
    pub(super) supports_thumbnails: bool,
    pub(super) supports_events: bool,
}

impl LocationCapabilities {
    pub(super) fn can_mutate(self) -> bool {
        self.can_upload
            || self.can_delete
            || self.can_rename
            || self.can_move
            || self.can_copy
            || self.can_create_folder
    }

    fn from_mtp(value: Capabilities, writable: bool) -> Self {
        Self {
            can_upload: writable && value.can_upload,
            can_delete: writable && value.can_delete,
            can_rename: writable && value.can_rename,
            can_move: writable && value.can_move,
            can_copy: writable && value.can_copy,
            can_create_folder: writable && value.can_create_folder,
            supports_partial_download: value.supports_partial_download,
            supports_thumbnails: value.supports_thumbnails,
            supports_events: value.supports_events,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PortableDeviceRoot {
    pub(super) label: String,
    pub(super) path: PathBuf,
    pub(super) unavailable_reason: Option<String>,
}

#[derive(Clone, Debug)]
struct PortableNode {
    device_location_id: u64,
    generation: u64,
    storage_id: Option<StorageId>,
    object_handle: Option<ObjectHandle>,
    labels: Vec<String>,
    is_directory: bool,
    size: Option<u64>,
    modified: Option<SystemTime>,
    capabilities: LocationCapabilities,
}

#[derive(Clone, Debug)]
pub(super) struct PortableMetadata {
    pub(super) name: String,
    pub(super) is_directory: bool,
    pub(super) size: Option<u64>,
    pub(super) modified: Option<SystemTime>,
}

#[derive(Clone)]
struct DeviceSession {
    generation: u64,
    device: MtpDevice,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DeviceOpenTarget {
    #[cfg(not(target_os = "windows"))]
    Serial(String),
    Location(u64),
}

#[derive(Default)]
struct PortableDeviceService {
    state: Mutex<PortableDeviceState>,
}

#[derive(Default)]
struct PortableDeviceState {
    generation: u64,
    discovered: HashMap<u64, MtpDeviceInfo>,
    sessions: HashMap<u64, DeviceSession>,
    nodes: HashMap<PathBuf, PortableNode>,
    errors: HashMap<u64, String>,
}

fn service() -> &'static PortableDeviceService {
    static SERVICE: OnceLock<PortableDeviceService> = OnceLock::new();
    SERVICE.get_or_init(PortableDeviceService::default)
}

#[cfg(target_os = "windows")]
pub(super) fn virtual_root() -> PathBuf {
    PathBuf::from(r"\\explorer.portable\devices")
}

#[cfg(not(target_os = "windows"))]
pub(super) fn virtual_root() -> PathBuf {
    PathBuf::from("/__explorer_portable__/devices")
}

fn device_root(location_id: u64) -> PathBuf {
    virtual_root().join(format!("device-{location_id:016x}"))
}

fn stable_device_id(info: &MtpDeviceInfo) -> u64 {
    if let Some(serial) = info
        .serial_number
        .as_deref()
        .filter(|serial| !serial.trim().is_empty())
    {
        return xxh3_64(
            format!("{:04x}:{:04x}:{serial}", info.vendor_id, info.product_id).as_bytes(),
        );
    }
    // Devices without a serial cannot be followed across USB ports. Their
    // topology location is nevertheless stable across reconnects to the same port.
    xxh3_64(
        format!(
            "{:04x}:{:04x}:location:{:016x}",
            info.vendor_id, info.product_id, info.location_id
        )
        .as_bytes(),
    )
}

fn object_path(parent: &Path, handle: ObjectHandle, name: &str) -> PathBuf {
    let safe_name = name
        .chars()
        .map(|character| {
            if matches!(character, '/' | '\\' | '\0') {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    parent.join(format!("object-{:016x}-{safe_name}", handle.0))
}

fn device_label(info: &MtpDeviceInfo) -> String {
    info.product
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or(info.manufacturer.as_deref())
        .unwrap_or("Portable device")
        .trim()
        .to_owned()
}

pub(super) fn is_portable_path(path: &Path) -> bool {
    path.starts_with(virtual_root())
}

pub(super) fn portable_device_roots() -> Vec<PortableDeviceRoot> {
    let devices = match MtpDevice::list_devices() {
        Ok(devices) => devices,
        Err(error) => {
            eprintln!("portable device discovery failed: {error}");
            Vec::new()
        }
    };
    let mut state = service()
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let present = devices.iter().map(stable_device_id).collect::<HashSet<_>>();
    state.discovered.retain(|id, _| present.contains(id));
    state.sessions.retain(|id, _| present.contains(id));
    state.errors.retain(|id, _| present.contains(id));
    state
        .nodes
        .retain(|_, node| present.contains(&node.device_location_id));

    for device in devices {
        let device_id = stable_device_id(&device);
        let label = device_label(&device);
        let path = device_root(device_id);
        state.nodes.insert(
            path,
            PortableNode {
                device_location_id: device_id,
                generation: 0,
                storage_id: None,
                object_handle: None,
                labels: vec!["This PC".to_owned(), label],
                is_directory: true,
                size: None,
                modified: None,
                capabilities: LocationCapabilities::default(),
            },
        );
        state.discovered.insert(device_id, device);
    }

    let mut roots = state
        .discovered
        .iter()
        .map(|(device_id, device)| PortableDeviceRoot {
            label: device_label(device),
            path: device_root(*device_id),
            unavailable_reason: state.errors.get(device_id).cloned(),
        })
        .collect::<Vec<_>>();
    roots.sort_by(|left, right| {
        left.label
            .to_ascii_lowercase()
            .cmp(&right.label.to_ascii_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    roots
}

fn node(path: &Path) -> Option<PortableNode> {
    service()
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .nodes
        .get(path)
        .cloned()
}

fn device_open_target(serial_number: Option<&str>, location_id: u64) -> DeviceOpenTarget {
    #[cfg(target_os = "windows")]
    {
        let _ = serial_number;
        DeviceOpenTarget::Location(location_id)
    }

    #[cfg(not(target_os = "windows"))]
    {
        serial_number
            .filter(|serial| !serial.trim().is_empty())
            .map(str::to_owned)
            .map(DeviceOpenTarget::Serial)
            .unwrap_or(DeviceOpenTarget::Location(location_id))
    }
}

fn device_session(device_id: u64) -> Result<DeviceSession, String> {
    let device_info;
    {
        let state = service()
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(session) = state.sessions.get(&device_id) {
            return Ok(session.clone());
        }
        device_info = state
            .discovered
            .get(&device_id)
            .cloned()
            .ok_or_else(|| "This portable device is no longer connected.".to_owned())?;
    }

    let opened = match device_open_target(
        device_info.serial_number.as_deref(),
        device_info.location_id,
    ) {
        #[cfg(not(target_os = "windows"))]
        DeviceOpenTarget::Serial(serial) => block_on(MtpDevice::open_by_serial(&serial)),
        DeviceOpenTarget::Location(location_id) => {
            block_on(MtpDevice::open_by_location(location_id))
        }
    };
    let mut state = service()
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match opened {
        Ok(device) => {
            state.generation = state.generation.wrapping_add(1).max(1);
            let session = DeviceSession {
                generation: state.generation,
                device,
            };
            state.errors.remove(&device_id);
            state.sessions.insert(device_id, session.clone());
            Ok(session)
        }
        Err(error) => {
            let message = guidance_for_error(&error);
            state.errors.insert(device_id, message.clone());
            Err(message)
        }
    }
}

fn validate_node_session(node: &PortableNode, session: &DeviceSession) -> Result<(), String> {
    if node.generation != 0 && node.generation != session.generation {
        return Err(
            "This portable-device location is stale. Reopen the device from Drives.".into(),
        );
    }
    Ok(())
}

pub(super) fn list_dir(path: &Path) -> io::Result<Vec<FileEntry>> {
    let current = node(path)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "portable location not found"))?;
    let session = device_session(current.device_location_id).map_err(io::Error::other)?;
    validate_node_session(&current, &session).map_err(io::Error::other)?;

    if current.storage_id.is_none() {
        let storages = block_on(session.device.storages())
            .map_err(|error| io::Error::other(guidance_for_error(&error)))?;
        let device_caps = session.device.capabilities();
        let mut entries = Vec::with_capacity(storages.len());
        let mut state = service()
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for storage in storages {
            let info = storage.info();
            let name = if info.description.trim().is_empty() {
                "Storage".to_owned()
            } else {
                info.description.trim().to_owned()
            };
            let child_path = path.join(format!(
                "storage-g{:016x}-{:016x}",
                session.generation, info.id.0
            ));
            let mut labels = current.labels.clone();
            labels.push(name.clone());
            state.nodes.insert(
                child_path.clone(),
                PortableNode {
                    device_location_id: current.device_location_id,
                    generation: session.generation,
                    storage_id: Some(info.id),
                    object_handle: None,
                    labels,
                    is_directory: true,
                    size: None,
                    modified: None,
                    capabilities: LocationCapabilities::from_mtp(*device_caps, info.is_writable),
                },
            );
            entries.push(FileEntry::from_provider(child_path, name, true, None, None));
        }
        return Ok(entries);
    }

    let storage_id = current.storage_id.expect("checked above");
    let storage = block_on(session.device.storage(storage_id))
        .map_err(|error| io::Error::other(guidance_for_error(&error)))?;
    let objects = block_on(storage.list_objects(current.object_handle))
        .map_err(|error| io::Error::other(guidance_for_error(&error)))?;
    let device_caps = session.device.capabilities();
    let writable = storage.info().is_writable;
    let mut entries = Vec::with_capacity(objects.len());
    let mut state = service()
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for object in objects {
        let child_path = object_path(path, object.handle, &object.filename);
        let mut labels = current.labels.clone();
        labels.push(object.filename.clone());
        let is_directory = object.is_folder();
        let modified = object.modified.and_then(mtp_datetime_to_system_time);
        state.nodes.insert(
            child_path.clone(),
            PortableNode {
                device_location_id: current.device_location_id,
                generation: session.generation,
                storage_id: Some(storage_id),
                object_handle: Some(object.handle),
                labels,
                is_directory,
                size: (!is_directory).then_some(object.size),
                modified,
                capabilities: LocationCapabilities::from_mtp(*device_caps, writable),
            },
        );
        entries.push(FileEntry::from_provider(
            child_path,
            object.filename,
            is_directory,
            Some(object.size),
            modified,
        ));
    }
    Ok(entries)
}

fn mtp_datetime_to_system_time(value: mtp_rs::mtp::DateTime) -> Option<SystemTime> {
    Utc.with_ymd_and_hms(
        value.year.into(),
        value.month.into(),
        value.day.into(),
        value.hour.into(),
        value.minute.into(),
        value.second.into(),
    )
    .single()
    .map(Into::into)
}

pub(super) fn exists(path: &Path) -> bool {
    node(path).is_some()
}

pub(super) fn is_dir(path: &Path) -> bool {
    node(path).is_some_and(|node| node.is_directory)
}

pub(super) fn capabilities(path: &Path) -> LocationCapabilities {
    node(path).map(|node| node.capabilities).unwrap_or_default()
}

pub(super) fn metadata(path: &Path) -> Option<PortableMetadata> {
    node(path).map(|node| PortableMetadata {
        name: node.labels.last().cloned().unwrap_or_default(),
        is_directory: node.is_directory,
        size: node.size,
        modified: node.modified,
    })
}

pub(super) fn labels(path: &Path) -> Option<Vec<String>> {
    node(path).map(|node| node.labels)
}

pub(super) fn display_address(path: &Path) -> Option<String> {
    labels(path).map(|labels| labels.join(r"\"))
}

pub(super) fn parent(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    (parent != virtual_root() && is_portable_path(parent)).then(|| parent.to_path_buf())
}

#[allow(dead_code)]
pub(super) fn device_root_for_path(path: &Path) -> Option<PathBuf> {
    let first = path
        .strip_prefix(virtual_root())
        .ok()?
        .components()
        .next()?;
    Some(virtual_root().join(first.as_os_str()))
}

pub(super) fn breadcrumb_segments(path: &Path) -> Option<Vec<(String, PathBuf)>> {
    let labels = labels(path)?;
    let relative = path.strip_prefix(virtual_root()).ok()?;
    let components = relative.components().collect::<Vec<_>>();
    let device = components.first()?;
    let mut target = virtual_root();
    target.push(device.as_os_str());

    let mut segments = vec![(labels.first()?.clone(), target.clone())];
    for (index, label) in labels.into_iter().enumerate().skip(1) {
        if index > 1 {
            let component = components.get(index - 1)?;
            target.push(component.as_os_str());
        }
        segments.push((label, target.clone()));
    }
    Some(segments)
}

pub(super) fn path_for_display_address(address: &str) -> Option<PathBuf> {
    let normalized = address.replace('/', r"\");
    let cached = service()
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .nodes
        .iter()
        .find_map(|(path, node)| {
            node.labels
                .join(r"\")
                .eq_ignore_ascii_case(&normalized)
                .then(|| path.clone())
        });
    if cached.is_some() {
        return cached;
    }

    let components = normalized
        .split('\\')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components.len() < 2 || !components[0].eq_ignore_ascii_case("This PC") {
        return None;
    }
    let roots = portable_device_roots();
    let mut current = roots
        .into_iter()
        .find(|root| root.label.eq_ignore_ascii_case(components[1]))?
        .path;
    for component in components.into_iter().skip(2) {
        current = list_dir(&current)
            .ok()?
            .into_iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(component))?
            .path;
    }
    Some(current)
}

pub(super) fn create_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let current = node(parent).ok_or_else(|| "Portable location not found.".to_owned())?;
    let storage_id = current
        .storage_id
        .ok_or_else(|| "Choose a storage location before creating a folder.".to_owned())?;
    if !current.capabilities.can_create_folder {
        return Err("This device does not support creating folders here.".to_owned());
    }
    let session = device_session(current.device_location_id)?;
    validate_node_session(&current, &session)?;
    let storage =
        block_on(session.device.storage(storage_id)).map_err(|error| guidance_for_error(&error))?;
    let handle = block_on(storage.create_folder(current.object_handle, name))
        .map_err(|error| guidance_for_error(&error))?;
    let path = object_path(parent, handle, name);
    let mut labels = current.labels;
    labels.push(name.to_owned());
    service()
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .nodes
        .insert(
            path.clone(),
            PortableNode {
                device_location_id: current.device_location_id,
                generation: session.generation,
                storage_id: Some(storage_id),
                object_handle: Some(handle),
                labels,
                is_directory: true,
                size: None,
                modified: None,
                capabilities: current.capabilities,
            },
        );
    Ok(path)
}

pub(super) fn write_file(parent: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let payload = Bytes::copy_from_slice(bytes);
    let input = stream::once(async move { Ok::<Bytes, io::Error>(payload) });
    upload_stream(parent, name, bytes.len() as u64, Box::pin(input))
}

fn upload_local_file(parent: &Path, name: &str, source: &Path) -> Result<PathBuf, String> {
    let size = std::fs::metadata(source)
        .map_err(|error| format!("Could not read {}: {error}", source.display()))?
        .len();
    let file = std::fs::File::open(source)
        .map_err(|error| format!("Could not open {}: {error}", source.display()))?;
    let input = stream::unfold(file, |mut file| async move {
        let mut buffer = vec![0; 256 * 1024];
        match file.read(&mut buffer) {
            Ok(0) => None,
            Ok(read) => {
                buffer.truncate(read);
                Some((Ok(Bytes::from(buffer)), file))
            }
            Err(error) => Some((Err(error), file)),
        }
    });
    upload_stream(parent, name, size, Box::pin(input))
}

fn upload_stream(
    parent: &Path,
    name: &str,
    size: u64,
    input: Pin<Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send>>,
) -> Result<PathBuf, String> {
    let current = node(parent).ok_or_else(|| "Portable location not found.".to_owned())?;
    let storage_id = current
        .storage_id
        .ok_or_else(|| "Choose a storage location before creating a file.".to_owned())?;
    if !current.capabilities.can_upload {
        return Err("This device does not support writing files here.".to_owned());
    }
    let session = device_session(current.device_location_id)?;
    validate_node_session(&current, &session)?;
    let storage =
        block_on(session.device.storage(storage_id)).map_err(|error| guidance_for_error(&error))?;
    let handle = match block_on(storage.upload(
        current.object_handle,
        NewObjectInfo::file(name, size),
        input,
    )) {
        Ok(handle) => handle,
        Err(error) => {
            let message = guidance_for_error(&error.source);
            if let Some(partial) = error.partial {
                if current.capabilities.can_delete {
                    if let Err(cleanup) = block_on(storage.delete(partial)) {
                        return Err(format!(
                            "{message} A partial item remains on the device because cleanup failed: {cleanup}"
                        ));
                    }
                } else {
                    return Err(format!(
                        "{message} A partial item remains on the device because deletion is not supported."
                    ));
                }
            }
            return Err(message);
        }
    };
    let path = object_path(parent, handle, name);
    let mut labels = current.labels.clone();
    labels.push(name.to_owned());
    service()
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .nodes
        .insert(
            path.clone(),
            PortableNode {
                device_location_id: current.device_location_id,
                generation: session.generation,
                storage_id: Some(storage_id),
                object_handle: Some(handle),
                labels,
                is_directory: false,
                size: Some(size),
                modified: Some(SystemTime::now()),
                capabilities: current.capabilities,
            },
        );
    Ok(path)
}

pub(super) fn rename(path: &Path, new_name: &str) -> Result<(), String> {
    let current = node(path).ok_or_else(|| "Portable location not found.".to_owned())?;
    if !current.capabilities.can_rename {
        return Err("This device does not support renaming this item.".to_owned());
    }
    let storage_id = current
        .storage_id
        .ok_or_else(|| "Storage not found.".to_owned())?;
    let handle = current
        .object_handle
        .ok_or_else(|| "This item cannot be renamed.".to_owned())?;
    let session = device_session(current.device_location_id)?;
    validate_node_session(&current, &session)?;
    let storage =
        block_on(session.device.storage(storage_id)).map_err(|error| guidance_for_error(&error))?;
    block_on(storage.rename(handle, new_name)).map_err(|error| guidance_for_error(&error))
}

pub(super) fn delete(path: &Path) -> Result<(), String> {
    let current = node(path).ok_or_else(|| "Portable location not found.".to_owned())?;
    if !current.capabilities.can_delete {
        return Err("This device does not support deleting this item.".to_owned());
    }
    let storage_id = current
        .storage_id
        .ok_or_else(|| "Storage not found.".to_owned())?;
    let handle = current
        .object_handle
        .ok_or_else(|| "This item cannot be deleted.".to_owned())?;
    let session = device_session(current.device_location_id)?;
    validate_node_session(&current, &session)?;
    let storage =
        block_on(session.device.storage(storage_id)).map_err(|error| guidance_for_error(&error))?;
    block_on(storage.delete(handle)).map_err(|error| guidance_for_error(&error))?;
    service()
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .nodes
        .remove(path);
    Ok(())
}

#[cfg(test)]
pub(super) fn download(path: &Path) -> Result<Vec<u8>, String> {
    let current = node(path).ok_or_else(|| "Portable location not found.".to_owned())?;
    let storage_id = current
        .storage_id
        .ok_or_else(|| "Storage not found.".to_owned())?;
    let handle = current
        .object_handle
        .ok_or_else(|| "Choose a file to open.".to_owned())?;
    let session = device_session(current.device_location_id)?;
    validate_node_session(&current, &session)?;
    let storage =
        block_on(session.device.storage(storage_id)).map_err(|error| guidance_for_error(&error))?;
    block_on(storage.download_to_vec(handle)).map_err(|error| guidance_for_error(&error))
}

fn download_to_path(path: &Path, target: &Path) -> Result<(), String> {
    let current = node(path).ok_or_else(|| "Portable location not found.".to_owned())?;
    let storage_id = current
        .storage_id
        .ok_or_else(|| "Storage not found.".to_owned())?;
    let handle = current
        .object_handle
        .ok_or_else(|| "Choose a file to copy.".to_owned())?;
    let session = device_session(current.device_location_id)?;
    validate_node_session(&current, &session)?;
    let storage =
        block_on(session.device.storage(storage_id)).map_err(|error| guidance_for_error(&error))?;
    let mut output = std::fs::File::create(target)
        .map_err(|error| format!("Could not create {}: {error}", target.display()))?;
    let result = block_on(async {
        let mut download = storage
            .download(handle, ByteRange::Full)
            .await
            .map_err(|error| guidance_for_error(&error))?;
        let expected = download.size();
        while let Some(chunk) = download.next_chunk().await {
            let chunk = chunk.map_err(|error| guidance_for_error(&error))?;
            output
                .write_all(&chunk)
                .map_err(|error| format!("Could not write {}: {error}", target.display()))?;
        }
        output
            .flush()
            .map_err(|error| format!("Could not write {}: {error}", target.display()))?;
        if output
            .metadata()
            .map_err(|error| format!("Could not verify {}: {error}", target.display()))?
            .len()
            != expected
        {
            return Err("Downloaded file size did not match device metadata.".to_owned());
        }
        Ok::<(), String>(())
    });
    if let Err(error) = result {
        let _ = std::fs::remove_file(target);
        return Err(error);
    }
    Ok(())
}

/// Performs a transfer when either endpoint is a portable location. A `None`
/// result means the existing native-filesystem pipeline should handle it.
pub(super) fn transfer_paths(
    sources: &[PathBuf],
    destination: &Path,
    move_sources: bool,
) -> Option<Result<Vec<PathBuf>, String>> {
    let uses_portable =
        is_portable_path(destination) || sources.iter().any(|source| is_portable_path(source));
    if !uses_portable {
        return None;
    }
    Some((|| {
        if sources.is_empty() {
            return Err("No items were selected.".to_owned());
        }
        if !is_dir(destination) && is_portable_path(destination) {
            return Err("The portable-device destination is not a folder.".to_owned());
        }
        let mut completed = Vec::new();
        for source in sources {
            let copied = copy_item(source, destination, move_sources)?;
            completed.push(copied);
        }
        Ok(completed)
    })())
}

fn copy_item(source: &Path, destination: &Path, move_source: bool) -> Result<PathBuf, String> {
    let name = item_name(source)?;
    if destination_child(destination, &name)?.is_some() {
        return Err(format!(
            "An item named {name} already exists in the destination."
        ));
    }

    if let Some(result) = native_portable_transfer(source, destination, move_source) {
        return result;
    }

    let source_is_dir = if is_portable_path(source) {
        is_dir(source)
    } else {
        source.is_dir()
    };
    let copied = if source_is_dir {
        let folder = create_destination_folder(destination, &name)?;
        let children = if is_portable_path(source) {
            list_dir(source).map_err(|error| error.to_string())?
        } else {
            std::fs::read_dir(source)
                .map_err(|error| format!("Could not read {}: {error}", source.display()))?
                .filter_map(Result::ok)
                .filter_map(|entry| FileEntry::from_path(entry.path()))
                .collect()
        };
        for child in children {
            copy_item(&child.path, &folder, false)?;
        }
        folder
    } else if is_portable_path(destination) {
        if is_portable_path(source) {
            let temporary = transfer_temporary_path(&name);
            let result = download_to_path(source, &temporary)
                .and_then(|_| upload_local_file(destination, &name, &temporary));
            let _ = std::fs::remove_file(&temporary);
            result?
        } else {
            upload_local_file(destination, &name, source)?
        }
    } else {
        let target = destination.join(&name);
        if is_portable_path(source) {
            download_to_path(source, &target)?;
        } else {
            std::fs::copy(source, &target)
                .map_err(|error| format!("Could not copy {}: {error}", source.display()))?;
        }
        target
    };

    if move_source {
        delete_transfer_source(source)?;
    }
    Ok(copied)
}

fn item_name(path: &Path) -> Result<String, String> {
    if is_portable_path(path) {
        return labels(path)
            .and_then(|labels| labels.last().cloned())
            .ok_or_else(|| "Portable item name was not found.".to_owned());
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("Could not determine the name of {}.", path.display()))
}

fn destination_child(destination: &Path, name: &str) -> Result<Option<PathBuf>, String> {
    if is_portable_path(destination) {
        return list_dir(destination)
            .map_err(|error| error.to_string())
            .map(|entries| {
                entries
                    .into_iter()
                    .find(|entry| entry.name.eq_ignore_ascii_case(name))
                    .map(|entry| entry.path)
            });
    }
    let child = destination.join(name);
    Ok(child.exists().then_some(child))
}

fn create_destination_folder(destination: &Path, name: &str) -> Result<PathBuf, String> {
    if is_portable_path(destination) {
        create_folder(destination, name)
    } else {
        let path = destination.join(name);
        std::fs::create_dir(&path)
            .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
        Ok(path)
    }
}

fn delete_transfer_source(source: &Path) -> Result<(), String> {
    if is_portable_path(source) {
        delete(source)
    } else if source.is_dir() {
        std::fs::remove_dir_all(source)
            .map_err(|error| format!("Could not delete {}: {error}", source.display()))
    } else {
        std::fs::remove_file(source)
            .map_err(|error| format!("Could not delete {}: {error}", source.display()))
    }
}

fn native_portable_transfer(
    source: &Path,
    destination: &Path,
    move_source: bool,
) -> Option<Result<PathBuf, String>> {
    let source_node = node(source)?;
    let destination_node = node(destination)?;
    if source_node.device_location_id != destination_node.device_location_id {
        return None;
    }
    let supported = if move_source {
        source_node.capabilities.can_move
    } else {
        source_node.capabilities.can_copy
    };
    if !supported {
        return None;
    }
    Some((|| {
        let source_storage_id = source_node
            .storage_id
            .ok_or_else(|| "Source storage was not found.".to_owned())?;
        let destination_storage_id = destination_node
            .storage_id
            .ok_or_else(|| "Destination storage was not found.".to_owned())?;
        let source_handle = source_node
            .object_handle
            .ok_or_else(|| "Source object was not found.".to_owned())?;
        let destination_parent = destination_node.object_handle.unwrap_or(ObjectHandle::ROOT);
        let session = device_session(source_node.device_location_id)?;
        validate_node_session(&source_node, &session)?;
        validate_node_session(&destination_node, &session)?;
        let storage = block_on(session.device.storage(source_storage_id))
            .map_err(|error| guidance_for_error(&error))?;
        let handle = if move_source {
            block_on(storage.move_object(
                source_handle,
                destination_parent,
                Some(destination_storage_id),
            ))
            .map_err(|error| guidance_for_error(&error))?;
            source_handle
        } else {
            block_on(storage.copy_object(
                source_handle,
                destination_parent,
                Some(destination_storage_id),
            ))
            .map_err(|error| guidance_for_error(&error))?
        };
        let name = item_name(source)?;
        let target = object_path(destination, handle, &name);
        let mut copied = source_node.clone();
        copied.generation = session.generation;
        copied.storage_id = Some(destination_storage_id);
        copied.object_handle = Some(handle);
        copied.labels = destination_node.labels.clone();
        copied.labels.push(name);
        let mut state = service()
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if move_source {
            state.nodes.remove(source);
        }
        state.nodes.insert(target.clone(), copied);
        Ok(target)
    })())
}

fn transfer_temporary_path(name: &str) -> PathBuf {
    static TRANSFER_ID: AtomicU64 = AtomicU64::new(1);
    let safe_name = Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("portable-transfer");
    std::env::temp_dir().join(format!(
        "explorer-portable-transfer-{:016x}-{safe_name}",
        TRANSFER_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

pub(super) fn materialize_for_open(path: &Path) -> io::Result<PathBuf> {
    static VERSION: AtomicU64 = AtomicU64::new(1);
    let name = labels(path)
        .and_then(|labels| labels.last().cloned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "portable-file".to_owned());
    let safe_name = Path::new(&name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("portable-file");
    let cache = crate::settings::config_dir()
        .map(|directory| directory.join("cache").join(PORTABLE_OPEN_CACHE_DIRECTORY))
        .unwrap_or_else(|| std::env::temp_dir().join("explorer-portable-open"));
    std::fs::create_dir_all(&cache)?;
    let target = cache.join(format!(
        "{:016x}-{safe_name}",
        VERSION.fetch_add(1, Ordering::Relaxed)
    ));
    download_to_path(path, &target).map_err(io::Error::other)?;
    Ok(target)
}

pub(super) fn cleanup_open_cache() {
    let Some(cache) = crate::settings::config_dir()
        .map(|directory| directory.join("cache").join(PORTABLE_OPEN_CACHE_DIRECTORY))
    else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(super) fn thumbnail(path: &Path) -> Result<Vec<u8>, String> {
    let current = node(path).ok_or_else(|| "Portable location not found.".to_owned())?;
    if !current.capabilities.supports_thumbnails {
        return Err("This device does not provide thumbnails.".to_owned());
    }
    let storage_id = current
        .storage_id
        .ok_or_else(|| "Storage not found.".to_owned())?;
    let handle = current
        .object_handle
        .ok_or_else(|| "Item not found.".to_owned())?;
    let session = device_session(current.device_location_id)?;
    validate_node_session(&current, &session)?;
    let storage =
        block_on(session.device.storage(storage_id)).map_err(|error| guidance_for_error(&error))?;
    block_on(storage.thumbnail(handle)).map_err(|error| guidance_for_error(&error))
}

pub(super) fn wait_for_object_event(path: &Path) -> Result<bool, String> {
    let current = node(path).ok_or_else(|| "Portable location not found.".to_owned())?;
    if !current.capabilities.supports_events {
        return Err("This device does not provide object events.".to_owned());
    }
    let session = device_session(current.device_location_id)?;
    validate_node_session(&current, &session)?;
    match block_on(session.device.next_event()) {
        Ok(_) => Ok(true),
        Err(mtp_rs::Error::Timeout) => Ok(false),
        Err(error) => Err(guidance_for_error(&error)),
    }
}

pub(super) fn guidance_for_error(error: &mtp_rs::Error) -> String {
    match error {
        mtp_rs::Error::PermissionDenied | mtp_rs::Error::AccessDenied => {
            if cfg!(target_os = "linux") {
                "Explorer cannot access this device. Unlock it, select File Transfer, and ensure your Linux udev rules permit USB access.".to_owned()
            } else {
                "Explorer cannot access this device. Unlock it and select File Transfer or Android Auto for the USB connection.".to_owned()
            }
        }
        mtp_rs::Error::ExclusiveAccess => {
            if cfg!(target_os = "macos") {
                "Another process is using this device. Close Photos, Image Capture, Android File Transfer, or another device manager, then refresh.".to_owned()
            } else {
                "Another application is using this portable device. Close it there, then refresh Explorer.".to_owned()
            }
        }
        mtp_rs::Error::Busy | mtp_rs::Error::Timeout => {
            "The device is busy. Unlock it, confirm File Transfer mode, and try Refresh.".to_owned()
        }
        mtp_rs::Error::Disconnected | mtp_rs::Error::NoDevice => {
            "This portable device is no longer connected.".to_owned()
        }
        mtp_rs::Error::StorageFull => {
            "The portable device does not have enough free space.".to_owned()
        }
        mtp_rs::Error::Unsupported => {
            "This operation is not supported by the portable device.".to_owned()
        }
        _ => format!("Could not access the portable device: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtp_rs::transport::virtual_device::config::{VirtualDeviceConfig, VirtualStorageConfig};
    use mtp_rs::{register_virtual_device, unregister_virtual_device};
    use std::sync::MutexGuard;
    use std::time::Duration;

    fn virtual_test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn reset_service() {
        *service()
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = PortableDeviceState::default();
    }

    fn virtual_config(
        serial: &str,
        first: &Path,
        second: &Path,
        second_read_only: bool,
    ) -> VirtualDeviceConfig {
        VirtualDeviceConfig {
            manufacturer: "Explorer Tests".into(),
            model: "Virtual Android".into(),
            serial: serial.into(),
            storages: vec![
                VirtualStorageConfig {
                    description: "Internal Storage".into(),
                    capacity: 16 * 1024 * 1024,
                    backing_dir: first.to_path_buf(),
                    read_only: false,
                },
                VirtualStorageConfig {
                    description: "SD Card".into(),
                    capacity: 16 * 1024 * 1024,
                    backing_dir: second.to_path_buf(),
                    read_only: second_read_only,
                },
            ],
            event_poll_interval: Duration::ZERO,
            watch_backing_dirs: false,
            ..Default::default()
        }
    }

    #[test]
    fn portable_virtual_paths_are_classified_without_touching_the_host_filesystem() {
        let path = device_root(0x1234);
        assert!(is_portable_path(&path));
        assert!(!is_portable_path(Path::new("/tmp/local")));
    }

    #[test]
    fn portable_guidance_distinguishes_permissions_and_exclusive_access() {
        let permission = guidance_for_error(&mtp_rs::Error::PermissionDenied);
        let exclusive = guidance_for_error(&mtp_rs::Error::ExclusiveAccess);
        assert_ne!(permission, exclusive);
        assert!(permission.to_ascii_lowercase().contains("access"));
        assert!(exclusive.to_ascii_lowercase().contains("another"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_device_open_target_uses_location_even_when_serial_is_available() {
        assert_eq!(
            device_open_target(Some("usb-serial"), 0x1234),
            DeviceOpenTarget::Location(0x1234)
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_device_open_target_prefers_available_serial() {
        assert_eq!(
            device_open_target(Some("usb-serial"), 0x1234),
            DeviceOpenTarget::Serial("usb-serial".to_owned())
        );
    }

    #[test]
    fn device_open_target_uses_location_without_a_serial() {
        assert_eq!(
            device_open_target(None, 0x1234),
            DeviceOpenTarget::Location(0x1234)
        );
    }

    #[test]
    fn virtual_device_supports_discovery_navigation_metadata_and_mutations() {
        let _guard = virtual_test_lock();
        reset_service();
        let first = tempfile::tempdir().expect("first storage");
        let second = tempfile::tempdir().expect("second storage");
        std::fs::write(first.path().join("hello.txt"), b"hello").expect("seed file");
        std::fs::create_dir(first.path().join("Pictures")).expect("seed folder");
        let config = virtual_config(
            "explorer-provider-round-trip",
            first.path(),
            second.path(),
            false,
        );
        let info = register_virtual_device(&config);

        let roots = portable_device_roots();
        let device_id = stable_device_id(&info);
        let root = roots
            .iter()
            .find(|root| root.path == device_root(device_id))
            .expect("registered device root")
            .path
            .clone();
        let storages = list_dir(&root).expect("list storages");
        assert_eq!(storages.len(), 2);
        assert_eq!(storages[0].name, "Internal Storage");
        assert_eq!(storages[1].name, "SD Card");

        let internal = storages[0].path.clone();
        let sd_card = storages[1].path.clone();
        let objects = list_dir(&internal).expect("list objects");
        let hello = objects
            .iter()
            .find(|entry| entry.name == "hello.txt")
            .expect("hello object")
            .path
            .clone();
        assert_eq!(download(&hello).expect("download"), b"hello");
        assert_eq!(
            display_address(&hello).as_deref(),
            Some(r"This PC\Virtual Android\Internal Storage\hello.txt")
        );
        assert_eq!(
            path_for_display_address(r"this pc\virtual android\internal storage\HELLO.TXT"),
            Some(hello.clone())
        );
        assert_eq!(breadcrumb_segments(&hello).expect("breadcrumbs").len(), 4);

        rename(&hello, "renamed.txt").expect("rename");
        assert!(first.path().join("renamed.txt").exists());
        let folder = create_folder(&internal, "Created").expect("create folder");
        assert!(is_dir(&folder));
        let uploaded = write_file(&folder, "upload.bin", b"payload").expect("upload");
        assert_eq!(download(&uploaded).expect("download upload"), b"payload");
        assert!(wait_for_object_event(&folder).expect("object event"));

        let failing_upload = stream::iter(vec![
            Ok(Bytes::from_static(b"partial")),
            Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled")),
        ]);
        assert!(upload_stream(&folder, "cancelled.bin", 64, Box::pin(failing_upload),).is_err());
        assert!(
            !first.path().join("Created").join("cancelled.bin").exists(),
            "the surfaced partial handle should be deleted after a failed upload"
        );

        let copied = transfer_paths(std::slice::from_ref(&uploaded), &sd_card, false)
            .expect("portable transfer")
            .expect("copy between storages");
        assert_eq!(copied.len(), 1);
        assert_eq!(
            std::fs::read(second.path().join("upload.bin")).unwrap(),
            b"payload"
        );

        let local_destination = tempfile::tempdir().expect("local destination");
        transfer_paths(
            std::slice::from_ref(&uploaded),
            local_destination.path(),
            false,
        )
        .expect("portable transfer")
        .expect("copy out");
        assert_eq!(
            std::fs::read(local_destination.path().join("upload.bin")).unwrap(),
            b"payload"
        );

        delete(&uploaded).expect("permanent delete");
        assert!(!first.path().join("Created").join("upload.bin").exists());
        unregister_virtual_device(info.location_id);
        portable_device_roots();
        assert!(!exists(&root));
        reset_service();
    }

    #[test]
    fn virtual_read_only_storage_is_visible_and_capability_gated() {
        let _guard = virtual_test_lock();
        reset_service();
        let first = tempfile::tempdir().expect("first storage");
        let second = tempfile::tempdir().expect("read only storage");
        let config = virtual_config(
            "explorer-provider-read-only",
            first.path(),
            second.path(),
            true,
        );
        let info = register_virtual_device(&config);
        portable_device_roots();
        let root = device_root(stable_device_id(&info));
        let storages = list_dir(&root).expect("list storages");
        let read_only = storages
            .iter()
            .find(|entry| entry.name == "SD Card")
            .expect("read only storage");
        assert!(!capabilities(&read_only.path).can_upload);
        assert!(write_file(&read_only.path, "blocked.txt", b"no").is_err());
        unregister_virtual_device(info.location_id);
        portable_device_roots();
        reset_service();
    }

    #[test]
    fn reconnect_changes_generation_and_rejects_stale_locations() {
        let _guard = virtual_test_lock();
        reset_service();
        let first = tempfile::tempdir().expect("first storage");
        let second = tempfile::tempdir().expect("second storage");
        let config = virtual_config(
            "explorer-provider-generation",
            first.path(),
            second.path(),
            false,
        );
        let first_info = register_virtual_device(&config);
        portable_device_roots();
        let root = device_root(stable_device_id(&first_info));
        let stale_storage = list_dir(&root).expect("first storage listing")[0]
            .path
            .clone();
        unregister_virtual_device(first_info.location_id);
        portable_device_roots();

        let second_info = register_virtual_device(&config);
        assert_ne!(first_info.location_id, second_info.location_id);
        assert_eq!(
            stable_device_id(&first_info),
            stable_device_id(&second_info)
        );
        portable_device_roots();
        let fresh_storage = list_dir(&root).expect("second storage listing")[0]
            .path
            .clone();
        assert_ne!(stale_storage, fresh_storage);
        assert!(!exists(&stale_storage));
        assert!(list_dir(&stale_storage).is_err());

        unregister_virtual_device(second_info.location_id);
        portable_device_roots();
        reset_service();
    }
}
