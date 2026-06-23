use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, SystemTime},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[cfg(not(target_os = "windows"))]
use crate::explorer::filesystem::user_home_dir;
use crate::{
    explorer::{
        entry::{EntryKind, FileEntry},
        filesystem::{EntryVisibility, path_is_same_or_descendant},
        view::ExplorerView,
    },
    settings::{RcloneSettings, config_dir},
};

const RCLONE_VIRTUAL_ROOT: &str = "rclone";
const RCLONE_MANIFEST_FILE_NAME: &str = "rclone-mounts.json";
const RCLONE_OPEN_COPY_DIR_NAME: &str = "explorer-rclone-open";
const RCLONE_TRANSFER_GROUP: &str = "explorer-rclone-transfer";
const RCLONE_JOB_POLL_INTERVAL: Duration = Duration::from_millis(200);
const RCLONE_JOB_POLL_LIMIT: usize = 36_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RcloneRemote {
    pub(super) name: String,
    pub(super) display_name: String,
    pub(super) provider_type: Option<String>,
    pub(super) state: RcloneRemoteState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(super) enum RcloneRemoteState {
    Disconnected,
    Connecting,
    Mounted(Box<MountedRemote>),
    TransferMode(Box<TransferRemote>),
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MountedRemote {
    pub(super) remote: RcloneRemoteIdentity,
    pub(super) mount_root: PathBuf,
    pub(super) display_root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TransferRemote {
    pub(super) remote: RcloneRemoteIdentity,
    pub(super) mount_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RcloneRemoteIdentity {
    pub(super) name: String,
    pub(super) display_name: String,
    pub(super) provider_type: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RcloneSidebarState {
    Disconnected,
    Connecting,
    Mounted,
    TransferMode,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum RcloneConnection {
    Mounted(MountedRemote),
    TransferMode(TransferRemote),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RclonePath {
    pub(super) remote_name: String,
    pub(super) relative_path: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct RcloneMountManifest {
    mounts: BTreeMap<String, PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RcloneTransferOperation {
    Copy,
    Move,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExistingMountPolicy {
    TrustAccessibleRoot,
    RequireKnownMount,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RcloneFileEndpoint {
    fs: String,
    remote: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TransferPathMetadata {
    is_dir: bool,
}

#[derive(Debug)]
pub(super) struct RcloneConnectPermit {
    remote_name: String,
}

pub(super) trait RcloneClient {
    fn rpc(&self, method: &str, input: Value) -> Result<Value, String>;
}

pub(super) struct LibrcloneClient;

impl RcloneClient for LibrcloneClient {
    fn rpc(&self, method: &str, input: Value) -> Result<Value, String> {
        initialize_librclone()?;
        let input = serde_json::to_string(&input).map_err(|error| error.to_string())?;
        let output = librclone::try_rpc(method, input).map_err(|error| error.to_string())?;
        serde_json::from_str::<Value>(&output).map_err(|error| error.to_string())
    }
}

pub(super) fn disabled_error() -> String {
    "rclone is disabled in Explorer settings.".to_owned()
}

fn ensure_enabled(settings: &RcloneSettings) -> Result<(), String> {
    settings.enabled.then_some(()).ok_or_else(disabled_error)
}

fn prepare_librclone(settings: &RcloneSettings) -> Result<(), String> {
    ensure_enabled(settings)?;
    apply_librclone_config(settings)
}

fn apply_librclone_config(settings: &RcloneSettings) -> Result<(), String> {
    let default_config_path = default_librclone_config_path()?;
    let path = match settings.resolved_conf_path() {
        Some(path) => path.to_string_lossy().into_owned(),
        None => default_config_path,
    };
    LibrcloneClient
        .rpc("config/setpath", json!({ "path": path }))
        .map(drop)
}

fn default_librclone_config_path() -> Result<String, String> {
    static DEFAULT_CONFIG_PATH: OnceLock<Result<String, String>> = OnceLock::new();
    DEFAULT_CONFIG_PATH
        .get_or_init(|| {
            let response = LibrcloneClient.rpc("config/paths", json!({}))?;
            response
                .get("config")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "rclone config/paths response did not contain config".to_owned())
        })
        .clone()
}

#[cfg(test)]
fn apply_rclone_config_with_client(
    client: &impl RcloneClient,
    settings: &RcloneSettings,
) -> Result<(), String> {
    ensure_enabled(settings)?;
    if let Some(path) = settings.resolved_conf_path() {
        client
            .rpc("config/setpath", json!({ "path": path.to_string_lossy() }))
            .map(drop)?;
    }
    Ok(())
}

fn initialize_librclone() -> Result<(), String> {
    static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();
    INITIALIZED
        .get_or_init(|| librclone::try_initialize().map_err(|error| error.to_string()))
        .clone()
}

fn connecting_remotes() -> &'static Mutex<BTreeSet<String>> {
    static CONNECTING_REMOTES: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    CONNECTING_REMOTES.get_or_init(|| Mutex::new(BTreeSet::new()))
}

pub(super) fn try_begin_remote_connection(remote_name: &str) -> Option<RcloneConnectPermit> {
    let remote_name = normalized_remote_name(remote_name);
    let mut connecting = connecting_remotes()
        .lock()
        .expect("rclone connecting remotes mutex poisoned");
    connecting.insert(remote_name.clone()).then_some(RcloneConnectPermit {
        remote_name,
    })
}

pub(super) fn remote_is_connecting(remote_name: &str) -> bool {
    let remote_name = normalized_remote_name(remote_name);
    connecting_remotes()
        .lock()
        .expect("rclone connecting remotes mutex poisoned")
        .contains(&remote_name)
}

impl Drop for RcloneConnectPermit {
    fn drop(&mut self) {
        let mut connecting = connecting_remotes()
            .lock()
            .expect("rclone connecting remotes mutex poisoned");
        connecting.remove(&self.remote_name);
    }
}

#[cfg(test)]
pub(super) fn reset_connecting_remotes_for_test() {
    connecting_remotes()
        .lock()
        .expect("rclone connecting remotes mutex poisoned")
        .clear();
}

impl RcloneRemote {
    fn new(name: String, provider_type: Option<String>) -> Self {
        let display_name = display_name_for_remote_name(&name);
        Self {
            name,
            display_name,
            provider_type,
            state: RcloneRemoteState::Disconnected,
        }
    }

    pub(super) fn identity(&self) -> RcloneRemoteIdentity {
        RcloneRemoteIdentity {
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            provider_type: self.provider_type.clone(),
        }
    }

    pub(super) fn sidebar_state(&self) -> RcloneSidebarState {
        match &self.state {
            RcloneRemoteState::Disconnected => RcloneSidebarState::Disconnected,
            RcloneRemoteState::Connecting => RcloneSidebarState::Connecting,
            RcloneRemoteState::Mounted(_) => RcloneSidebarState::Mounted,
            RcloneRemoteState::TransferMode(_) => RcloneSidebarState::TransferMode,
            RcloneRemoteState::Error(_) => RcloneSidebarState::Error,
        }
    }
}

impl RcloneRemoteState {
    #[cfg(test)]
    fn label(&self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Mounted(_) => "Mounted",
            Self::TransferMode(_) => "Transfer mode",
            Self::Error(_) => "Error",
        }
    }
}

pub(super) fn discover_remotes(settings: &RcloneSettings) -> Vec<RcloneRemote> {
    if prepare_librclone(settings).is_err() {
        return Vec::new();
    }
    discover_remotes_with_client(&LibrcloneClient).unwrap_or_default()
}

pub(super) fn discover_remotes_with_client(
    client: &impl RcloneClient,
) -> Result<Vec<RcloneRemote>, String> {
    let list = client.rpc("config/listremotes", json!({}))?;
    let dump = client.rpc("config/dump", json!({})).unwrap_or(Value::Null);
    let mut remotes = list
        .get("remotes")
        .and_then(Value::as_array)
        .ok_or_else(|| "rclone config/listremotes response did not contain remotes".to_owned())?
        .iter()
        .filter_map(Value::as_str)
        .map(normalized_remote_name)
        .filter(|name| !name.is_empty())
        .map(|name| {
            let provider_type = provider_type_from_config_dump(&dump, &name);
            RcloneRemote::new(name, provider_type)
        })
        .collect::<Vec<_>>();

    remotes.sort_by(|left, right| {
        left.display_name
            .to_ascii_lowercase()
            .cmp(&right.display_name.to_ascii_lowercase())
            .then_with(|| left.display_name.cmp(&right.display_name))
    });
    Ok(remotes)
}

#[cfg(test)]
fn discover_remotes_with_client_and_settings(
    client: &impl RcloneClient,
    settings: &RcloneSettings,
) -> Result<Vec<RcloneRemote>, String> {
    apply_rclone_config_with_client(client, settings)?;
    discover_remotes_with_client(client)
}

fn provider_type_from_config_dump(dump: &Value, remote_name: &str) -> Option<String> {
    dump.get(remote_name)
        .or_else(|| dump.get(format!("{remote_name}:")))
        .and_then(Value::as_object)
        .and_then(|config| config.get("type"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn production_existing_mount_policy() -> ExistingMountPolicy {
    if cfg!(target_os = "windows") {
        ExistingMountPolicy::TrustAccessibleRoot
    } else {
        ExistingMountPolicy::RequireKnownMount
    }
}

fn existing_accessible_mount_root(
    mount_root: &Path,
    existing_mount_policy: ExistingMountPolicy,
) -> Option<PathBuf> {
    if existing_mount_policy != ExistingMountPolicy::TrustAccessibleRoot {
        return None;
    }
    mount_root_is_accessible(mount_root).then(|| mount_root.to_path_buf())
}

fn mount_root_is_accessible(mount_root: &Path) -> bool {
    fs::metadata(mount_root).is_ok_and(|metadata| metadata.is_dir())
}

pub(super) fn connect_remote(
    remote: RcloneRemote,
    settings: &RcloneSettings,
) -> Result<RcloneConnection, String> {
    prepare_librclone(settings)?;
    Ok(
        connect_remote_with_client(&LibrcloneClient, remote, settings).unwrap_or_else(|remote| {
            RcloneConnection::TransferMode(transfer_remote(remote.identity(), None))
        }),
    )
}

pub(super) fn remote_for_virtual_path(
    path: &Path,
    settings: &RcloneSettings,
) -> Result<Option<RcloneRemote>, String> {
    ensure_enabled(settings)?;
    let Some(parsed) = parse_virtual_path(path) else {
        return Ok(None);
    };
    Ok(discover_remotes(settings)
        .into_iter()
        .find(|remote| remote.name == parsed.remote_name)
        .or_else(|| Some(RcloneRemote::new(parsed.remote_name, None))))
}

pub(super) fn connect_remote_with_client(
    client: &impl RcloneClient,
    remote: RcloneRemote,
    settings: &RcloneSettings,
) -> Result<RcloneConnection, RcloneRemote> {
    let requested_mount_root = mount_root_for_remote(&remote.name);
    connect_remote_with_client_and_mount_root(
        client,
        remote,
        settings,
        requested_mount_root,
        production_existing_mount_policy(),
    )
}

fn connect_remote_with_client_and_mount_root(
    client: &impl RcloneClient,
    remote: RcloneRemote,
    settings: &RcloneSettings,
    requested_mount_root: PathBuf,
    existing_mount_policy: ExistingMountPolicy,
) -> Result<RcloneConnection, RcloneRemote> {
    if let Some(connection) =
        existing_mounted_connection(&remote, &requested_mount_root, existing_mount_policy)
    {
        return Ok(connection);
    }

    let mount_types = match mount_types(client) {
        Ok(types) if !types.is_empty() => types,
        Err(error) => {
            return Ok(RcloneConnection::TransferMode(transfer_remote(
                remote.identity(),
                Some(error),
            )));
        }
        Ok(_) => {
            return Ok(RcloneConnection::TransferMode(transfer_remote(
                remote.identity(),
                Some("rclone did not report any supported mount types".to_owned()),
            )));
        }
    };

    if !cfg!(target_os = "windows")
        && let Some(parent) = requested_mount_root.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return Ok(RcloneConnection::TransferMode(transfer_remote(
            remote.identity(),
            Some(format!(
                "Could not create rclone mount parent {}: {error}",
                parent.display()
            )),
        )));
    }

    let mut config = json!({
        "BufferSize": settings.mount.buffer_size,
    });
    if let Some(cache_dir) = settings.mount.cache_dir.configured_path()
        && let Some(config) = config.as_object_mut()
    {
        config.insert(
            "CacheDir".to_owned(),
            Value::String(cache_dir.to_string_lossy().into_owned()),
        );
    }

    let input = json!({
        "_config": config,
        "fs": rclone_fs_for_remote(&remote.name),
        "mountOpt": {
            "AllowOther": settings.mount.allow_other,
        },
        "mountPoint": requested_mount_root.to_string_lossy(),
        "mountType": preferred_mount_type(&mount_types),
        "vfsOpt": {
            "CacheMaxAge": settings.mount.vfs_cache_max_age,
            "CacheMaxSize": settings.mount.vfs_cache_max_size,
            "CacheMode": settings.mount.vfs_cache_mode,
            "ChunkSize": settings.mount.vfs_read_chunk_size,
            "ChunkSizeLimit": settings.mount.vfs_read_chunk_size_limit,
            "DirCacheTime": settings.mount.dir_cache_time,
            "ReadAhead": settings.mount.vfs_read_ahead,
            "ReadOnly": settings.mount.read_only,
        },
    });
    let mounted = client.rpc("mount/mount", input);
    let response = match mounted {
        Ok(response) => response,
        Err(error) => {
            if let Some(connection) =
                existing_mounted_connection(&remote, &requested_mount_root, existing_mount_policy)
            {
                return Ok(connection);
            }
            return Ok(RcloneConnection::TransferMode(transfer_remote(
                remote.identity(),
                Some(error),
            )));
        }
    };

    let mount_root = response
        .get("mountPoint")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(requested_mount_root);
    store_mount_manifest_entry(&remote.name, &mount_root);

    Ok(RcloneConnection::Mounted(mounted_remote(
        &remote, mount_root,
    )))
}

fn existing_mounted_connection(
    remote: &RcloneRemote,
    mount_root: &Path,
    existing_mount_policy: ExistingMountPolicy,
) -> Option<RcloneConnection> {
    let mount_root = existing_accessible_mount_root(mount_root, existing_mount_policy)?;
    store_mount_manifest_entry(&remote.name, &mount_root);
    Some(RcloneConnection::Mounted(mounted_remote(
        remote, mount_root,
    )))
}

fn mounted_remote(remote: &RcloneRemote, mount_root: PathBuf) -> MountedRemote {
    MountedRemote {
        remote: remote.identity(),
        display_root: display_root_for_remote(&remote.display_name),
        mount_root,
    }
}

fn transfer_remote(remote: RcloneRemoteIdentity, mount_error: Option<String>) -> TransferRemote {
    TransferRemote {
        remote,
        mount_error: mount_error.filter(|error| !error.trim().is_empty()),
    }
}

pub(super) fn disconnect_mounted_remote(
    mount_root: &Path,
    settings: &RcloneSettings,
) -> Result<(), String> {
    prepare_librclone(settings)?;
    disconnect_mounted_remote_with_client(&LibrcloneClient, mount_root)
}

pub(super) fn disconnect_mounted_remote_with_client(
    client: &impl RcloneClient,
    mount_root: &Path,
) -> Result<(), String> {
    client.rpc(
        "mount/unmount",
        json!({ "mountPoint": mount_root.to_string_lossy() }),
    )?;
    remove_mount_manifest_path(mount_root);
    Ok(())
}

pub(super) fn apply_known_mount_states(remotes: &mut [RcloneRemote]) {
    let manifest = load_reconciled_mount_manifest_with_client(&LibrcloneClient).ok();
    for remote in remotes {
        apply_known_mount_state(
            remote,
            manifest.as_ref(),
            production_existing_mount_policy(),
        );
    }
}

pub(super) fn apply_connecting_remote_states(remotes: &mut [RcloneRemote]) {
    for remote in remotes {
        if matches!(remote.state, RcloneRemoteState::Mounted(_)) {
            continue;
        }
        if remote_is_connecting(&remote.name) {
            remote.state = RcloneRemoteState::Connecting;
        }
    }
}

fn apply_known_mount_state(
    remote: &mut RcloneRemote,
    manifest: Option<&RcloneMountManifest>,
    existing_mount_policy: ExistingMountPolicy,
) {
    if let Some(mount_root) = manifest
        .and_then(|manifest| manifest.mounts.get(&normalized_remote_name(&remote.name)))
        .cloned()
        .or_else(|| {
            let mount_root = mount_root_for_remote(&remote.name);
            existing_accessible_mount_root(&mount_root, existing_mount_policy)
        })
    {
        store_mount_manifest_entry(&remote.name, &mount_root);
        remote.state = RcloneRemoteState::Mounted(Box::new(mounted_remote(remote, mount_root)));
    }
}

pub(super) fn sidebar_path_for_remote(remote: &RcloneRemote) -> PathBuf {
    match &remote.state {
        RcloneRemoteState::Mounted(mounted) => mounted.mount_root.clone(),
        _ => virtual_root_for_remote(&remote.name),
    }
}

pub(super) fn is_managed_mount_root(path: &Path) -> bool {
    load_mount_manifest()
        .mounts
        .values()
        .any(|mount_root| mount_root == path)
}

fn managed_mounted_path_with_manifest(
    path: &Path,
    manifest: &RcloneMountManifest,
) -> Option<RclonePath> {
    manifest
        .mounts
        .iter()
        .filter_map(|(remote_name, mount_root)| {
            let relative_path = path.strip_prefix(mount_root).ok()?;
            Some((
                mount_root.components().count(),
                RclonePath {
                    remote_name: remote_name.clone(),
                    relative_path: relative_path.to_path_buf(),
                },
            ))
        })
        .max_by_key(|(component_count, _)| *component_count)
        .map(|(_, rclone_path)| rclone_path)
}

fn path_is_managed_mount_with_manifest(path: &Path, manifest: &RcloneMountManifest) -> bool {
    manifest
        .mounts
        .values()
        .any(|mount_root| path_is_same_or_descendant(path, mount_root))
}

pub(super) fn move_managed_mounted_file(
    source_path: &Path,
    target_path: &Path,
    settings: &RcloneSettings,
) -> Result<bool, String> {
    prepare_librclone(settings)?;
    move_managed_mounted_file_with_client_and_manifest(
        &LibrcloneClient,
        source_path,
        target_path,
        &load_mount_manifest(),
    )
}

fn move_managed_mounted_file_with_client_and_manifest(
    client: &impl RcloneClient,
    source_path: &Path,
    target_path: &Path,
    manifest: &RcloneMountManifest,
) -> Result<bool, String> {
    let Some(source) = managed_mounted_path_with_manifest(source_path, manifest) else {
        return Ok(false);
    };
    let Some(target) = managed_mounted_path_with_manifest(target_path, manifest) else {
        return Ok(false);
    };

    run_transfer_job(
        client,
        "operations/movefile",
        json!({
            "srcFs": rclone_fs_for_remote(&source.remote_name),
            "srcRemote": remote_path_string(&source.relative_path),
            "dstFs": rclone_fs_for_remote(&target.remote_name),
            "dstRemote": remote_path_string(&target.relative_path),
        }),
    )?;
    Ok(true)
}

fn mount_types(client: &impl RcloneClient) -> Result<Vec<String>, String> {
    let response = client.rpc("mount/types", json!({}))?;
    Ok(response
        .get("mountTypes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect())
}

fn preferred_mount_type(mount_types: &[String]) -> &str {
    ["mount", "cmount", "mount2"]
        .into_iter()
        .find(|candidate| {
            mount_types
                .iter()
                .any(|mount_type| mount_type.eq_ignore_ascii_case(candidate))
        })
        .unwrap_or_else(|| mount_types.first().map(String::as_str).unwrap_or("mount"))
}

pub(super) fn load_transfer_entries(
    path: &Path,
    visibility: EntryVisibility,
    settings: &RcloneSettings,
) -> io::Result<Vec<FileEntry>> {
    prepare_librclone(settings).map_err(io::Error::other)?;
    load_transfer_entries_with_client(&LibrcloneClient, path, visibility)
}

pub(super) fn load_transfer_entries_with_client(
    client: &impl RcloneClient,
    path: &Path,
    visibility: EntryVisibility,
) -> io::Result<Vec<FileEntry>> {
    let rclone_path = parse_virtual_path(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a virtual rclone transfer path",
        )
    })?;
    let response = client
        .rpc(
            "operations/list",
            json!({
                "fs": rclone_fs_for_remote(&rclone_path.remote_name),
                "remote": remote_path_string(&rclone_path.relative_path),
                "opt": {
                    "noModTime": false,
                    "noMimeType": true,
                }
            }),
        )
        .map_err(io::Error::other)?;

    let items = response
        .get("list")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("rclone operations/list response did not contain list"))?;

    Ok(items
        .iter()
        .filter_map(|item| transfer_entry_from_value(path, item, visibility))
        .collect())
}

fn transfer_entry_from_value(
    parent: &Path,
    item: &Value,
    visibility: EntryVisibility,
) -> Option<FileEntry> {
    let name = item
        .get("Name")
        .or_else(|| item.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())?;
    if should_hide_transfer_entry(name, visibility) {
        return None;
    }
    let is_dir = item
        .get("IsDir")
        .or_else(|| item.get("isDir"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let size = (!is_dir)
        .then(|| {
            item.get("Size")
                .or_else(|| item.get("size"))
                .and_then(Value::as_u64)
        })
        .flatten();
    let modified = item
        .get("ModTime")
        .or_else(|| item.get("modTime"))
        .and_then(Value::as_str)
        .and_then(parse_rclone_mod_time);

    Some(FileEntry {
        path: parent.join(name),
        name: name.to_owned(),
        kind: if is_dir {
            EntryKind::Directory
        } else {
            EntryKind::File
        },
        modified,
        size,
    })
}

fn should_hide_transfer_entry(name: &str, visibility: EntryVisibility) -> bool {
    name == ".localized"
        || name == ".DS_Store"
        || name == "__MACOSX"
        || (!visibility.show_dotfiles && name.starts_with('.'))
}

fn parse_rclone_mod_time(value: &str) -> Option<SystemTime> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| SystemTime::from(time.with_timezone(&Utc)))
}

pub(super) fn transfer_open_action_label(path: &Path, read_only: bool) -> Option<&'static str> {
    is_transfer_path(path).then_some(if read_only {
        "Open read-only copy"
    } else {
        "Download and open copy"
    })
}

pub(super) fn normal_open_block_message(path: &Path) -> Option<String> {
    is_transfer_path(path).then(|| {
        format!(
            "{} is in rclone transfer mode. Use \"Download and open copy\" or \"Open read-only copy\".",
            path.file_name()
                .and_then(OsStr::to_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("This file")
        )
    })
}

pub(super) fn download_transfer_files_to_temp(
    paths: &[PathBuf],
    read_only: bool,
    settings: &RcloneSettings,
) -> io::Result<Vec<PathBuf>> {
    prepare_librclone(settings).map_err(io::Error::other)?;
    let client = LibrcloneClient;
    download_transfer_files_to_temp_with_client(&client, paths, read_only)
}

pub(super) fn download_transfer_files_to_temp_with_client(
    client: &impl RcloneClient,
    paths: &[PathBuf],
    read_only: bool,
) -> io::Result<Vec<PathBuf>> {
    let mut downloaded = Vec::new();
    for path in paths {
        let rclone_path = parse_virtual_path(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a rclone transfer path", path.display()),
            )
        })?;
        let file_name = path
            .file_name()
            .and_then(OsStr::to_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
        let destination_dir = unique_open_copy_dir(&rclone_path.remote_name)?;
        client
            .rpc(
                "operations/copyfile",
                json!({
                    "srcFs": rclone_fs_for_remote(&rclone_path.remote_name),
                    "srcRemote": remote_path_string(&rclone_path.relative_path),
                    "dstFs": destination_dir.to_string_lossy(),
                    "dstRemote": file_name,
                    "_group": "explorer-rclone-open",
                }),
            )
            .map_err(io::Error::other)?;
        let destination = destination_dir.join(file_name);
        if read_only {
            let mut permissions = fs::metadata(&destination)?.permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&destination, permissions)?;
        }
        downloaded.push(destination);
    }
    Ok(downloaded)
}

pub(super) fn transfer_path_exists(path: &Path, settings: &RcloneSettings) -> Result<bool, String> {
    prepare_librclone(settings)?;
    transfer_path_exists_with_client(&LibrcloneClient, path)
}

pub(super) fn transfer_path_exists_with_client(
    client: &impl RcloneClient,
    path: &Path,
) -> Result<bool, String> {
    Ok(transfer_path_metadata_with_client(client, path)?.is_some())
}

fn transfer_path_metadata_with_client(
    client: &impl RcloneClient,
    path: &Path,
) -> Result<Option<TransferPathMetadata>, String> {
    let rclone_path = parse_virtual_path(path)
        .ok_or_else(|| format!("{} is not a rclone transfer path", path.display()))?;
    let response = client.rpc(
        "operations/stat",
        json!({
            "fs": rclone_fs_for_remote(&rclone_path.remote_name),
            "remote": remote_path_string(&rclone_path.relative_path),
        }),
    )?;
    let Some(item) = response.get("item").filter(|item| !item.is_null()) else {
        return Ok(None);
    };
    Ok(Some(TransferPathMetadata {
        is_dir: item
            .get("IsDir")
            .or_else(|| item.get("isDir"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }))
}

pub(super) fn create_transfer_folder(path: &Path, settings: &RcloneSettings) -> Result<(), String> {
    prepare_librclone(settings)?;
    create_transfer_folder_with_client(&LibrcloneClient, path)
}

pub(super) fn create_transfer_folder_with_client(
    client: &impl RcloneClient,
    path: &Path,
) -> Result<(), String> {
    let rclone_path = parse_virtual_path(path)
        .ok_or_else(|| format!("{} is not a rclone transfer path", path.display()))?;
    client.rpc(
        "operations/mkdir",
        json!({
            "fs": rclone_fs_for_remote(&rclone_path.remote_name),
            "remote": remote_path_string(&rclone_path.relative_path),
        }),
    )?;
    Ok(())
}

pub(super) fn rename_transfer_path(
    original_path: &Path,
    target_path: &Path,
    settings: &RcloneSettings,
) -> io::Result<()> {
    prepare_librclone(settings).map_err(io::Error::other)?;
    rename_transfer_path_with_client(&LibrcloneClient, original_path, target_path)
        .map_err(io::Error::other)
}

pub(super) fn rename_transfer_path_with_client(
    client: &impl RcloneClient,
    original_path: &Path,
    target_path: &Path,
) -> Result<(), String> {
    if transfer_path_exists_with_client(client, target_path)? {
        return Err("an item with this name already exists".to_owned());
    }

    let source_is_dir = path_is_directory_with_client(client, original_path)?;
    if source_is_dir {
        run_transfer_job(
            client,
            "sync/move",
            json!({
                "srcFs": rclone_fs_path_for_path(original_path)?,
                "dstFs": rclone_fs_path_for_path(target_path)?,
                "createEmptySrcDirs": true,
                "deleteEmptySrcDirs": true,
            }),
        )?;
    } else {
        let source = rclone_file_endpoint_for_path(original_path)?;
        let destination = rclone_file_endpoint_for_path(target_path)?;
        run_transfer_job(
            client,
            "operations/movefile",
            json!({
                "srcFs": source.fs,
                "srcRemote": source.remote,
                "dstFs": destination.fs,
                "dstRemote": destination.remote,
            }),
        )?;
    }
    Ok(())
}

pub(super) fn delete_transfer_paths(
    paths: &[PathBuf],
    settings: &RcloneSettings,
) -> Result<(), String> {
    prepare_librclone(settings)?;
    delete_transfer_paths_with_client(&LibrcloneClient, paths)
}

pub(super) fn delete_transfer_paths_with_client(
    client: &impl RcloneClient,
    paths: &[PathBuf],
) -> Result<(), String> {
    for path in paths {
        let rclone_path = parse_virtual_path(path)
            .ok_or_else(|| format!("{} is not a rclone transfer path", path.display()))?;
        let Some(metadata) = transfer_path_metadata_with_client(client, path)? else {
            continue;
        };
        let (method, input) = if metadata.is_dir {
            (
                "operations/purge",
                json!({
                    "fs": rclone_fs_for_remote(&rclone_path.remote_name),
                    "remote": remote_path_string(&rclone_path.relative_path),
                }),
            )
        } else {
            (
                "operations/deletefile",
                json!({
                    "fs": rclone_fs_for_remote(&rclone_path.remote_name),
                    "remote": remote_path_string(&rclone_path.relative_path),
                }),
            )
        };
        run_transfer_job(client, method, input)?;
    }
    Ok(())
}

pub(super) fn copy_or_move_paths_to_transfer_destination(
    paths: &[PathBuf],
    destination: &Path,
    operation: RcloneTransferOperation,
    settings: &RcloneSettings,
) -> Result<Vec<PathBuf>, String> {
    prepare_librclone(settings)?;
    copy_or_move_paths_to_transfer_destination_with_client(
        &LibrcloneClient,
        paths,
        destination,
        operation,
    )
}

pub(super) fn copy_or_move_paths_to_transfer_destination_with_client(
    client: &impl RcloneClient,
    paths: &[PathBuf],
    destination: &Path,
    operation: RcloneTransferOperation,
) -> Result<Vec<PathBuf>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    if !is_transfer_path(destination) && !paths.iter().any(|path| is_transfer_path(path)) {
        return Err("copy/move does not involve an rclone transfer path".to_owned());
    }

    let mut destinations = Vec::new();
    for source in paths {
        let file_name = source
            .file_name()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("{} does not have a file name", source.display()))?;
        let target_path = destination.join(Path::new(file_name));
        if path_exists_with_client(client, &target_path)? {
            return Err(format!(
                "{} already exists",
                target_path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or("The destination item")
            ));
        }

        if path_is_directory_with_client(client, source)? {
            let method = match operation {
                RcloneTransferOperation::Copy => "sync/copy",
                RcloneTransferOperation::Move => "sync/move",
            };
            let mut input = json!({
                "srcFs": rclone_fs_path_for_path(source)?,
                "dstFs": rclone_fs_path_for_path(&target_path)?,
                "createEmptySrcDirs": true,
            });
            if operation == RcloneTransferOperation::Move {
                input["deleteEmptySrcDirs"] = Value::Bool(true);
            }
            run_transfer_job(client, method, input)?;
        } else {
            let method = match operation {
                RcloneTransferOperation::Copy => "operations/copyfile",
                RcloneTransferOperation::Move => "operations/movefile",
            };
            let source = rclone_file_endpoint_for_path(source)?;
            let destination = rclone_file_endpoint_for_path(&target_path)?;
            run_transfer_job(
                client,
                method,
                json!({
                    "srcFs": source.fs,
                    "srcRemote": source.remote,
                    "dstFs": destination.fs,
                    "dstRemote": destination.remote,
                }),
            )?;
        }
        destinations.push(target_path);
    }
    Ok(destinations)
}

fn path_exists_with_client(client: &impl RcloneClient, path: &Path) -> Result<bool, String> {
    if is_transfer_path(path) {
        transfer_path_exists_with_client(client, path)
    } else {
        Ok(path.exists())
    }
}

fn path_is_directory_with_client(client: &impl RcloneClient, path: &Path) -> Result<bool, String> {
    if is_transfer_path(path) {
        transfer_path_metadata_with_client(client, path)?
            .map(|metadata| metadata.is_dir)
            .ok_or_else(|| format!("{} does not exist", path.display()))
    } else {
        Ok(path.is_dir())
    }
}

impl ExplorerView {
    pub(super) fn download_and_open_rclone_copies(
        &mut self,
        paths: Vec<PathBuf>,
        read_only: bool,
        cx: &mut gpui::Context<Self>,
    ) {
        if paths.is_empty() || self.open_with_task.is_some() {
            return;
        }

        self.clear_operation_notice();
        let settings = self.rclone_settings.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let copies = download_transfer_files_to_temp(&paths, read_only, &settings)?;
                    for copy in &copies {
                        open::that_detached(copy)?;
                    }
                    Ok::<(), io::Error>(())
                })
                .await;

            let _ = this.update(cx, |explorer, cx| {
                explorer.open_with_task = None;
                if let Err(error) = result {
                    explorer.set_error_notice(format!("Could not open rclone copy: {error}"));
                }
                cx.notify();
            });
        });
        self.open_with_task = Some(task);
    }
}

fn unique_open_copy_dir(remote_name: &str) -> io::Result<PathBuf> {
    let dir = std::env::temp_dir()
        .join(RCLONE_OPEN_COPY_DIR_NAME)
        .join(sanitize_remote_name(remote_name))
        .join(format!(
            "{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub(super) fn is_transfer_path(path: &Path) -> bool {
    if parse_virtual_path(path).is_none() {
        return false;
    }
    !is_active_managed_mount_path(path)
}

pub(super) fn is_active_managed_mount_path(path: &Path) -> bool {
    if load_reconciled_mount_manifest_with_client(&LibrcloneClient)
        .ok()
        .is_some_and(|manifest| path_is_managed_mount_with_manifest(path, &manifest))
    {
        return true;
    }

    if let Some((remote_name, mount_root)) =
        existing_accessible_mount_root_for_path(path, production_existing_mount_policy())
    {
        store_mount_manifest_entry(&remote_name, &mount_root);
        return true;
    }

    false
}

#[cfg(test)]
fn is_transfer_path_with_manifest(path: &Path, manifest: &RcloneMountManifest) -> bool {
    parse_virtual_path(path).is_some() && !path_is_managed_mount_with_manifest(path, manifest)
}

pub(super) fn parse_virtual_path(path: &Path) -> Option<RclonePath> {
    #[cfg(target_os = "windows")]
    {
        parse_windows_virtual_path(path)
    }

    #[cfg(not(target_os = "windows"))]
    {
        parse_unix_virtual_path(path)
    }
}

pub(super) fn is_virtual_namespace_path(path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        is_windows_virtual_namespace_path(path)
    }

    #[cfg(not(target_os = "windows"))]
    {
        is_unix_virtual_namespace_path(path)
    }
}

#[cfg(target_os = "windows")]
fn is_windows_virtual_namespace_path(path: &Path) -> bool {
    use std::path::Prefix;

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return false;
    };
    let server = match prefix.kind() {
        Prefix::UNC(server, _) | Prefix::VerbatimUNC(server, _) => server,
        _ => return false,
    };
    server
        .to_string_lossy()
        .eq_ignore_ascii_case(RCLONE_VIRTUAL_ROOT)
}

#[cfg(not(target_os = "windows"))]
fn is_unix_virtual_namespace_path(path: &Path) -> bool {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::RootDir)) {
        return false;
    }
    matches!(components.next(), Some(Component::Normal(root)) if root == OsStr::new(RCLONE_VIRTUAL_ROOT))
}

fn existing_accessible_mount_root_for_path(
    path: &Path,
    existing_mount_policy: ExistingMountPolicy,
) -> Option<(String, PathBuf)> {
    let parsed = parse_virtual_path(path)?;
    let mount_root = mount_root_for_remote(&parsed.remote_name);
    if !path_is_same_or_descendant(path, &mount_root) {
        return None;
    }
    existing_accessible_mount_root(&mount_root, existing_mount_policy)
        .map(|mount_root| (parsed.remote_name, mount_root))
}

#[cfg(target_os = "windows")]
fn parse_windows_virtual_path(path: &Path) -> Option<RclonePath> {
    use std::path::Prefix;

    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    let (server, share) = match prefix.kind() {
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => (server, share),
        _ => return None,
    };
    if !server
        .to_string_lossy()
        .eq_ignore_ascii_case(RCLONE_VIRTUAL_ROOT)
    {
        return None;
    }
    let remote_name = share.to_string_lossy().into_owned();
    if remote_name.is_empty() {
        return None;
    }
    if matches!(components.clone().next(), Some(Component::RootDir)) {
        components.next();
    }
    let relative_path = components.collect::<PathBuf>();
    Some(RclonePath {
        remote_name,
        relative_path,
    })
}

#[cfg(not(target_os = "windows"))]
fn parse_unix_virtual_path(path: &Path) -> Option<RclonePath> {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::RootDir)) {
        return None;
    }
    let Component::Normal(root) = components.next()? else {
        return None;
    };
    if root != OsStr::new(RCLONE_VIRTUAL_ROOT) {
        return None;
    }
    let Component::Normal(remote) = components.next()? else {
        return None;
    };
    let remote_name = remote.to_string_lossy().into_owned();
    if remote_name.is_empty() {
        return None;
    }
    Some(RclonePath {
        remote_name,
        relative_path: components.collect(),
    })
}

pub(super) fn virtual_root_for_remote(remote_name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(format!(
            "\\\\{}\\{}\\",
            RCLONE_VIRTUAL_ROOT,
            normalized_remote_name(remote_name)
        ))
    }

    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from("/")
            .join(RCLONE_VIRTUAL_ROOT)
            .join(normalized_remote_name(remote_name))
    }
}

pub(super) fn mount_root_for_remote(remote_name: &str) -> PathBuf {
    let sanitized = sanitize_remote_name(remote_name);
    platform_mount_root(&sanitized)
}

fn platform_mount_root(sanitized_remote_name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(format!(
            "\\\\{}\\{}",
            RCLONE_VIRTUAL_ROOT, sanitized_remote_name
        ))
    }

    #[cfg(not(target_os = "windows"))]
    {
        user_home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Explorer")
            .join("Rclone")
            .join(sanitized_remote_name)
    }
}

fn display_root_for_remote(display_name: &str) -> PathBuf {
    PathBuf::from("Rclone").join(display_name)
}

fn manifest_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(RCLONE_MANIFEST_FILE_NAME))
}

fn load_mount_manifest() -> RcloneMountManifest {
    let Some(path) = manifest_path() else {
        return RcloneMountManifest::default();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|json| serde_json::from_str::<RcloneMountManifest>(&json).ok())
        .unwrap_or_default()
}

fn load_reconciled_mount_manifest_with_client(
    client: &impl RcloneClient,
) -> Result<RcloneMountManifest, String> {
    let manifest = load_mount_manifest();
    let reconciled = reconcile_mount_manifest_with_client(client, manifest)?;
    let _ = save_mount_manifest(&reconciled);
    Ok(reconciled)
}

fn reconcile_mount_manifest_with_client(
    client: &impl RcloneClient,
    manifest: RcloneMountManifest,
) -> Result<RcloneMountManifest, String> {
    let active_mount_points = active_mount_points_with_client(client)?;
    Ok(reconcile_mount_manifest(manifest, &active_mount_points))
}

fn reconcile_mount_manifest(
    mut manifest: RcloneMountManifest,
    active_mount_points: &[PathBuf],
) -> RcloneMountManifest {
    manifest.mounts.retain(|_, mount_root| {
        active_mount_points
            .iter()
            .any(|active| mount_paths_match(mount_root, active))
    });
    manifest
}

fn active_mount_points_with_client(client: &impl RcloneClient) -> Result<Vec<PathBuf>, String> {
    let response = client.rpc("mount/listmounts", json!({}))?;
    active_mount_points_from_response(&response)
}

fn active_mount_points_from_response(response: &Value) -> Result<Vec<PathBuf>, String> {
    let mount_points = response
        .get("mountPoints")
        .and_then(Value::as_array)
        .ok_or_else(|| "rclone mount/listmounts response did not contain mountPoints".to_owned())?;

    Ok(mount_points
        .iter()
        .filter_map(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect())
}

#[cfg(target_os = "windows")]
fn mount_paths_match(left: &Path, right: &Path) -> bool {
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    left.trim_end_matches(['\\', '/'])
        .eq_ignore_ascii_case(right.trim_end_matches(['\\', '/']))
}

#[cfg(not(target_os = "windows"))]
fn mount_paths_match(left: &Path, right: &Path) -> bool {
    left == right
}

fn save_mount_manifest(manifest: &RcloneMountManifest) -> io::Result<()> {
    let Some(path) = manifest_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
    fs::write(path, json)
}

fn store_mount_manifest_entry(remote_name: &str, mount_root: &Path) {
    let mut manifest = load_mount_manifest();
    manifest.mounts.insert(
        normalized_remote_name(remote_name),
        mount_root.to_path_buf(),
    );
    let _ = save_mount_manifest(&manifest);
}

fn remove_mount_manifest_path(mount_root: &Path) {
    let mut manifest = load_mount_manifest();
    manifest.mounts.retain(|_, path| path != mount_root);
    let _ = save_mount_manifest(&manifest);
}

pub(super) fn sanitize_remote_name(remote_name: &str) -> String {
    let sanitized = normalized_remote_name(remote_name)
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>()
        .trim_matches([' ', '.'])
        .to_owned();

    if sanitized.is_empty() {
        "remote".to_owned()
    } else {
        sanitized
    }
}

fn normalized_remote_name(remote_name: &str) -> String {
    remote_name.trim().trim_end_matches(':').to_owned()
}

fn display_name_for_remote_name(remote_name: &str) -> String {
    normalized_remote_name(remote_name)
}

fn rclone_fs_for_remote(remote_name: &str) -> String {
    format!("{}:", normalized_remote_name(remote_name))
}

fn rclone_fs_path_for_path(path: &Path) -> Result<String, String> {
    if let Some(rclone_path) = parse_virtual_path(path) {
        let mut fs = rclone_fs_for_remote(&rclone_path.remote_name);
        let remote = remote_path_string(&rclone_path.relative_path);
        if !remote.is_empty() {
            fs.push_str(&remote);
        }
        Ok(fs)
    } else {
        Ok(path.to_string_lossy().into_owned())
    }
}

fn rclone_file_endpoint_for_path(path: &Path) -> Result<RcloneFileEndpoint, String> {
    if let Some(rclone_path) = parse_virtual_path(path) {
        Ok(RcloneFileEndpoint {
            fs: rclone_fs_for_remote(&rclone_path.remote_name),
            remote: remote_path_string(&rclone_path.relative_path),
        })
    } else {
        Ok(RcloneFileEndpoint {
            fs: "/".to_owned(),
            remote: path.to_string_lossy().into_owned(),
        })
    }
}

fn remote_path_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn run_transfer_job(
    client: &impl RcloneClient,
    method: &str,
    mut input: Value,
) -> Result<Value, String> {
    if let Some(input) = input.as_object_mut() {
        input.insert("_async".to_owned(), Value::Bool(true));
        input.insert(
            "_group".to_owned(),
            Value::String(RCLONE_TRANSFER_GROUP.to_owned()),
        );
    }
    let response = client.rpc(method, input)?;
    let Some(job_id) = response.get("jobid").and_then(Value::as_i64) else {
        return Ok(response);
    };

    for _ in 0..RCLONE_JOB_POLL_LIMIT {
        let status = client.rpc("job/status", json!({ "jobid": job_id }))?;
        if !status
            .get("finished")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            thread::sleep(RCLONE_JOB_POLL_INTERVAL);
            continue;
        }

        if status
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(status);
        }

        let error = status
            .get("error")
            .and_then(Value::as_str)
            .filter(|error| !error.is_empty())
            .unwrap_or("rclone job failed");
        return Err(error.to_owned());
    }

    Err(format!("{method} did not finish"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use crate::settings::RcloneCacheDirSetting;
    use std::{cell::RefCell, collections::VecDeque};

    #[derive(Default)]
    struct FakeRcloneClient {
        calls: RefCell<Vec<(String, Value)>>,
        responses: RefCell<VecDeque<Result<Value, String>>>,
    }

    impl FakeRcloneClient {
        fn with_responses(responses: Vec<Result<Value, String>>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                responses: RefCell::new(responses.into()),
            }
        }

        fn calls(&self) -> Vec<(String, Value)> {
            self.calls.borrow().clone()
        }
    }

    impl RcloneClient for FakeRcloneClient {
        fn rpc(&self, method: &str, input: Value) -> Result<Value, String> {
            self.calls
                .borrow_mut()
                .push((method.to_owned(), input.clone()));
            self.responses
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(format!("unexpected call to {method}")))
        }
    }

    struct MountErrorCreatesRootClient {
        calls: RefCell<Vec<(String, Value)>>,
        mount_root: PathBuf,
    }

    impl MountErrorCreatesRootClient {
        fn new(mount_root: PathBuf) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                mount_root,
            }
        }

        fn calls(&self) -> Vec<(String, Value)> {
            self.calls.borrow().clone()
        }
    }

    impl RcloneClient for MountErrorCreatesRootClient {
        fn rpc(&self, method: &str, input: Value) -> Result<Value, String> {
            self.calls
                .borrow_mut()
                .push((method.to_owned(), input.clone()));
            match method {
                "mount/types" => Ok(json!({ "mountTypes": ["mount"] })),
                "mount/mount" => {
                    fs::create_dir_all(&self.mount_root)
                        .map_err(|error| format!("create mount root: {error}"))?;
                    Err("Cannot create WinFsp-FUSE file system. Status=80070050".to_owned())
                }
                _ => Err(format!("unexpected call to {method}")),
            }
        }
    }

    fn queued_finished_job(job_id: i64) -> Vec<Result<Value, String>> {
        vec![
            Ok(json!({ "jobid": job_id })),
            Ok(json!({ "finished": true, "success": true })),
        ]
    }

    #[test]
    fn sanitizes_remote_names_for_mount_paths() {
        assert_eq!(sanitize_remote_name("gdrive:"), "gdrive");
        assert_eq!(
            sanitize_remote_name(" Work/Docs:Archive? "),
            "Work_Docs_Archive_"
        );
        assert_eq!(sanitize_remote_name("..."), "remote");
    }

    #[test]
    fn virtual_roots_use_requested_platform_shape() {
        let root = virtual_root_for_remote("gdrive:");
        if cfg!(target_os = "windows") {
            assert_eq!(root, PathBuf::from(r"\\rclone\gdrive\"));
        } else {
            assert_eq!(root, PathBuf::from("/rclone/gdrive"));
        }
    }

    #[test]
    fn parses_virtual_transfer_paths() {
        let path = if cfg!(target_os = "windows") {
            PathBuf::from(r"\\rclone\gdrive\Folder\File.txt")
        } else {
            PathBuf::from("/rclone/gdrive/Folder/File.txt")
        };

        let parsed = parse_virtual_path(&path).expect("parse rclone path");

        assert_eq!(parsed.remote_name, "gdrive");
        assert_eq!(remote_path_string(&parsed.relative_path), "Folder/File.txt");
    }

    #[test]
    fn parses_managed_mounted_paths_from_manifest() {
        let mount_root = if cfg!(target_os = "windows") {
            PathBuf::from(r"X:\Explorer\Rclone\gdrive")
        } else {
            PathBuf::from("/tmp/explorer-rclone/gdrive")
        };
        let mut manifest = RcloneMountManifest::default();
        manifest
            .mounts
            .insert("gdrive".to_owned(), mount_root.clone());
        let path = mount_root.join("Folder").join("File.txt");

        let parsed =
            managed_mounted_path_with_manifest(&path, &manifest).expect("managed mount path");

        assert_eq!(parsed.remote_name, "gdrive");
        assert_eq!(remote_path_string(&parsed.relative_path), "Folder/File.txt");
    }

    #[test]
    fn mount_root_accessibility_requires_directory() {
        let temp = TempDir::new();
        let file = temp.path().join("file.txt");
        fs::write(&file, b"file").expect("write test file");

        assert!(mount_root_is_accessible(temp.path()));
        assert!(!mount_root_is_accessible(&file));
        assert!(!mount_root_is_accessible(&temp.path().join("missing")));
    }

    #[test]
    fn transfer_path_classification_prefers_active_managed_mounts() {
        let mount_root = virtual_root_for_remote("gdrive");
        let mut manifest = RcloneMountManifest::default();
        manifest
            .mounts
            .insert("gdrive".to_owned(), mount_root.clone());

        assert!(path_is_managed_mount_with_manifest(
            &mount_root.join("Folder"),
            &manifest
        ));
        assert!(!is_transfer_path_with_manifest(&mount_root, &manifest));
        assert!(!is_transfer_path_with_manifest(
            &mount_root.join("Folder"),
            &manifest
        ));
    }

    #[test]
    fn transfer_path_classification_keeps_unmanaged_virtual_paths() {
        let path = virtual_root_for_remote("gdrive");
        let mut stale_manifest = RcloneMountManifest::default();
        stale_manifest
            .mounts
            .insert("stale".to_owned(), virtual_root_for_remote("stale"));

        assert!(is_transfer_path_with_manifest(
            &path,
            &RcloneMountManifest::default()
        ));
        assert!(is_transfer_path_with_manifest(&path, &stale_manifest));
    }

    #[test]
    fn parses_active_mount_points_from_listmounts_response() {
        let response = json!({
            "mountPoints": ["/mnt/gdrive", "", 42, "/mnt/hbox"]
        });

        let mount_points =
            active_mount_points_from_response(&response).expect("parse mount points");

        assert_eq!(
            mount_points,
            vec![PathBuf::from("/mnt/gdrive"), PathBuf::from("/mnt/hbox")]
        );
    }

    #[test]
    fn reconciled_mount_manifest_keeps_only_active_mounts() {
        let mut manifest = RcloneMountManifest::default();
        manifest
            .mounts
            .insert("gdrive".to_owned(), PathBuf::from("/mnt/gdrive"));
        manifest
            .mounts
            .insert("stale".to_owned(), PathBuf::from("/mnt/stale"));
        let client = FakeRcloneClient::with_responses(vec![Ok(json!({
            "mountPoints": ["/mnt/gdrive"]
        }))]);

        let reconciled =
            reconcile_mount_manifest_with_client(&client, manifest).expect("reconcile manifest");

        assert_eq!(
            reconciled.mounts.keys().cloned().collect::<Vec<_>>(),
            vec!["gdrive".to_owned()]
        );
        assert_eq!(client.calls()[0].0, "mount/listmounts");
    }

    #[test]
    fn mount_list_failure_does_not_trust_manifest() {
        let mut manifest = RcloneMountManifest::default();
        manifest
            .mounts
            .insert("gdrive".to_owned(), PathBuf::from("/mnt/gdrive"));
        let client = FakeRcloneClient::with_responses(vec![Err("mount list failed".to_owned())]);

        let error =
            reconcile_mount_manifest_with_client(&client, manifest).expect_err("list should fail");

        assert_eq!(error, "mount list failed");
    }

    #[test]
    fn known_mount_state_ignores_stale_reconciled_manifest() {
        let mut manifest = RcloneMountManifest::default();
        manifest
            .mounts
            .insert("gdrive".to_owned(), PathBuf::from("/mnt/gdrive"));
        let reconciled = reconcile_mount_manifest(manifest, &[PathBuf::from("/mnt/other")]);
        let mut remote = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));

        apply_known_mount_state(
            &mut remote,
            Some(&reconciled),
            ExistingMountPolicy::RequireKnownMount,
        );

        assert!(matches!(remote.state, RcloneRemoteState::Disconnected));
    }

    #[test]
    fn known_mount_state_uses_active_reconciled_manifest() {
        let mut manifest = RcloneMountManifest::default();
        manifest
            .mounts
            .insert("gdrive".to_owned(), PathBuf::from("/mnt/gdrive"));
        let mut remote = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));

        apply_known_mount_state(
            &mut remote,
            Some(&manifest),
            ExistingMountPolicy::RequireKnownMount,
        );

        assert!(matches!(
            remote.state,
            RcloneRemoteState::Mounted(ref mounted) if mounted.mount_root == Path::new("/mnt/gdrive")
        ));
    }

    #[test]
    fn remote_connection_permit_blocks_same_remote_until_drop() {
        reset_connecting_remotes_for_test();

        let gdrive = try_begin_remote_connection("gdrive").expect("first permit");
        assert!(remote_is_connecting("gdrive"));
        assert!(try_begin_remote_connection("gdrive:").is_none());

        let dropbox = try_begin_remote_connection("dropbox").expect("different remote permit");
        assert!(remote_is_connecting("dropbox"));

        drop(gdrive);
        assert!(!remote_is_connecting("gdrive"));
        assert!(remote_is_connecting("dropbox"));

        drop(dropbox);
        assert!(!remote_is_connecting("dropbox"));
    }

    #[test]
    fn connecting_state_applies_without_overriding_mounted_remote() {
        reset_connecting_remotes_for_test();
        let permit = try_begin_remote_connection("gdrive").expect("connect permit");
        let mut connecting = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));
        let mut mounted = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));
        mounted.state = RcloneRemoteState::Mounted(Box::new(mounted_remote(
            &mounted,
            PathBuf::from("/mnt/gdrive"),
        )));

        apply_connecting_remote_states(std::slice::from_mut(&mut connecting));
        apply_connecting_remote_states(std::slice::from_mut(&mut mounted));

        assert!(matches!(connecting.state, RcloneRemoteState::Connecting));
        assert!(matches!(mounted.state, RcloneRemoteState::Mounted(_)));

        drop(permit);
    }

    #[test]
    fn mount_root_uses_platform_default_without_manifest() {
        assert_eq!(
            mount_root_for_remote("gdrive"),
            platform_mount_root(&sanitize_remote_name("gdrive"))
        );
    }

    #[test]
    fn sidebar_path_uses_mount_root_only_for_mounted_remotes() {
        let mut remote = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));
        assert_eq!(
            sidebar_path_for_remote(&remote),
            virtual_root_for_remote("gdrive")
        );

        remote.state = RcloneRemoteState::Mounted(Box::new(MountedRemote {
            remote: remote.identity(),
            mount_root: PathBuf::from("/mnt/gdrive"),
            display_root: display_root_for_remote(&remote.display_name),
        }));

        assert_eq!(sidebar_path_for_remote(&remote), Path::new("/mnt/gdrive"));
    }

    #[test]
    fn managed_mount_move_file_uses_async_movefile() {
        let mount_root = if cfg!(target_os = "windows") {
            PathBuf::from(r"X:\Explorer\Rclone\gdrive")
        } else {
            PathBuf::from("/tmp/explorer-rclone/gdrive")
        };
        let mut manifest = RcloneMountManifest::default();
        manifest
            .mounts
            .insert("gdrive".to_owned(), mount_root.clone());
        let source = mount_root.join(".explorer-copy-key-file.txt.partial");
        let target = mount_root.join("Folder").join("file.txt");
        let mut responses = Vec::new();
        responses.extend(queued_finished_job(11));
        let client = FakeRcloneClient::with_responses(responses);

        let handled = move_managed_mounted_file_with_client_and_manifest(
            &client, &source, &target, &manifest,
        )
        .expect("move managed mount file");

        assert!(handled);
        let calls = client.calls();
        assert_eq!(calls[0].0, "operations/movefile");
        assert_eq!(calls[0].1["srcFs"], "gdrive:");
        assert_eq!(
            calls[0].1["srcRemote"],
            ".explorer-copy-key-file.txt.partial"
        );
        assert_eq!(calls[0].1["dstFs"], "gdrive:");
        assert_eq!(calls[0].1["dstRemote"], "Folder/file.txt");
        assert_eq!(calls[0].1["_async"], true);
        assert_eq!(calls[0].1["_group"], RCLONE_TRANSFER_GROUP);
        assert_eq!(calls[1].0, "job/status");
        assert_eq!(calls[1].1["jobid"], 11);
    }

    #[test]
    fn discovers_remotes_with_provider_types() {
        let client = FakeRcloneClient::with_responses(vec![
            Ok(json!({ "remotes": ["b2:", "gdrive:"] })),
            Ok(json!({
                "gdrive": { "type": "drive" },
                "b2": { "type": "b2" }
            })),
        ]);

        let remotes = discover_remotes_with_client(&client).expect("discover remotes");

        assert_eq!(
            remotes
                .iter()
                .map(|remote| (&remote.name, remote.provider_type.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                (&"b2".to_owned(), Some("b2")),
                (&"gdrive".to_owned(), Some("drive"))
            ]
        );
        assert_eq!(
            client
                .calls()
                .iter()
                .map(|(method, _)| method.as_str())
                .collect::<Vec<_>>(),
            vec!["config/listremotes", "config/dump"]
        );
    }

    #[test]
    fn configured_conf_path_is_set_before_discovery() {
        let conf_path = std::env::temp_dir().join("explorer-rclone-test.conf");
        let settings = RcloneSettings {
            conf_path: Some(conf_path.clone()),
            ..RcloneSettings::default()
        };
        let client = FakeRcloneClient::with_responses(vec![
            Ok(json!({})),
            Ok(json!({ "remotes": ["gdrive:"] })),
            Ok(json!({ "gdrive": { "type": "drive" } })),
        ]);

        let remotes = discover_remotes_with_client_and_settings(&client, &settings)
            .expect("discover remotes");

        assert_eq!(remotes.len(), 1);
        let calls = client.calls();
        assert_eq!(
            calls
                .iter()
                .map(|(method, _)| method.as_str())
                .collect::<Vec<_>>(),
            vec!["config/setpath", "config/listremotes", "config/dump"]
        );
        assert_eq!(calls[0].1["path"], conf_path.to_string_lossy().into_owned());
    }

    #[test]
    fn disabled_settings_stop_configured_discovery_before_rpc() {
        let settings = RcloneSettings {
            enabled: false,
            ..RcloneSettings::default()
        };
        let client = FakeRcloneClient::default();

        let error = discover_remotes_with_client_and_settings(&client, &settings)
            .expect_err("disabled rclone should stop discovery");

        assert_eq!(error, disabled_error());
        assert!(client.calls().is_empty());
    }

    #[test]
    fn mount_success_returns_mounted_remote() {
        let client = FakeRcloneClient::with_responses(vec![
            Ok(json!({ "mountTypes": ["mount", "cmount"] })),
            Ok(json!({ "mountPoint": "/tmp/mounted-gdrive" })),
        ]);
        let remote = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));
        let settings = RcloneSettings::default();

        let connection =
            connect_remote_with_client(&client, remote, &settings).expect("connect remote");

        assert!(matches!(
            connection,
            RcloneConnection::Mounted(MountedRemote { ref mount_root, .. })
                if mount_root == Path::new("/tmp/mounted-gdrive")
        ));
        let calls = client.calls();
        assert_eq!(calls[0].0, "mount/types");
        assert_eq!(calls[1].0, "mount/mount");
        assert_eq!(calls[1].1["fs"], "gdrive:");
        assert_eq!(calls[1].1["mountOpt"]["AllowOther"], true);
        assert_eq!(calls[1].1["vfsOpt"]["ReadOnly"], true);
        assert_eq!(calls[1].1["vfsOpt"]["DirCacheTime"], "48h");
        assert_eq!(calls[1].1["vfsOpt"]["CacheMode"], "full");
        assert_eq!(calls[1].1["vfsOpt"]["CacheMaxSize"], "150G");
        assert_eq!(calls[1].1["vfsOpt"]["CacheMaxAge"], "336h");
        assert_eq!(calls[1].1["vfsOpt"]["ReadAhead"], "256M");
        assert_eq!(calls[1].1["vfsOpt"]["ChunkSize"], "32M");
        assert_eq!(calls[1].1["vfsOpt"]["ChunkSizeLimit"], "2G");
        assert_eq!(calls[1].1["_config"]["BufferSize"], "128M");
        assert!(calls[1].1["_config"].get("CacheDir").is_none());
    }

    #[test]
    fn existing_accessible_mount_root_short_circuits_mount_rpc() {
        let temp = TempDir::new();
        let client = FakeRcloneClient::default();
        let remote = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));

        let connection = connect_remote_with_client_and_mount_root(
            &client,
            remote,
            &RcloneSettings::default(),
            temp.path().to_path_buf(),
            ExistingMountPolicy::TrustAccessibleRoot,
        )
        .expect("connect existing mount");

        assert!(matches!(
            connection,
            RcloneConnection::Mounted(MountedRemote { ref mount_root, .. })
                if mount_root == temp.path()
        ));
        assert!(client.calls().is_empty());
    }

    #[test]
    fn mount_error_adopts_accessible_mount_root() {
        let temp = TempDir::new();
        let mount_root = temp.path().join("mounted-gdrive");
        let client = MountErrorCreatesRootClient::new(mount_root.clone());
        let remote = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));

        let connection = connect_remote_with_client_and_mount_root(
            &client,
            remote,
            &RcloneSettings::default(),
            mount_root.clone(),
            ExistingMountPolicy::TrustAccessibleRoot,
        )
        .expect("connect existing mount after mount error");

        assert!(matches!(
            connection,
            RcloneConnection::Mounted(MountedRemote { mount_root: ref actual, .. })
                if actual == &mount_root
        ));
        assert_eq!(
            client
                .calls()
                .iter()
                .map(|(method, _)| method.as_str())
                .collect::<Vec<_>>(),
            vec!["mount/types", "mount/mount"]
        );
    }

    #[test]
    fn configured_mount_cache_dir_sets_config_override() {
        let client = FakeRcloneClient::with_responses(vec![
            Ok(json!({ "mountTypes": ["mount"] })),
            Ok(json!({ "mountPoint": "/tmp/mounted-gdrive" })),
        ]);
        let remote = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));
        let mut settings = RcloneSettings::default();
        settings.mount.cache_dir = RcloneCacheDirSetting::Path(PathBuf::from("~/rclone-cache"));

        let _ = connect_remote_with_client(&client, remote, &settings).expect("connect remote");

        let calls = client.calls();
        assert_eq!(calls[1].0, "mount/mount");
        assert_eq!(calls[1].1["_config"]["CacheDir"], "~/rclone-cache");
    }

    #[test]
    fn mount_preflight_failure_falls_back_to_transfer_mode() {
        let client = FakeRcloneClient::with_responses(vec![Err("missing fuse".to_owned())]);
        let remote = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));

        let connection = connect_remote_with_client(&client, remote, &RcloneSettings::default())
            .expect("connect remote");

        assert!(matches!(connection, RcloneConnection::TransferMode(_)));
    }

    #[test]
    fn mount_error_falls_back_to_transfer_mode() {
        let client = FakeRcloneClient::with_responses(vec![
            Ok(json!({ "mountTypes": ["mount"] })),
            Err("mount failed".to_owned()),
        ]);
        let remote = RcloneRemote::new("gdrive".to_owned(), Some("drive".to_owned()));

        let connection = connect_remote_with_client(&client, remote, &RcloneSettings::default())
            .expect("connect remote");

        assert!(matches!(
            connection,
            RcloneConnection::TransferMode(TransferRemote {
                mount_error: Some(ref error),
                ..
            }) if error == "mount failed"
        ));
    }

    #[test]
    fn unmount_calls_rc_mount_unmount() {
        let client = FakeRcloneClient::with_responses(vec![Ok(json!({}))]);

        disconnect_mounted_remote_with_client(&client, Path::new("/tmp/gdrive")).unwrap();

        assert_eq!(client.calls()[0].0, "mount/unmount");
        assert_eq!(client.calls()[0].1["mountPoint"], "/tmp/gdrive");
    }

    #[test]
    fn transfer_listing_maps_rclone_items_to_file_entries() {
        let client = FakeRcloneClient::with_responses(vec![Ok(json!({
            "list": [
                { "Name": "folder", "IsDir": true, "ModTime": "2026-06-01T12:00:00Z" },
                { "Name": "file.txt", "IsDir": false, "Size": 42, "ModTime": "2026-06-01T12:01:00Z" },
                { "Name": ".hidden", "IsDir": false, "Size": 1 }
            ]
        }))]);
        let path = virtual_root_for_remote("gdrive");

        let entries =
            load_transfer_entries_with_client(&client, &path, EntryVisibility::new(false, false))
                .expect("list transfer entries");

        assert_eq!(
            entries
                .iter()
                .map(|entry| (&entry.name, entry.is_directory_like(), entry.size))
                .collect::<Vec<_>>(),
            vec![
                (&"folder".to_owned(), true, None),
                (&"file.txt".to_owned(), false, Some(42))
            ]
        );
        assert_eq!(client.calls()[0].0, "operations/list");
        assert_eq!(client.calls()[0].1["fs"], "gdrive:");
        assert_eq!(client.calls()[0].1["remote"], "");
    }

    #[test]
    fn transfer_stat_and_mkdir_use_remote_path() {
        let path = virtual_root_for_remote("gdrive").join("New folder");
        let client =
            FakeRcloneClient::with_responses(vec![Ok(json!({ "item": null })), Ok(json!({}))]);

        assert!(!transfer_path_exists_with_client(&client, &path).unwrap());
        create_transfer_folder_with_client(&client, &path).unwrap();

        let calls = client.calls();
        assert_eq!(calls[0].0, "operations/stat");
        assert_eq!(calls[0].1["remote"], "New folder");
        assert_eq!(calls[1].0, "operations/mkdir");
        assert_eq!(calls[1].1["remote"], "New folder");
    }

    #[test]
    fn transfer_rename_file_uses_async_movefile() {
        let source = virtual_root_for_remote("gdrive").join("old.txt");
        let target = virtual_root_for_remote("gdrive").join("new.txt");
        let mut responses = vec![
            Ok(json!({ "item": null })),
            Ok(json!({ "item": { "IsDir": false } })),
        ];
        responses.extend(queued_finished_job(7));
        let client = FakeRcloneClient::with_responses(responses);

        rename_transfer_path_with_client(&client, &source, &target).unwrap();

        let calls = client.calls();
        assert_eq!(calls[2].0, "operations/movefile");
        assert_eq!(calls[2].1["srcFs"], "gdrive:");
        assert_eq!(calls[2].1["srcRemote"], "old.txt");
        assert_eq!(calls[2].1["dstFs"], "gdrive:");
        assert_eq!(calls[2].1["dstRemote"], "new.txt");
        assert_eq!(calls[2].1["_async"], true);
        assert_eq!(calls[2].1["_group"], RCLONE_TRANSFER_GROUP);
        assert_eq!(calls[3].0, "job/status");
        assert_eq!(calls[3].1["jobid"], 7);
    }

    #[test]
    fn transfer_delete_uses_deletefile_and_purge_by_item_kind() {
        let file = virtual_root_for_remote("gdrive").join("old.txt");
        let folder = virtual_root_for_remote("gdrive").join("Old");
        let mut responses = vec![Ok(json!({ "item": { "IsDir": false } }))];
        responses.extend(queued_finished_job(1));
        responses.push(Ok(json!({ "item": { "IsDir": true } })));
        responses.extend(queued_finished_job(2));
        let client = FakeRcloneClient::with_responses(responses);

        delete_transfer_paths_with_client(&client, &[file, folder]).unwrap();

        let calls = client.calls();
        assert_eq!(calls[1].0, "operations/deletefile");
        assert_eq!(calls[1].1["remote"], "old.txt");
        assert_eq!(calls[3].0, "operations/stat");
        assert_eq!(calls[4].0, "operations/purge");
        assert_eq!(calls[4].1["remote"], "Old");
    }

    #[test]
    fn transfer_copy_file_uses_copyfile_to_destination_directory() {
        let source = virtual_root_for_remote("gdrive").join("file.txt");
        let destination = virtual_root_for_remote("gdrive").join("Target");
        let mut responses = vec![
            Ok(json!({ "item": null })),
            Ok(json!({ "item": { "IsDir": false } })),
        ];
        responses.extend(queued_finished_job(3));
        let client = FakeRcloneClient::with_responses(responses);

        let copied = copy_or_move_paths_to_transfer_destination_with_client(
            &client,
            std::slice::from_ref(&source),
            &destination,
            RcloneTransferOperation::Copy,
        )
        .unwrap();

        assert_eq!(copied, vec![destination.join("file.txt")]);
        let calls = client.calls();
        assert_eq!(calls[2].0, "operations/copyfile");
        assert_eq!(calls[2].1["srcRemote"], "file.txt");
        assert_eq!(calls[2].1["dstRemote"], "Target/file.txt");
        assert_eq!(calls[2].1["_async"], true);
    }

    #[test]
    fn transfer_move_directory_uses_sync_move_for_directory_tree() {
        let source = virtual_root_for_remote("gdrive").join("Folder");
        let destination = virtual_root_for_remote("gdrive").join("Target");
        let mut responses = vec![
            Ok(json!({ "item": null })),
            Ok(json!({ "item": { "IsDir": true } })),
        ];
        responses.extend(queued_finished_job(4));
        let client = FakeRcloneClient::with_responses(responses);

        let moved = copy_or_move_paths_to_transfer_destination_with_client(
            &client,
            std::slice::from_ref(&source),
            &destination,
            RcloneTransferOperation::Move,
        )
        .unwrap();

        assert_eq!(moved, vec![destination.join("Folder")]);
        let calls = client.calls();
        assert_eq!(calls[2].0, "sync/move");
        assert_eq!(calls[2].1["srcFs"], "gdrive:Folder");
        assert_eq!(calls[2].1["dstFs"], "gdrive:Target/Folder");
        assert_eq!(calls[2].1["deleteEmptySrcDirs"], true);
    }

    #[test]
    fn transfer_mode_open_labels_are_explicit() {
        let path = virtual_root_for_remote("gdrive").join("file.txt");

        assert_eq!(
            transfer_open_action_label(&path, false),
            Some("Download and open copy")
        );
        assert_eq!(
            transfer_open_action_label(&path, true),
            Some("Open read-only copy")
        );
        assert_eq!(normal_open_block_message(&path).is_some(), true);
        assert_eq!(
            transfer_open_action_label(Path::new("/tmp/file.txt"), false),
            None
        );
    }

    #[test]
    fn state_labels_cover_public_state_variants() {
        assert_eq!(RcloneRemoteState::Disconnected.label(), "Disconnected");
        assert_eq!(RcloneRemoteState::Connecting.label(), "Connecting");
        assert_eq!(
            RcloneRemoteState::Error("failed".to_owned()).label(),
            "Error"
        );
    }
}
