use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
};

use gpui::Context;

use super::{
    ExplorerView,
    explorer_fs::ExplorerFs,
    filesystem::EntryVisibility,
    filesystem::NetworkDriveState,
    sidebar::{SidebarItem, SidebarItemKind, SidebarSections},
    sorting::compare_file_names,
};
use crate::settings::SidebarGroupKind;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SidebarTreeRoot {
    group: SidebarGroupKind,
    path: PathBuf,
    occurrence: usize,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct SidebarTreeKey {
    root: SidebarTreeRoot,
    path: PathBuf,
}

impl SidebarTreeKey {
    pub(super) fn group(&self) -> SidebarGroupKind {
        self.root.group
    }

    pub(super) fn child(&self, path: &Path) -> Self {
        Self {
            root: self.root.clone(),
            path: path.to_owned(),
        }
    }
}

pub(super) fn sidebar_tree_roots(
    group: SidebarGroupKind,
    items: &[SidebarItem],
) -> impl Iterator<Item = (SidebarTreeKey, &SidebarItem)> {
    let mut occurrences = HashMap::new();
    items.iter().map(move |item| {
        let occurrence = occurrences.entry(item.path.as_path()).or_insert(0);
        let root = SidebarTreeRoot {
            group,
            path: item.path.clone(),
            occurrence: *occurrence,
        };
        *occurrence += 1;
        (
            SidebarTreeKey {
                root,
                path: item.path.clone(),
            },
            item,
        )
    })
}

pub(super) fn sidebar_item_can_expand(item: &SidebarItem) -> bool {
    !super::trash::is_root(&item.path)
        && !matches!(
            item.kind,
            SidebarItemKind::Directory(super::DirectoryKind::Bin)
                | SidebarItemKind::DriveNetwork(NetworkDriveState::Disconnected)
                | SidebarItemKind::Remote(NetworkDriveState::Disconnected)
        )
}

pub(super) struct SidebarTreeChild {
    pub(super) row_id: usize,
    pub(super) item: SidebarItem,
}

struct SidebarListing {
    visibility: EntryVisibility,
    children: Vec<SidebarTreeChild>,
}

#[derive(Default)]
struct SidebarBranch {
    expanded: bool,
    request: Option<u64>,
    listing: Option<SidebarListing>,
}

#[derive(Default)]
pub(super) struct SidebarTree {
    branches: HashMap<SidebarTreeKey, SidebarBranch>,
    generation: u64,
    next_child: usize,
}

impl SidebarTree {
    pub(super) fn chevron(
        &self,
        key: &SidebarTreeKey,
        visibility: EntryVisibility,
    ) -> Option<bool> {
        let Some(branch) = self.branches.get(key) else {
            return Some(false);
        };
        if branch
            .listing
            .as_ref()
            .is_some_and(|listing| listing.visibility == visibility && listing.children.is_empty())
        {
            return None;
        }
        Some(branch.expanded)
    }

    pub(super) fn children(
        &self,
        key: &SidebarTreeKey,
        visibility: EntryVisibility,
    ) -> &[SidebarTreeChild] {
        self.branches
            .get(key)
            .filter(|branch| branch.expanded)
            .and_then(|branch| branch.listing.as_ref())
            .filter(|listing| listing.visibility == visibility)
            .map_or(&[], |listing| &listing.children)
    }

    pub(super) fn reconcile_roots(&mut self, sections: &SidebarSections) {
        if self.branches.is_empty() {
            return;
        }
        let roots: HashSet<_> = [
            (
                SidebarGroupKind::Pinned,
                sections.user_directories.as_slice(),
            ),
            (SidebarGroupKind::Drives, sections.drives.as_slice()),
            (
                SidebarGroupKind::Network,
                sections.network_drives.as_slice(),
            ),
            (SidebarGroupKind::Wsl, sections.wsl_drives.as_slice()),
        ]
        .into_iter()
        .flat_map(|(group, items)| sidebar_tree_roots(group, items))
        .filter(|(_, item)| sidebar_item_can_expand(item))
        .map(|(key, _)| key.root)
        .collect();
        self.branches.retain(|key, _| roots.contains(&key.root));
    }

    fn collapse(&mut self, key: &SidebarTreeKey) -> bool {
        let Some(branch) = self.branches.get_mut(key).filter(|branch| branch.expanded) else {
            return false;
        };
        branch.expanded = false;
        branch.request = None;
        true
    }

    fn start(&mut self, key: SidebarTreeKey) -> Option<u64> {
        let branch = self.branches.entry(key).or_default();
        if branch.request.is_some() {
            return None;
        }
        self.generation += 1;
        branch.expanded = true;
        branch.request = Some(self.generation);
        Some(self.generation)
    }

    fn refresh_roots(&mut self) -> Vec<SidebarTreeKey> {
        let mut roots = Vec::new();
        for (key, branch) in &mut self.branches {
            branch.request = None;
            if branch
                .listing
                .as_ref()
                .is_some_and(|listing| listing.children.is_empty())
            {
                branch.listing = None;
            }
            if branch.expanded && key.root.path == key.path {
                roots.push(key.clone());
            }
        }
        roots
    }

    fn finish(
        &mut self,
        key: &SidebarTreeKey,
        generation: u64,
        visibility: EntryVisibility,
        items: io::Result<Vec<SidebarItem>>,
    ) -> Option<Vec<SidebarTreeKey>> {
        let branch = self.branches.get_mut(key)?;
        if branch.request != Some(generation) {
            return None;
        }
        branch.request = None;
        let Ok(items) = items else {
            branch.expanded = false;
            return Some(Vec::new());
        };
        let mut previous_ids: HashMap<_, _> = branch
            .listing
            .take()
            .into_iter()
            .flat_map(|listing| listing.children)
            .map(|child| (child.item.path, child.row_id))
            .collect();
        let children: Vec<_> = items
            .into_iter()
            .map(|item| {
                let row_id = previous_ids.remove(&item.path).unwrap_or_else(|| {
                    // Root rows use low, group-specific IDs.
                    self.next_child += 1;
                    usize::MAX - self.next_child
                });
                SidebarTreeChild { row_id, item }
            })
            .collect();
        branch.expanded = !children.is_empty();
        let child_keys = children
            .iter()
            .map(|child| key.child(&child.item.path))
            .collect::<Vec<_>>();
        branch.listing = Some(SidebarListing {
            visibility,
            children,
        });

        // Provider paths can be opaque, so follow the listed children to remove a subtree.
        let mut removed: Vec<_> = previous_ids.into_keys().collect();
        while let Some(path) = removed.pop() {
            if let Some(listing) = self
                .branches
                .remove(&key.child(&path))
                .and_then(|branch| branch.listing)
            {
                removed.extend(listing.children.into_iter().map(|child| child.item.path));
            }
        }
        Some(
            child_keys
                .into_iter()
                .filter(|child| {
                    self.branches
                        .get(child)
                        .is_some_and(|branch| branch.expanded)
                })
                .collect(),
        )
    }
}

fn list_sidebar_folders(path: &Path, visibility: EntryVisibility) -> io::Result<Vec<SidebarItem>> {
    let mut items: Vec<_> = ExplorerFs::new()
        .list_dir(path, visibility)?
        .into_iter()
        .filter(|entry| entry.sorts_as_directory())
        .map(|entry| SidebarItem {
            label: entry.name,
            path: entry.path,
            kind: SidebarItemKind::CustomDirectory,
            configured_index: None,
        })
        .collect();
    items.sort_by(|a, b| compare_file_names(&a.label, &b.label).then_with(|| a.path.cmp(&b.path)));
    Ok(items)
}

impl ExplorerView {
    pub(super) fn toggle_sidebar_folder(&mut self, key: SidebarTreeKey, cx: &mut Context<Self>) {
        self.close_context_menu();
        if !self.sidebar_tree.collapse(&key) {
            self.load_sidebar_folder(key, cx);
        }
        cx.notify();
    }

    pub(super) fn refresh_sidebar_folders(&mut self, cx: &mut Context<Self>) {
        for key in self.sidebar_tree.refresh_roots() {
            self.load_sidebar_folder(key, cx);
        }
    }

    fn load_sidebar_folder(&mut self, key: SidebarTreeKey, cx: &mut Context<Self>) {
        let Some(generation) = self.sidebar_tree.start(key.clone()) else {
            return;
        };
        let visibility = self.entry_visibility();
        let path = key.path.clone();
        let listing = cx
            .background_executor()
            .spawn(async move { list_sidebar_folders(&path, visibility) });
        cx.spawn(async move |this, cx| {
            let items = listing.await;
            let _ = this.update(cx, |view, cx| {
                let Some(expanded_children) = view
                    .sidebar_tree
                    .finish(&key, generation, visibility, items)
                else {
                    return;
                };
                for child in expanded_children {
                    view.load_sidebar_folder(child, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::explorer::test_support::TempDir;

    fn folder(path: impl Into<PathBuf>) -> SidebarItem {
        let path = path.into();
        SidebarItem {
            label: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            kind: SidebarItemKind::CustomDirectory,
            configured_index: None,
        }
    }

    #[test]
    fn sidebar_folders_follow_visibility_and_natural_name_order() {
        let temp = TempDir::new();
        for name in ["Folder 10", ".hidden", "Folder 2"] {
            fs::create_dir(temp.path().join(name)).unwrap();
        }
        fs::write(temp.path().join("File 1"), "").unwrap();
        let visible = list_sidebar_folders(temp.path(), false.into()).unwrap();
        assert_eq!(
            visible
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["Folder 2", "Folder 10"],
        );
        let all = list_sidebar_folders(temp.path(), true.into()).unwrap();
        assert!(all.iter().any(|item| item.label == ".hidden"));
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn sidebar_tree_ignores_superseded_and_removed_results() {
        let root_items = [folder("root")];
        let (key, _) = sidebar_tree_roots(SidebarGroupKind::Pinned, &root_items)
            .next()
            .unwrap();
        let child = folder("root/child");
        let visibility = false.into();
        let mut tree = SidebarTree::default();
        let first = tree.start(key.clone()).unwrap();
        assert!(tree.collapse(&key));
        let second = tree.start(key.clone()).unwrap();
        assert!(
            tree.finish(&key, first, visibility, Ok(vec![child.clone()]))
                .is_none()
        );
        assert!(tree.children(&key, visibility).is_empty());
        tree.finish(&key, second, visibility, Ok(vec![child.clone()]))
            .unwrap();
        let child_id = tree.children(&key, visibility)[0].row_id;

        assert!(tree.collapse(&key));
        let third = tree.start(key.clone()).unwrap();
        tree.finish(&key, third, visibility, Ok(vec![child.clone()]))
            .unwrap();
        assert_eq!(tree.children(&key, visibility)[0].row_id, child_id);

        let child_key = key.child(&child.path);
        let child_request = tree.start(child_key.clone()).unwrap();
        let refresh = tree.start(key.clone()).unwrap();
        tree.finish(&key, refresh, visibility, Ok(vec![])).unwrap();
        assert!(
            tree.finish(&child_key, child_request, visibility, Ok(vec![]))
                .is_none()
        );

        let pending = tree.start(key.clone()).unwrap();
        tree.reconcile_roots(&SidebarSections::default());
        assert!(
            tree.finish(&key, pending, visibility, Ok(vec![child]))
                .is_none()
        );
    }

    #[test]
    fn sidebar_tree_keeps_duplicate_roots_independent_and_retries_errors() {
        let root_items = [folder("root"), folder("root")];
        let mut pinned = sidebar_tree_roots(SidebarGroupKind::Pinned, &root_items);
        let (first, _) = pinned.next().unwrap();
        let (second, _) = pinned.next().unwrap();
        let (drive, _) = sidebar_tree_roots(SidebarGroupKind::Drives, &root_items)
            .next()
            .unwrap();
        let visibility = false.into();
        let mut tree = SidebarTree::default();
        let request = tree.start(first.clone()).unwrap();
        assert_eq!(tree.chevron(&first, visibility), Some(true));
        assert_eq!(tree.chevron(&second, visibility), Some(false));
        assert_eq!(tree.chevron(&drive, visibility), Some(false));
        tree.finish(
            &first,
            request,
            visibility,
            Err(io::ErrorKind::PermissionDenied.into()),
        )
        .unwrap();
        assert_eq!(tree.chevron(&first, visibility), Some(false));

        let retry = tree.start(first.clone()).unwrap();
        tree.finish(&first, retry, visibility, Ok(vec![])).unwrap();
        assert_eq!(tree.chevron(&first, visibility), None);
        assert!(tree.refresh_roots().is_empty());
        assert_eq!(tree.chevron(&first, visibility), Some(false));
    }
}
