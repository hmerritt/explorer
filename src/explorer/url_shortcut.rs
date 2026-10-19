use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use gpui::http_client::Url;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct UrlShortcut {
    pub(super) url: Url,
    pub(super) title: Option<String>,
}

pub(super) fn supported_url(text: &str) -> Option<Url> {
    let text = text.trim();
    if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let url = Url::parse(text).ok()?;
    (matches!(url.scheme(), "http" | "https" | "ftp" | "sftp") && url.host_str().is_some())
        .then_some(url)
}

impl UrlShortcut {
    pub(super) fn name(&self) -> String {
        let title = self
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| self.url.host_str().unwrap_or("Shortcut"));
        let mut name: String = title
            .trim()
            .chars()
            .map(|ch| {
                if ch.is_control()
                    || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                {
                    '_'
                } else {
                    ch
                }
            })
            .collect();
        // Leave room for the extension and collision suffix on all supported filesystems.
        while name.len() > 180 || name.encode_utf16().count() > 180 {
            name.pop();
        }
        name = name.trim_end_matches(['.', ' ']).to_owned();
        if name.is_empty() {
            name = "Shortcut".to_owned();
        }
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end()
            .to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|n| {
                    matches!(
                        n,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                })
            })
        {
            name.insert(0, '_');
        }
        name
    }

    pub(super) fn create_in(&self, directory: &Path) -> Result<PathBuf, String> {
        let name = self.name();
        let (extension, bytes) = self.native_contents(&name)?;
        for index in 1..=usize::MAX {
            let suffix = if index == 1 {
                String::new()
            } else {
                format!(" ({index})")
            };
            let path = directory.join(format!("{name}{suffix}.{extension}"));
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("Could not create URL shortcut: {error}")),
            };
            if let Err(error) = file.write_all(&bytes) {
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err(format!("Could not write URL shortcut: {error}"));
            }
            return Ok(path);
        }
        Err("No available URL shortcut filename.".to_owned())
    }

    fn native_contents(&self, _name: &str) -> Result<(&'static str, Vec<u8>), String> {
        #[cfg(target_os = "windows")]
        {
            Ok((
                "url",
                format!("[InternetShortcut]\r\nURL={}\r\n", self.url).into_bytes(),
            ))
        }
        #[cfg(target_os = "macos")]
        {
            let mut dictionary = plist::Dictionary::new();
            dictionary.insert("URL".into(), plist::Value::String(self.url.to_string()));
            let mut bytes = Vec::new();
            plist::Value::Dictionary(dictionary)
                .to_writer_xml(&mut bytes)
                .map_err(|error| error.to_string())?;
            Ok(("webloc", bytes))
        }
        #[cfg(target_os = "linux")]
        {
            Ok((
                "desktop",
                desktop_contents(_name, self.url.as_str()).into_bytes(),
            ))
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn desktop_contents(name: &str, url: &str) -> String {
    fn escape(value: &str) -> String {
        value
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
            .replace(' ', "\\s")
    }
    format!(
        "[Desktop Entry]\nType=Link\nName={}\nURL={}\n",
        escape(name),
        escape(url)
    )
}

#[cfg(target_os = "linux")]
pub(super) fn desktop_link_url(path: &Path) -> Option<String> {
    if path.extension()?.to_str()? != "desktop" {
        return None;
    }
    let contents = std::fs::read_to_string(path).ok()?;
    let entry =
        freedesktop_desktop_entry::DesktopEntry::from_str(path, &contents, None::<&[&str]>).ok()?;
    if entry.desktop_entry("Type")? != "Link" {
        return None;
    }
    supported_url(entry.desktop_entry("URL")?).map(|url| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut(title: Option<&str>) -> UrlShortcut {
        UrlShortcut {
            url: supported_url("https://example.com/a?q=one&b=two#part").unwrap(),
            title: title.map(str::to_owned),
        }
    }

    #[test]
    fn urls_require_supported_scheme_host_and_no_embedded_whitespace() {
        for url in [
            "https://example.com",
            "ftp://example.com/folder/",
            "sftp://example.com",
        ] {
            assert!(supported_url(url).is_some());
        }
        for url in [
            "file:///tmp/test",
            "mailto:a@example.com",
            "hello",
            "https://",
            "https://example.com/a\nb",
            "https://example.com/a b",
        ] {
            assert!(supported_url(url).is_none(), "{url}");
        }
    }

    #[test]
    fn names_are_safe_and_bounded_and_use_title_then_host() {
        assert_eq!(shortcut(None).name(), "example.com");
        assert_eq!(shortcut(Some("  ")).name(), "example.com");
        assert_eq!(shortcut(Some("A/B: title?. ")).name(), "A_B_ title_");
        assert_eq!(shortcut(Some("CON.txt")).name(), "_CON.txt");
        assert_eq!(shortcut(Some("...")).name(), "Shortcut");
        assert!(shortcut(Some(&"界".repeat(200))).name().len() <= 180);
    }

    #[test]
    fn creation_does_not_overwrite_and_reports_bad_destinations() {
        let temp = tempfile::tempdir().unwrap();
        let shortcut = shortcut(Some("Example"));
        let first = shortcut.create_in(temp.path()).unwrap();
        let bytes = std::fs::read(&first).unwrap();
        let second = shortcut.create_in(temp.path()).unwrap();
        assert!(
            second
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(" (2)")
        );
        assert_eq!(std::fs::read(&first).unwrap(), bytes);
        assert!(shortcut.create_in(&temp.path().join("missing")).is_err());
    }

    #[test]
    fn native_shortcut_preserves_url_contents() {
        let shortcut = shortcut(Some("Title & details"));
        let (extension, bytes) = shortcut.native_contents(&shortcut.name()).unwrap();
        #[cfg(target_os = "windows")]
        {
            assert_eq!(extension, "url");
            assert_eq!(
                String::from_utf8(bytes).unwrap(),
                format!("[InternetShortcut]\r\nURL={}\r\n", shortcut.url)
            );
        }
        #[cfg(target_os = "macos")]
        {
            assert_eq!(extension, "webloc");
            let value = plist::Value::from_reader_xml(bytes.as_slice()).unwrap();
            assert_eq!(
                value.as_dictionary().unwrap()["URL"].as_string(),
                Some(shortcut.url.as_str())
            );
        }
        #[cfg(target_os = "linux")]
        {
            assert_eq!(extension, "desktop");
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("test.desktop");
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(
                desktop_link_url(&path).as_deref(),
                Some(shortcut.url.as_str())
            );
            std::fs::write(
                &path,
                "[Desktop Entry]\nType=Application\nExec=echo hello\nURL=https://example.com/\n",
            )
            .unwrap();
            assert!(desktop_link_url(&path).is_none());
        }
    }

    #[test]
    fn desktop_links_escape_values_and_never_contain_exec() {
        let contents = desktop_contents("A \\ B\nC", "https://example.com/?q=a&b=c#d");
        assert!(contents.contains("Type=Link\n"));
        assert!(contents.contains("Name=A\\s\\\\\\sB\\nC\n"));
        assert!(contents.contains("URL=https://example.com/?q=a&b=c#d\n"));
        assert!(!contents.contains("Exec="));
    }
}
