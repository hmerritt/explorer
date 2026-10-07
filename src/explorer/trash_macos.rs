use super::*;
use cocoa::{
    base::{id, nil},
    foundation::NSAutoreleasePool,
};
use objc::{class, msg_send, sel, sel_impl};
use std::io::Write;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct JournalRecord {
    original: PathBuf,
    payload: Option<PathBuf>,
    identity: String,
    deleted: SystemTime,
    #[serde(default)]
    original_volume: Option<(PathBuf, String)>,
}

fn journal_path() -> Result<PathBuf, String> {
    crate::settings::config_dir()
        .map(|p| p.join("trash-origins.json"))
        .ok_or_else(|| "Explorer's application-data directory is unavailable.".into())
}
fn read_journal() -> Result<Vec<JournalRecord>, String> {
    match fs::read(journal_path()?) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("Could not read Trash recovery metadata: {e}")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}
fn write_journal(records: &[JournalRecord]) -> Result<(), String> {
    let path = journal_path()?;
    let parent = path.parent().ok_or("Invalid journal location.")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer(staged.as_file_mut(), records).map_err(|e| e.to_string())?;
    staged.flush().map_err(|e| e.to_string())?;
    staged.as_file().sync_all().map_err(|e| e.to_string())?;
    staged.persist(&path).map_err(|e| e.to_string())?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn trash_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = super::super::filesystem::user_home_dir() {
        roots.push(home.join(".Trash"));
    }
    // SAFETY: getuid has no preconditions and returns the process user's uid.
    let uid = unsafe { libc::getuid() };
    let mut mounts = std::ptr::null_mut();
    // SAFETY: getmntinfo returns a system-owned array valid until the next call.
    let count = unsafe { libc::getmntinfo(&mut mounts, libc::MNT_NOWAIT) };
    if count > 0 && !mounts.is_null() {
        use std::os::unix::ffi::OsStrExt;
        for mount in unsafe { std::slice::from_raw_parts(mounts, count as usize) } {
            let bytes = unsafe { std::ffi::CStr::from_ptr(mount.f_mntonname.as_ptr()) }.to_bytes();
            roots.push(
                PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
                    .join(".Trashes")
                    .join(uid.to_string()),
            );
        }
    }
    let mut seen = BTreeSet::new();
    roots.retain(|root| seen.insert(root.clone()));
    roots
}

pub(super) fn platform_list() -> Result<Vec<TrashEntry>, String> {
    let records = read_journal()?;
    let mut live = BTreeSet::new();
    let mut entries = Vec::new();
    let mut inspected = Vec::new();
    let mut payloads = Vec::new();
    let mut identity_counts = BTreeMap::new();
    for (index, root) in trash_roots().into_iter().enumerate() {
        let children = match fs::read_dir(&root) {
            Ok(children) => children,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) if index == 0 => {
                return Err(format!(
                    "Could not read Trash: {e}. Check macOS privacy permissions."
                ));
            }
            Err(_) => continue,
        };
        inspected.push(root.clone());
        for child in children {
            let Ok(child) = child else {
                continue;
            };
            if child.file_name() == ".DS_Store" {
                continue;
            }
            let payload = child.path();
            let Ok(identity) = fingerprint(&payload) else {
                continue;
            };
            let Ok(metadata) = fs::symlink_metadata(&payload) else {
                continue;
            };
            live.insert(identity.clone());
            *identity_counts.entry(identity.clone()).or_insert(0usize) += 1;
            payloads.push((payload, identity, metadata, child.file_name()));
        }
    }
    for (payload, identity, metadata, native_name) in payloads {
        let record = records
            .iter()
            .rev()
            .find(|r| r.identity == identity && r.payload.as_deref() == Some(payload.as_path()))
            .or_else(|| {
                // Pending crash recovery is trustworthy only for a unique inode.
                // Two trashed hard links cannot inherit one another's origins.
                if identity_counts.get(&identity) != Some(&1) {
                    return None;
                }
                records.iter().rev().find(|r| {
                    r.identity == identity
                        && r.payload.is_none()
                        && fs::symlink_metadata(&r.original).is_err()
                })
            });
        let original_path = record.map(|r| r.original.clone());
        let name = original_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(OsString::from)
            .unwrap_or(native_name);
        let deleted = record.map(|r| r.deleted);
        let id = TrashItemId(format!(
            "{:x}",
            Sha256::digest(format!("{payload:?}:{identity}:{deleted:?}").as_bytes())
        ));
        entries.push(TrashEntry {
            id,
            name,
            original_path,
            deleted,
            size: metadata.is_file().then_some(metadata.len()),
            directory: metadata.is_dir(),
            payload,
            fingerprint: identity,
            original_volume: record.and_then(|r| r.original_volume.clone()),
        });
    }
    // Retain pending records whose original source still exists. They make a
    // crash between Foundation's move and journal commit recoverable.
    let retained: Vec<_> = records
        .iter()
        .filter(|r| {
            live.contains(&r.identity)
                || r.payload.as_ref().is_some_and(|payload| {
                    !inspected
                        .iter()
                        .any(|root| payload.parent() == Some(root.as_path()))
                })
                || (r.payload.is_none()
                    && fingerprint(&r.original).ok().as_ref() == Some(&r.identity))
        })
        .cloned()
        .collect();
    if retained.len() != records.len() {
        write_journal(&retained)?;
    }
    Ok(entries)
}

pub(super) fn platform_trash(path: &Path) -> Result<(), String> {
    let mut records = read_journal()?;
    let identity = fingerprint(path).map_err(|e| e.to_string())?;
    let original_volume = volume_identity(path);
    records.push(JournalRecord {
        original: std::path::absolute(path).map_err(|e| e.to_string())?,
        payload: None,
        identity,
        deleted: SystemTime::now(),
        original_volume,
    });
    write_journal(&records)?;
    // SAFETY: Foundation objects are used in this thread's autorelease pool;
    // output pointers are valid for the duration of the message send.
    let resulting = unsafe {
        let pool = NSAutoreleasePool::new(nil);
        use std::os::unix::ffi::OsStrExt;
        let bytes =
            std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let url: id = msg_send![class!(NSURL), fileURLWithFileSystemRepresentation: bytes.as_ptr() isDirectory: objc::runtime::NO relativeToURL: nil];
        let manager: id = msg_send![class!(NSFileManager), defaultManager];
        let mut resulting: id = nil;
        let mut error: id = nil;
        let success: objc::runtime::BOOL = msg_send![manager, trashItemAtURL: url resultingItemURL: &mut resulting error: &mut error];
        let result = if success == objc::runtime::YES && resulting != nil {
            let name: id = msg_send![resulting, path];
            let chars: *const std::ffi::c_char = msg_send![name, fileSystemRepresentation];
            use std::os::unix::ffi::OsStrExt;
            Ok(PathBuf::from(std::ffi::OsStr::from_bytes(
                std::ffi::CStr::from_ptr(chars).to_bytes(),
            )))
        } else {
            Err("macOS could not move this item to Trash.".to_string())
        };
        let _: () = msg_send![pool, drain];
        result
    };
    match resulting {
        Ok(payload) => {
            if let Ok(identity) = fingerprint(&payload) {
                records.last_mut().unwrap().identity = identity;
            }
            records.last_mut().unwrap().payload = Some(payload);
            // The durable pending record already guarantees origin recovery if
            // this commit fails; reconcile it by filesystem identity on listing.
            let _ = write_journal(&records);
            Ok(())
        }
        Err(error) => {
            records.pop();
            write_journal(&records)?;
            Err(error)
        }
    }
}

pub(super) fn platform_purge(item: &TrashEntry) -> Result<(), String> {
    if fingerprint(&item.payload).ok().as_ref() != Some(&item.fingerprint) {
        return Err("The Trash item changed; refresh and try again.".into());
    }
    remove_payload(&item.payload).map_err(|e| e.to_string())?;
    if let Ok(mut records) = read_journal() {
        records.retain(|r| {
            r.payload.as_deref() != Some(item.payload.as_path())
                && !(r.payload.is_none() && r.identity == item.fingerprint)
        });
        // A removed payload is authoritative; listing reconciles a stale journal.
        let _ = write_journal(&records);
    }
    Ok(())
}

pub(super) fn update_macos_origin(
    recovered: &Path,
    original: &Path,
    volume: Option<(PathBuf, String)>,
) -> Result<(), String> {
    let mut records = read_journal()?;
    if let Some(record) = records
        .iter_mut()
        .rev()
        .find(|r| same_path(&r.original, recovered))
    {
        record.original = original.to_owned();
        record.original_volume = volume;
    }
    write_journal(&records)
}

fn volume_identity(path: &Path) -> Option<(PathBuf, String)> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path = CString::new(path.parent()?.as_os_str().as_bytes()).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: statfs writes the initialized structure on success.
    if unsafe { libc::statfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    let bytes = unsafe { std::ffi::CStr::from_ptr(info.f_mntonname.as_ptr()) }.to_bytes();
    let root = PathBuf::from(std::ffi::OsStr::from_bytes(bytes));
    Some((root.clone(), fingerprint(&root).ok()?))
}
pub(super) fn validate_original_volume(item: &TrashEntry) -> Result<(), String> {
    if let Some((root, expected)) = &item.original_volume {
        if fingerprint(root).ok().as_ref() != Some(expected) {
            return Err("The original volume is unavailable or was replaced. Use Restore to… to choose another folder.".into());
        }
    }
    Ok(())
}
