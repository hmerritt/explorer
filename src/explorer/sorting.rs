use std::{cmp::Ordering, ffi::OsStr};

use crate::explorer::entry::FileEntry;

pub(super) fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(compare_entries);
}

fn compare_entries(a: &FileEntry, b: &FileEntry) -> Ordering {
    match (a.sorts_as_directory(), b.sorts_as_directory()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => compare_names(&a.name, &b.name),
    }
}

#[cfg(target_os = "windows")]
fn compare_names(a: &str, b: &str) -> Ordering {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::StrCmpLogicalW;

    let a = OsStr::new(a)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let b = OsStr::new(b)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe { StrCmpLogicalW(a.as_ptr(), b.as_ptr()) };

    result.cmp(&0)
}

#[cfg(not(target_os = "windows"))]
fn compare_names(a: &str, b: &str) -> Ordering {
    natural_key(a).cmp(&natural_key(b)).then_with(|| a.cmp(b))
}

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
enum NaturalPart {
    Text(String),
    Number(u64),
}

#[cfg(not(target_os = "windows"))]
fn natural_key(value: &str) -> Vec<NaturalPart> {
    let mut parts = Vec::new();
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            let mut digits = String::new();
            while let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() {
                    digits.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            parts.push(NaturalPart::Number(digits.parse().unwrap_or(u64::MAX)));
        } else {
            let mut text = String::new();
            while let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() {
                    break;
                }
                text.extend(next.to_lowercase());
                chars.next();
            }
            parts.push(NaturalPart::Text(text));
        }
    }

    parts
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::entry::{DirectoryLinkKind, FileEntry};
    use std::path::PathBuf;

    #[test]
    fn sorts_directories_before_files() {
        let mut entries = vec![
            FileEntry::test("b.txt", false, Some(1), None),
            FileEntry::test("c", true, None, None),
            FileEntry::test("a.txt", false, Some(1), None),
            FileEntry::test("a", true, None, None),
        ];

        sort_entries(&mut entries);

        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["a", "c", "a.txt", "b.txt"]);
    }

    #[test]
    fn sorts_filesystem_directory_links_with_directories() {
        let mut entries = vec![
            FileEntry::test("b.txt", false, Some(1), None),
            FileEntry::test_directory_link("linked", DirectoryLinkKind::FilesystemLink),
            FileEntry::test("a", true, None, None),
        ];

        sort_entries(&mut entries);

        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["a", "linked", "b.txt"]);
    }

    #[test]
    fn sorts_shell_directory_shortcuts_with_files() {
        let mut entries = vec![
            FileEntry::test("z.txt", false, Some(1), None),
            FileEntry::test("folder", true, None, None),
            FileEntry::test_directory_link(
                "a shortcut.lnk",
                DirectoryLinkKind::ShellShortcut {
                    target: PathBuf::from("target"),
                },
            ),
        ];

        sort_entries(&mut entries);

        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["folder", "a shortcut.lnk", "z.txt"]);
    }

    #[test]
    fn sorts_names_naturally() {
        let mut entries = vec![
            FileEntry::test("file10.txt", false, Some(1), None),
            FileEntry::test("file2.txt", false, Some(1), None),
            FileEntry::test("file1.txt", false, Some(1), None),
        ];

        sort_entries(&mut entries);

        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["file1.txt", "file2.txt", "file10.txt"]);
    }

    #[test]
    fn sorting_is_deterministic_for_case_differences() {
        let mut entries = vec![
            FileEntry::test("Readme.md", false, Some(1), None),
            FileEntry::test("README.md", false, Some(1), None),
        ];

        sort_entries(&mut entries);

        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        #[cfg(target_os = "windows")]
        assert_eq!(names, vec!["Readme.md", "README.md"]);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(names, vec!["README.md", "Readme.md"]);
    }
}
