use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

use xxhash_rust::xxh3::xxh3_64;

use crate::explorer::{
    entry::FileEntry,
    filesystem::{
        archive_path_is_supported, extract_archive_entries_to_directory, list_archive_entries,
    },
};

const ARCHIVE_CACHE_DIRECTORY: &str = "archive-materialized-v1";

#[cfg(target_os = "windows")]
fn virtual_root() -> PathBuf {
    PathBuf::from(r"\\explorer.archive\archives")
}

#[cfg(not(target_os = "windows"))]
fn virtual_root() -> PathBuf {
    PathBuf::from("/__explorer_archive__/archives")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ArchiveFingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug)]
struct ArchiveNode {
    root: PathBuf,
    parent: Option<PathBuf>,
    inner_path: PathBuf,
    name: String,
    is_directory: bool,
}

#[derive(Clone, Debug)]
struct ArchiveRoot {
    source: PathBuf,
    outer_source: PathBuf,
    display_name: String,
    nested_entry: Option<PathBuf>,
    fingerprint: Option<ArchiveFingerprint>,
    indexed: bool,
    inner_to_virtual: HashMap<PathBuf, PathBuf>,
    descendants: Vec<PathBuf>,
}

#[derive(Default)]
struct ArchiveState {
    roots: HashMap<PathBuf, ArchiveRoot>,
    nodes: HashMap<PathBuf, ArchiveNode>,
    mounts: HashMap<String, PathBuf>,
}

fn state() -> &'static Mutex<ArchiveState> {
    static STATE: OnceLock<Mutex<ArchiveState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ArchiveState::default()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ArchiveBreadcrumbInfo {
    pub(super) physical_parent: PathBuf,
    pub(super) segments: Vec<(String, PathBuf)>,
}

pub(super) fn is_archive_path(path: &Path) -> bool {
    path.starts_with(virtual_root())
}

pub(super) fn is_dir(path: &Path) -> bool {
    node(path).is_some_and(|node| node.is_directory)
}

pub(super) fn is_file(path: &Path) -> bool {
    node(path).is_some_and(|node| !node.is_directory)
}

pub(super) fn is_supported_archive_file(path: &Path) -> bool {
    if let Some(node) = node(path) {
        return !node.is_directory && archive_path_is_supported(Path::new(&node.name));
    }
    path.is_file() && archive_path_is_supported(path)
}

pub(super) fn mount(path: &Path) -> Result<PathBuf, String> {
    if let Some(node) = node(path) {
        if node.is_directory || !archive_path_is_supported(Path::new(&node.name)) {
            return Err(format!("{} is not a supported archive.", node.name));
        }
        let materialized = materialize_paths(std::slice::from_ref(&path.to_path_buf()))?
            .into_iter()
            .next()
            .ok_or_else(|| format!("Could not materialize {}.", node.name))?;
        let outer_source = root_for_path(path)
            .map(|root| root.outer_source)
            .unwrap_or_else(|| materialized.clone());
        return mount_source(
            materialized,
            outer_source,
            Some(path.to_path_buf()),
            node.name,
        );
    }

    if !path.is_file() || !archive_path_is_supported(path) {
        return Err(format!(
            "{} is not a supported archive.",
            display_name(path)
        ));
    }
    let source = path.to_path_buf();
    mount_source(source.clone(), source.clone(), None, display_name(&source))
}

fn mount_source(
    source: PathBuf,
    outer_source: PathBuf,
    nested_entry: Option<PathBuf>,
    display_name: String,
) -> Result<PathBuf, String> {
    let mount_key = format!(
        "{}\0{}",
        source.to_string_lossy(),
        nested_entry
            .as_deref()
            .map(Path::to_string_lossy)
            .unwrap_or_default()
    );
    let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
    if let Some(root) = state.mounts.get(&mount_key) {
        return Ok(root.clone());
    }

    let id = xxh3_64(mount_key.as_bytes());
    let root_path = virtual_root().join(format!(
        "archive-{id:016x}-{}",
        safe_virtual_name(&display_name)
    ));
    let root = ArchiveRoot {
        source,
        outer_source,
        display_name: display_name.clone(),
        nested_entry,
        fingerprint: None,
        indexed: false,
        inner_to_virtual: HashMap::from([(PathBuf::new(), root_path.clone())]),
        descendants: Vec::new(),
    };
    state.nodes.insert(
        root_path.clone(),
        ArchiveNode {
            root: root_path.clone(),
            parent: None,
            inner_path: PathBuf::new(),
            name: display_name,
            is_directory: true,
        },
    );
    state.roots.insert(root_path.clone(), root);
    state.mounts.insert(mount_key, root_path.clone());
    Ok(root_path)
}

pub(super) fn list_dir(path: &Path) -> io::Result<Vec<FileEntry>> {
    ensure_index(path).map_err(io::Error::other)?;
    let state = state().lock().unwrap_or_else(|error| error.into_inner());
    let current = state
        .nodes
        .get(path)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "archive location not found"))?;
    if !current.is_directory {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "archive location is not a directory",
        ));
    }

    let root = state
        .roots
        .get(&current.root)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "archive root not found"))?;
    let mut children = root
        .descendants
        .iter()
        .filter_map(|child| state.nodes.get(child))
        .filter(|child| child.parent.as_deref() == Some(path))
        .map(|child| {
            FileEntry::from_provider(
                child_path(child, &state),
                child.name.clone(),
                child.is_directory,
                None,
                None,
            )
        })
        .collect::<Vec<_>>();
    children.shrink_to_fit();
    Ok(children)
}

fn child_path(node: &ArchiveNode, state: &ArchiveState) -> PathBuf {
    state
        .roots
        .get(&node.root)
        .and_then(|root| root.inner_to_virtual.get(&node.inner_path))
        .cloned()
        .unwrap_or_else(|| node.root.clone())
}

fn ensure_index(path: &Path) -> Result<(), String> {
    let root_path = node(path)
        .map(|node| node.root)
        .ok_or_else(|| "Archive location not found.".to_owned())?;
    let (source, old_fingerprint, indexed) = {
        let state = state().lock().unwrap_or_else(|error| error.into_inner());
        let root = state
            .roots
            .get(&root_path)
            .ok_or_else(|| "Archive root not found.".to_owned())?;
        (root.source.clone(), root.fingerprint, root.indexed)
    };
    let fingerprint = fingerprint(&source)?;
    if indexed && old_fingerprint == Some(fingerprint) {
        return Ok(());
    }

    let listed = list_archive_entries(&source)?;
    let mut hierarchy = BTreeMap::<PathBuf, bool>::new();
    for entry in listed {
        let components = normal_components(&entry.path);
        if components.is_empty() {
            continue;
        }
        let mut current = PathBuf::new();
        for (index, component) in components.iter().enumerate() {
            current.push(component);
            let is_final = index + 1 == components.len();
            let is_directory = !is_final || entry.is_directory;
            hierarchy
                .entry(current.clone())
                .and_modify(|existing| *existing |= is_directory)
                .or_insert(is_directory);
        }
    }

    let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
    let Some(existing_root) = state.roots.get(&root_path) else {
        return Err("Archive root was closed while it was loading.".to_owned());
    };
    if existing_root.source != source {
        return Err("Archive source changed while it was loading.".to_owned());
    }

    let stale = existing_root.descendants.clone();
    for path in stale {
        state.nodes.remove(&path);
    }

    let mut inner_to_virtual = HashMap::from([(PathBuf::new(), root_path.clone())]);
    let mut descendants = Vec::with_capacity(hierarchy.len());
    for (inner_path, is_directory) in hierarchy {
        let Some(name) = inner_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        let parent_inner = inner_path.parent().unwrap_or_else(|| Path::new(""));
        let Some(parent_virtual) = inner_to_virtual.get(parent_inner).cloned() else {
            continue;
        };
        let virtual_path = parent_virtual.join(virtual_component(&name, &inner_path));
        state.nodes.insert(
            virtual_path.clone(),
            ArchiveNode {
                root: root_path.clone(),
                parent: Some(parent_virtual),
                inner_path: inner_path.clone(),
                name,
                is_directory,
            },
        );
        inner_to_virtual.insert(inner_path, virtual_path.clone());
        descendants.push(virtual_path);
    }

    let root = state
        .roots
        .get_mut(&root_path)
        .expect("archive root checked above");
    root.fingerprint = Some(fingerprint);
    root.indexed = true;
    root.inner_to_virtual = inner_to_virtual;
    root.descendants = descendants;
    Ok(())
}

pub(super) fn parent(path: &Path) -> Option<PathBuf> {
    let state = state().lock().unwrap_or_else(|error| error.into_inner());
    let node = state.nodes.get(path)?;
    if let Some(parent) = &node.parent {
        return Some(parent.clone());
    }
    let root = state.roots.get(&node.root)?;
    root.nested_entry
        .as_deref()
        .and_then(|entry| state.nodes.get(entry))
        .and_then(|entry| entry.parent.clone())
        .or_else(|| root.source.parent().map(Path::to_path_buf))
}

pub(super) fn exit_selection(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let state = state().lock().unwrap_or_else(|error| error.into_inner());
    let node = state.nodes.get(path)?;
    if node.parent.is_some() {
        return None;
    }
    let root = state.roots.get(&node.root)?;
    if let Some(entry) = &root.nested_entry {
        let parent = state.nodes.get(entry)?.parent.clone()?;
        return Some((parent, entry.clone()));
    }
    Some((root.source.parent()?.to_path_buf(), root.source.clone()))
}

pub(super) fn display_name_for_path(path: &Path) -> Option<String> {
    node(path).map(|node| node.name)
}

pub(super) fn display_address(path: &Path) -> Option<String> {
    let info = breadcrumb_info(path)?;
    let mut address = info.physical_parent.display().to_string();
    for (label, _) in info.segments {
        if !address.ends_with(std::path::MAIN_SEPARATOR) && !address.is_empty() {
            address.push(std::path::MAIN_SEPARATOR);
        }
        address.push_str(&label);
    }
    Some(address)
}

pub(super) fn breadcrumb_info(path: &Path) -> Option<ArchiveBreadcrumbInfo> {
    let state = state().lock().unwrap_or_else(|error| error.into_inner());
    breadcrumb_info_locked(path, &state)
}

fn breadcrumb_info_locked(path: &Path, state: &ArchiveState) -> Option<ArchiveBreadcrumbInfo> {
    let node = state.nodes.get(path)?;
    let root = state.roots.get(&node.root)?;
    let mut info = if let Some(nested_entry) = &root.nested_entry {
        let nested = state.nodes.get(nested_entry)?;
        let containing = nested.parent.as_deref()?;
        let mut info = breadcrumb_info_locked(containing, state)?;
        info.segments
            .push((root.display_name.clone(), node.root.clone()));
        info
    } else {
        ArchiveBreadcrumbInfo {
            physical_parent: root.source.parent()?.to_path_buf(),
            segments: vec![(root.display_name.clone(), node.root.clone())],
        }
    };

    if !node.inner_path.as_os_str().is_empty() {
        let mut inner = PathBuf::new();
        for component in normal_components(&node.inner_path) {
            inner.push(&component);
            let target = root.inner_to_virtual.get(&inner)?.clone();
            info.segments
                .push((component.to_string_lossy().into_owned(), target));
        }
    }
    Some(info)
}

pub(super) fn materialize_for_open(path: &Path) -> io::Result<PathBuf> {
    materialize_paths(std::slice::from_ref(&path.to_path_buf()))
        .map_err(io::Error::other)?
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::other("archive entry was not materialized"))
}

pub(super) fn materialize_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    for path in paths {
        ensure_index(path)?;
    }

    let cache = materialization_cache_dir();
    fs::create_dir_all(&cache)
        .map_err(|error| format!("Could not create archive cache: {error}"))?;
    let batch = cache.join(format!(
        "batch-{}-{}",
        std::process::id(),
        unique_materialization_id()
    ));
    fs::create_dir(&batch).map_err(|error| format!("Could not create archive cache: {error}"))?;

    let result = materialize_paths_into(paths, &batch);
    if result.is_err() {
        let _ = fs::remove_dir_all(&batch);
    }
    result
}

fn materialize_paths_into(paths: &[PathBuf], batch: &Path) -> Result<Vec<PathBuf>, String> {
    #[derive(Default)]
    struct Group {
        source: PathBuf,
        entries: HashSet<PathBuf>,
        directories: HashSet<PathBuf>,
    }

    let (groups, outputs) = {
        let state = state().lock().unwrap_or_else(|error| error.into_inner());
        let mut groups = HashMap::<PathBuf, Group>::new();
        let mut outputs = Vec::with_capacity(paths.len());
        for selected_path in paths {
            let selected = state
                .nodes
                .get(selected_path)
                .ok_or_else(|| "Archive entry not found.".to_owned())?;
            let root = state
                .roots
                .get(&selected.root)
                .ok_or_else(|| "Archive root not found.".to_owned())?;
            let group = groups.entry(selected.root.clone()).or_default();
            group.source = root.source.clone();
            if selected.is_directory {
                group.directories.insert(selected.inner_path.clone());
            } else {
                group.entries.insert(selected.inner_path.clone());
            }
            for descendant_path in &root.descendants {
                let Some(descendant) = state.nodes.get(descendant_path) else {
                    continue;
                };
                if descendant.inner_path.starts_with(&selected.inner_path) {
                    if descendant.is_directory {
                        group.directories.insert(descendant.inner_path.clone());
                    } else {
                        group.entries.insert(descendant.inner_path.clone());
                    }
                }
            }
            outputs.push(batch.join(&selected.inner_path));
        }
        (groups, outputs)
    };

    for group in groups.into_values() {
        for directory in &group.directories {
            fs::create_dir_all(batch.join(directory))
                .map_err(|error| format!("Could not create archive folder: {error}"))?;
        }
        let mut entries = group.entries.into_iter().collect::<Vec<_>>();
        entries.sort();
        extract_archive_entries_to_directory(&group.source, &entries, batch)?;
    }
    Ok(outputs)
}

pub(super) fn cleanup_materialization_cache() {
    let cache = materialization_cache_dir();
    let Ok(entries) = fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}

fn materialization_cache_dir() -> PathBuf {
    crate::settings::config_dir()
        .map(|directory| directory.join("cache").join(ARCHIVE_CACHE_DIRECTORY))
        .unwrap_or_else(|| std::env::temp_dir().join("explorer-archive-materialized"))
}

fn unique_materialization_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn root_for_path(path: &Path) -> Option<ArchiveRoot> {
    let state = state().lock().unwrap_or_else(|error| error.into_inner());
    let root = state.nodes.get(path)?.root.clone();
    state.roots.get(&root).cloned()
}

fn node(path: &Path) -> Option<ArchiveNode> {
    state()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .nodes
        .get(path)
        .cloned()
}

fn fingerprint(path: &Path) -> Result<ArchiveFingerprint, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Could not read {}: {error}", display_name(path)))?;
    if !metadata.is_file() {
        return Err(format!("{} is no longer a file.", display_name(path)));
    }
    Ok(ArchiveFingerprint {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn normal_components(path: &Path) -> Vec<std::ffi::OsString> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            Component::CurDir => None,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => None,
        })
        .collect()
}

fn virtual_component(name: &str, inner_path: &Path) -> String {
    let id = xxh3_64(inner_path.to_string_lossy().as_bytes());
    format!("entry-{id:016x}-{}", safe_virtual_name(name))
}

fn safe_virtual_name(name: &str) -> String {
    let safe = name
        .chars()
        .map(|character| {
            if matches!(
                character,
                '/' | '\\' | '\0' | '<' | '>' | ':' | '"' | '|' | '?' | '*'
            ) {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        "archive-entry".to_owned()
    } else {
        safe
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("archive")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::FileOptions::default();
        for (name, contents) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sample.zip");
        fs::write(path, zip_bytes(entries)).unwrap();
        temp
    }

    #[test]
    fn archive_mount_lists_synthesized_folders_and_files() {
        let temp = zip_with(&[("folder/nested.txt", b"nested"), ("top.txt", b"top")]);
        let root = mount(&temp.path().join("sample.zip")).unwrap();
        let entries = list_dir(&root).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "folder" && entry.is_directory_like())
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "top.txt" && !entry.is_directory_like())
        );
    }

    #[test]
    fn selected_folder_materializes_recursively() {
        let temp = zip_with(&[("folder/nested.txt", b"nested"), ("other.txt", b"other")]);
        let root = mount(&temp.path().join("sample.zip")).unwrap();
        let folder = list_dir(&root)
            .unwrap()
            .into_iter()
            .find(|entry| entry.name == "folder")
            .unwrap();
        let outputs = materialize_paths(std::slice::from_ref(&folder.path)).unwrap();

        assert_eq!(fs::read(outputs[0].join("nested.txt")).unwrap(), b"nested");
        assert!(!outputs[0].parent().unwrap().join("other.txt").exists());
    }

    #[test]
    fn multiple_selected_entries_materialize_without_unselected_files() {
        let temp = zip_with(&[
            ("first.txt", b"first"),
            ("folder/nested.txt", b"nested"),
            ("unselected.txt", b"unselected"),
        ]);
        let root = mount(&temp.path().join("sample.zip")).unwrap();
        let entries = list_dir(&root).unwrap();
        let first = entries
            .iter()
            .find(|entry| entry.name == "first.txt")
            .unwrap();
        let folder = entries.iter().find(|entry| entry.name == "folder").unwrap();

        let outputs = materialize_paths(&[first.path.clone(), folder.path.clone()]).unwrap();

        assert_eq!(fs::read(&outputs[0]).unwrap(), b"first");
        assert_eq!(fs::read(outputs[1].join("nested.txt")).unwrap(), b"nested");
        assert!(!outputs[0].parent().unwrap().join("unselected.txt").exists());
    }

    #[test]
    fn mounted_archive_locations_are_read_only() {
        let temp = zip_with(&[("file.txt", b"file")]);
        let root = mount(&temp.path().join("sample.zip")).unwrap();
        let explorer_fs = crate::explorer::explorer_fs::ExplorerFs::new();

        assert!(!explorer_fs.can_mutate(&root));
        assert_eq!(
            explorer_fs.refresh_driver(&root),
            crate::explorer::explorer_fs::ExplorerRefreshDriver::Poll
        );
        assert_eq!(
            explorer_fs
                .list_dir(
                    &root,
                    crate::explorer::filesystem::EntryVisibility::new(true, true)
                )
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn archive_root_exits_to_parent_and_selects_source() {
        let temp = zip_with(&[("file.txt", b"file")]);
        let source = temp.path().join("sample.zip");
        let root = mount(&source).unwrap();

        assert_eq!(
            exit_selection(&root),
            Some((temp.path().to_path_buf(), source))
        );
    }

    #[test]
    fn breadcrumb_uses_physical_parent_and_virtual_archive_segments() {
        let temp = zip_with(&[("folder/nested.txt", b"nested")]);
        let root = mount(&temp.path().join("sample.zip")).unwrap();
        let folder = list_dir(&root)
            .unwrap()
            .into_iter()
            .find(|entry| entry.name == "folder")
            .unwrap();
        let info = breadcrumb_info(&folder.path).unwrap();

        assert_eq!(info.physical_parent, temp.path());
        assert_eq!(
            info.segments
                .iter()
                .map(|part| part.0.as_str())
                .collect::<Vec<_>>(),
            vec!["sample.zip", "folder"]
        );
    }

    #[test]
    fn nested_archive_mounts_and_preserves_breadcrumb_chain() {
        let inner = zip_bytes(&[("inside.txt", b"inside")]);
        let temp = zip_with(&[("nested.zip", &inner)]);
        let root = mount(&temp.path().join("sample.zip")).unwrap();
        let nested = list_dir(&root)
            .unwrap()
            .into_iter()
            .find(|entry| entry.name == "nested.zip")
            .unwrap();

        let nested_root = mount(&nested.path).unwrap();
        let nested_entries = list_dir(&nested_root).unwrap();
        let info = breadcrumb_info(&nested_root).unwrap();

        assert!(
            nested_entries
                .iter()
                .any(|entry| entry.name == "inside.txt")
        );
        assert_eq!(
            info.segments
                .iter()
                .map(|part| part.0.as_str())
                .collect::<Vec<_>>(),
            vec!["sample.zip", "nested.zip"]
        );
        assert_eq!(exit_selection(&nested_root), Some((root, nested.path)));
    }

    #[test]
    fn fingerprint_uses_size_and_modified_time() {
        let temp = zip_with(&[("file.txt", b"file")]);
        let fingerprint = fingerprint(&temp.path().join("sample.zip")).unwrap();
        assert!(fingerprint.len > 0);
        assert!(fingerprint.modified.unwrap_or(std::time::UNIX_EPOCH) >= std::time::UNIX_EPOCH);
    }
}
