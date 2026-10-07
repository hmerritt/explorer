use std::{ffi::OsString, path::Path};

/// Convert a filesystem path at a native interoperability boundary, without
/// resolving it or changing its spelling beyond Windows separators.
pub(crate) fn native_path(path: &Path) -> OsString {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let units = path
            .as_os_str()
            .encode_wide()
            .map(|unit| {
                if unit == u16::from(b'/') {
                    u16::from(b'\\')
                } else {
                    unit
                }
            })
            .collect::<Vec<_>>();
        OsString::from_wide(&units)
    }
    #[cfg(not(target_os = "windows"))]
    {
        path.as_os_str().to_owned()
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn native_path_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    native_path(path).encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_paths_are_normalized_lexically() {
        for (input, expected) in [
            (r"C:/folder\file.txt", r"C:\folder\file.txt"),
            (r"C:/", r"C:\"),
            (r"C:\folder\", r"C:\folder\"),
            (r"C:folder/./../file", r"C:folder\.\..\file"),
            (r"folder/子 folder/", r"folder\子 folder\"),
            (r"//server/share/folder\file", r"\\server\share\folder\file"),
            (r"\\?\C:\folder/file", r"\\?\C:\folder\file"),
            (
                r"\\?\UNC\server\share/folder",
                r"\\?\UNC\server\share\folder",
            ),
            (r"\\.\C:/folder", r"\\.\C:\folder"),
            ("", ""),
        ] {
            let output = native_path(Path::new(input));
            assert_eq!(output, expected);
            assert_eq!(native_path(Path::new(&output)), output);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_paths_preserve_unpaired_surrogates() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let input = OsString::from_wide(&[0x43, 0x3a, 0x2f, 0xd800, 0x2f, 0xdc00]);
        assert_eq!(
            native_path(Path::new(&input))
                .encode_wide()
                .collect::<Vec<_>>(),
            [0x43, 0x3a, 0x5c, 0xd800, 0x5c, 0xdc00]
        );
        assert_eq!(
            native_path_wide(Path::new(&input)),
            [0x43, 0x3a, 0x5c, 0xd800, 0x5c, 0xdc00, 0]
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths_preserve_backslashes_and_non_utf8_bytes() {
        use std::os::unix::ffi::OsStringExt;

        let input = OsString::from_vec(b"folder\\name/\xff".to_vec());
        assert_eq!(native_path(Path::new(&input)), input);
    }
}
