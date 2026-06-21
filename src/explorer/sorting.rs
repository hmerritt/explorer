use std::{cmp::Ordering, ffi::OsStr};

use crate::explorer::entry::FileEntry;
use crate::settings::{FileSortColumn, FileSortSettings, SortDirection};

pub(super) fn sort_entries(entries: &mut [FileEntry], sort: FileSortSettings) {
    entries.sort_by(|a, b| compare_entries(a, b, sort));
}

fn compare_entries(a: &FileEntry, b: &FileEntry, sort: FileSortSettings) -> Ordering {
    match (a.sorts_as_directory(), b.sorts_as_directory()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => compare_entries_in_group(a, b, sort),
    }
}

fn compare_entries_in_group(a: &FileEntry, b: &FileEntry, sort: FileSortSettings) -> Ordering {
    match sort.column {
        FileSortColumn::Name => {
            compare_with_direction(compare_names(&a.name, &b.name), sort.direction)
        }
        FileSortColumn::DateModified => {
            compare_optional_values(a.modified, b.modified, sort.direction)
                .then_with(|| compare_names(&a.name, &b.name))
        }
        FileSortColumn::Size => compare_optional_values(a.size, b.size, sort.direction)
            .then_with(|| compare_names(&a.name, &b.name)),
    }
}

fn compare_optional_values<T: Ord>(
    a: Option<T>,
    b: Option<T>,
    direction: SortDirection,
) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => compare_with_direction(a.cmp(&b), direction),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_with_direction(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
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
    use crate::explorer::entry::{DirectoryLinkKind, FileEntry, ShellShortcutTargetKind};
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn sort(column: FileSortColumn, direction: SortDirection) -> FileSortSettings {
        FileSortSettings { column, direction }
    }

    fn names(entries: &[FileEntry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>()
    }

    #[test]
    fn sorts_directories_before_files() {
        let mut entries = vec![
            FileEntry::test("b.txt", false, Some(1), None),
            FileEntry::test("c", true, None, None),
            FileEntry::test("a.txt", false, Some(1), None),
            FileEntry::test("a", true, None, None),
        ];

        sort_entries(
            &mut entries,
            sort(FileSortColumn::Name, SortDirection::Ascending),
        );

        assert_eq!(names(&entries), vec!["a", "c", "a.txt", "b.txt"]);

        sort_entries(
            &mut entries,
            sort(FileSortColumn::Name, SortDirection::Descending),
        );

        assert_eq!(names(&entries), vec!["c", "a", "b.txt", "a.txt"]);
    }

    #[test]
    fn sorts_filesystem_directory_links_with_directories() {
        let mut entries = vec![
            FileEntry::test("b.txt", false, Some(1), None),
            FileEntry::test_directory_link("linked", DirectoryLinkKind::FilesystemLink),
            FileEntry::test("a", true, None, None),
        ];

        sort_entries(
            &mut entries,
            sort(FileSortColumn::Name, SortDirection::Ascending),
        );

        assert_eq!(names(&entries), vec!["a", "linked", "b.txt"]);
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
                    target_kind: ShellShortcutTargetKind::Directory,
                },
            ),
        ];

        sort_entries(
            &mut entries,
            sort(FileSortColumn::Name, SortDirection::Ascending),
        );

        assert_eq!(names(&entries), vec!["folder", "a shortcut.lnk", "z.txt"]);
    }

    #[test]
    fn sorts_names_naturally() {
        let mut entries = vec![
            FileEntry::test("file10.txt", false, Some(1), None),
            FileEntry::test("file2.txt", false, Some(1), None),
            FileEntry::test("file1.txt", false, Some(1), None),
        ];

        sort_entries(
            &mut entries,
            sort(FileSortColumn::Name, SortDirection::Ascending),
        );

        assert_eq!(
            names(&entries),
            vec!["file1.txt", "file2.txt", "file10.txt"]
        );

        sort_entries(
            &mut entries,
            sort(FileSortColumn::Name, SortDirection::Descending),
        );

        assert_eq!(
            names(&entries),
            vec!["file10.txt", "file2.txt", "file1.txt"]
        );
    }

    #[test]
    fn sorts_by_date_modified_in_both_directions_with_missing_last() {
        let oldest = UNIX_EPOCH + Duration::from_secs(10);
        let newest = UNIX_EPOCH + Duration::from_secs(30);
        let mut entries = vec![
            FileEntry::test("missing.txt", false, Some(1), None),
            FileEntry::test("new.txt", false, Some(1), Some(newest)),
            FileEntry::test("old.txt", false, Some(1), Some(oldest)),
        ];

        sort_entries(
            &mut entries,
            sort(FileSortColumn::DateModified, SortDirection::Ascending),
        );

        assert_eq!(names(&entries), vec!["old.txt", "new.txt", "missing.txt"]);

        sort_entries(
            &mut entries,
            sort(FileSortColumn::DateModified, SortDirection::Descending),
        );

        assert_eq!(names(&entries), vec!["new.txt", "old.txt", "missing.txt"]);
    }

    #[test]
    fn sorts_by_size_in_both_directions_with_missing_last() {
        let mut entries = vec![
            FileEntry::test("missing.txt", false, None, None),
            FileEntry::test("large.txt", false, Some(30), None),
            FileEntry::test("small.txt", false, Some(10), None),
        ];

        sort_entries(
            &mut entries,
            sort(FileSortColumn::Size, SortDirection::Ascending),
        );

        assert_eq!(
            names(&entries),
            vec!["small.txt", "large.txt", "missing.txt"]
        );

        sort_entries(
            &mut entries,
            sort(FileSortColumn::Size, SortDirection::Descending),
        );

        assert_eq!(
            names(&entries),
            vec!["large.txt", "small.txt", "missing.txt"]
        );
    }

    #[test]
    fn metadata_sort_ties_fall_back_to_name_ascending() {
        let mut entries = vec![
            FileEntry::test("b.txt", false, Some(10), None),
            FileEntry::test("a.txt", false, Some(10), None),
        ];

        sort_entries(
            &mut entries,
            sort(FileSortColumn::Size, SortDirection::Descending),
        );

        assert_eq!(names(&entries), vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn sorting_is_deterministic_for_case_differences() {
        let mut entries = vec![
            FileEntry::test("Readme.md", false, Some(1), None),
            FileEntry::test("README.md", false, Some(1), None),
        ];

        sort_entries(
            &mut entries,
            sort(FileSortColumn::Name, SortDirection::Ascending),
        );

        #[cfg(target_os = "windows")]
        assert_eq!(names(&entries), vec!["Readme.md", "README.md"]);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(names(&entries), vec!["README.md", "Readme.md"]);
    }
}
