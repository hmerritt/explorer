//! A native, paginated reader. EPUB resources are read off the UI thread; page
//! boundaries refer to semantic content so they survive changes in typography.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use gpui::actions;

mod book;
mod input;
mod pagination;
mod search;
mod state;
mod styling;
mod view;

pub(crate) use view::open_epub_window;

actions!(
    epub_reader,
    [
        EpubNext,
        EpubPrevious,
        EpubBeginning,
        EpubEnd,
        EpubBack,
        EpubCopy,
        EpubDismiss,
        EpubContents,
        EpubSettings,
        EpubFind,
        EpubBookmark,
        EpubFullscreen,
        EpubFocusNext,
        EpubFocusPrevious,
        EpubActivate
    ]
);

pub(crate) fn epub_existing_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("epub"))
        && path.is_file()
}

pub(crate) fn startup_epub_path(args: impl IntoIterator<Item = OsString>) -> Option<PathBuf> {
    let mut args = args.into_iter();
    let _ = args.next();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if text == "--debug" {
            let _ = args.next();
            continue;
        }
        if text.starts_with('-') {
            continue;
        }
        let path = PathBuf::from(arg);
        return epub_existing_file(&path).then_some(path);
    }
    None
}

#[cfg(test)]
mod tests;
