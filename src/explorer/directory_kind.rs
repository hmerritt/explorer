use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use crate::explorer::filesystem::{
    user_home_dir, user_desktop_dir, user_documents_dir, user_downloads_dir,
    user_pictures_dir, user_videos_dir, user_music_dir,
    macos_applications_dir, macos_bin_dir
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryKind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
    Applications,
    Bin,
}

pub fn resolve_directory_kind(path: &Path) -> Option<DirectoryKind> {
    static SPECIAL_DIRS: OnceLock<Vec<(PathBuf, DirectoryKind)>> = OnceLock::new();
    let special_dirs = SPECIAL_DIRS.get_or_init(|| {
        let home = user_home_dir();
        let mut dirs = Vec::new();

        if let Some(p) = &home { dirs.push((p.clone(), DirectoryKind::Home)); }
        if let Some(p) = user_desktop_dir(home.as_deref()) { dirs.push((p, DirectoryKind::Desktop)); }
        if let Some(p) = user_documents_dir(home.as_deref()) { dirs.push((p, DirectoryKind::Documents)); }
        if let Some(p) = user_downloads_dir(home.as_deref()) { dirs.push((p, DirectoryKind::Downloads)); }
        if let Some(p) = user_pictures_dir(home.as_deref()) { dirs.push((p, DirectoryKind::Pictures)); }
        if let Some(p) = user_videos_dir(home.as_deref()) { dirs.push((p, DirectoryKind::Videos)); }
        if let Some(p) = user_music_dir(home.as_deref()) { dirs.push((p, DirectoryKind::Music)); }

        if let Some(p) = macos_applications_dir() {
            dirs.push((p, DirectoryKind::Applications));
        }
        if let Some(p) = macos_bin_dir(home.as_deref()) {
            dirs.push((p, DirectoryKind::Bin));
        }

        dirs
    });

    special_dirs.iter().find(|(p, _)| p == path).map(|(_, k)| *k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use crate::explorer::test_support::TempDir;

    #[test]
    fn test_resolve_directory_kind() {
        let temp = TempDir::new();
        let home = temp.path().join("home");
        let desktop = home.join("Desktop");
        fs::create_dir_all(&desktop).unwrap();

        // We can't easily mock user_home_dir without environment manipulation,
        // but we can test that it returns None for unknown paths.
        assert_eq!(resolve_directory_kind(temp.path()), None);
        assert_eq!(resolve_directory_kind(&home), None);
    }
}
