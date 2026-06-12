use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use gpui::{App, BorrowAppContext, Global};
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

pub(crate) const APP_ID: &str = "com.hmerritt.explorer";
const LINUX_CONFIG_DIR_NAME: &str = "explorer";
const SETTINGS_FILE_NAME: &str = "settings.json";
const SETTINGS_REFRESH_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigPlatform {
    MacOS,
    Linux,
    Windows,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SidebarLocation {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Videos,
    Music,
    Custom {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StartupLocation {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Videos,
    Music,
    Custom { path: PathBuf },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ExplorerSettings {
    pub sidebar_items: Vec<SidebarLocation>,
    pub show_hidden_files: bool,
    pub show_file_name_extensions: bool,
    pub startup_location: StartupLocation,
}

impl Default for ExplorerSettings {
    fn default() -> Self {
        Self {
            sidebar_items: vec![
                SidebarLocation::Home,
                SidebarLocation::Desktop,
                SidebarLocation::Documents,
                SidebarLocation::Downloads,
            ],
            show_hidden_files: false,
            show_file_name_extensions: true,
            startup_location: StartupLocation::Downloads,
        }
    }
}

pub(crate) struct SettingsState {
    pub(crate) value: ExplorerSettings,
    path: PathBuf,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl Global for SettingsState {}

impl SettingsState {
    pub(crate) fn startup_path(&self) -> PathBuf {
        self.value
            .startup_location
            .resolve()
            .filter(|path| path.is_dir())
            .unwrap_or_else(crate::explorer::default_start_path)
    }

    pub(crate) fn settings_path(&self) -> Option<&Path> {
        (!self.path.as_os_str().is_empty()).then_some(self.path.as_path())
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: ExplorerSettings) -> Self {
        Self {
            value,
            path: PathBuf::new(),
            _watcher: None,
        }
    }
}

impl SidebarLocation {
    pub(crate) fn resolve(&self) -> Option<PathBuf> {
        match self {
            Self::Home => crate::explorer::user_home_dir(),
            Self::Desktop => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_desktop_dir(home.as_deref())
            }
            Self::Documents => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_documents_dir(home.as_deref())
            }
            Self::Downloads => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_downloads_dir(home.as_deref())
            }
            Self::Pictures => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_pictures_dir(home.as_deref())
            }
            Self::Videos => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_videos_dir(home.as_deref())
            }
            Self::Music => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_music_dir(home.as_deref())
            }
            Self::Custom { path, .. } => expand_configured_path(path),
        }
    }
}

impl StartupLocation {
    pub(crate) fn resolve(&self) -> Option<PathBuf> {
        match self {
            Self::Home => crate::explorer::user_home_dir(),
            Self::Desktop => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_desktop_dir(home.as_deref())
            }
            Self::Documents => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_documents_dir(home.as_deref())
            }
            Self::Downloads => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_downloads_dir(home.as_deref())
            }
            Self::Pictures => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_pictures_dir(home.as_deref())
            }
            Self::Videos => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_videos_dir(home.as_deref())
            }
            Self::Music => {
                let home = crate::explorer::user_home_dir();
                crate::explorer::user_music_dir(home.as_deref())
            }
            Self::Custom { path } => expand_configured_path(path),
        }
    }
}

pub(crate) fn initialize(cx: &mut App) {
    let Some(path) = settings_path() else {
        eprintln!("Unable to determine Explorer settings directory; using defaults.");
        cx.set_global(SettingsState {
            value: ExplorerSettings::default(),
            path: PathBuf::new(),
            _watcher: None,
        });
        return;
    };

    let value = load_or_create_settings(&path);
    let (watcher, rx) = settings_watcher(&path);
    cx.set_global(SettingsState {
        value,
        path: path.clone(),
        _watcher: watcher,
    });

    if let Some(rx) = rx {
        spawn_settings_watcher(path, rx, cx);
    }
}

pub(crate) fn set_show_hidden_files(value: bool, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| settings.show_hidden_files = value);
}

pub(crate) fn set_show_file_name_extensions(value: bool, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| settings.show_file_name_extensions = value);
}

pub(crate) fn can_pin_sidebar_path(path: &Path, settings: &ExplorerSettings) -> bool {
    path.is_dir()
        && !settings
            .sidebar_items
            .iter()
            .filter_map(SidebarLocation::resolve)
            .any(|configured_path| configured_path == path)
}

pub(crate) fn pin_sidebar_path(
    path: PathBuf,
    insertion_index: usize,
    cx: &mut impl BorrowAppContext,
) -> bool {
    update_settings(cx, |settings| {
        pin_sidebar_path_in_settings(path, insertion_index, settings)
    })
}

pub(crate) fn reorder_sidebar_item(
    source_index: usize,
    target_index: usize,
    before: bool,
    cx: &mut impl BorrowAppContext,
) -> Option<usize> {
    update_settings(cx, |settings| {
        reorder_sidebar_item_in_settings(source_index, target_index, before, settings)
    })
}

pub(crate) fn unpin_sidebar_item(
    configured_index: usize,
    cx: &mut impl BorrowAppContext,
) -> Option<SidebarLocation> {
    update_settings(cx, |settings| {
        unpin_sidebar_item_in_settings(configured_index, settings)
    })
}

fn update_settings<R>(
    cx: &mut impl BorrowAppContext,
    update: impl FnOnce(&mut ExplorerSettings) -> R,
) -> R {
    cx.update_global::<SettingsState, _>(|state, _| {
        let result = update(&mut state.value);
        if !state.path.as_os_str().is_empty()
            && let Err(error) = save_settings_to_path(&state.path, &state.value)
        {
            eprintln!("Unable to save Explorer settings: {error}");
        }
        result
    })
}

fn sidebar_reorder_index(
    len: usize,
    source_index: usize,
    mut target_index: usize,
    before: bool,
) -> Option<usize> {
    if source_index >= len || target_index >= len || source_index == target_index {
        return None;
    }
    if source_index < target_index {
        target_index -= 1;
    }

    let new_index = if before {
        target_index
    } else {
        target_index + 1
    };
    (new_index != source_index).then_some(new_index)
}

fn pin_sidebar_path_in_settings(
    path: PathBuf,
    insertion_index: usize,
    settings: &mut ExplorerSettings,
) -> bool {
    if !can_pin_sidebar_path(&path, settings) {
        return false;
    }
    let insertion_index = insertion_index.min(settings.sidebar_items.len());
    let location = [
        SidebarLocation::Home,
        SidebarLocation::Desktop,
        SidebarLocation::Documents,
        SidebarLocation::Downloads,
        SidebarLocation::Pictures,
        SidebarLocation::Videos,
        SidebarLocation::Music,
    ]
    .into_iter()
    .find(|loc| loc.resolve().as_ref() == Some(&path))
    .unwrap_or(SidebarLocation::Custom { path, label: None });

    settings.sidebar_items.insert(insertion_index, location);
    true
}

fn reorder_sidebar_item_in_settings(
    source_index: usize,
    target_index: usize,
    before: bool,
    settings: &mut ExplorerSettings,
) -> Option<usize> {
    let new_index = sidebar_reorder_index(
        settings.sidebar_items.len(),
        source_index,
        target_index,
        before,
    )?;
    let item = settings.sidebar_items.remove(source_index);
    settings.sidebar_items.insert(new_index, item);
    Some(new_index)
}

fn unpin_sidebar_item_in_settings(
    configured_index: usize,
    settings: &mut ExplorerSettings,
) -> Option<SidebarLocation> {
    (configured_index < settings.sidebar_items.len())
        .then(|| settings.sidebar_items.remove(configured_index))
}

fn settings_watcher(
    path: &Path,
) -> (
    Option<notify::RecommendedWatcher>,
    Option<Receiver<Vec<PathBuf>>>,
) {
    let Some(parent) = path.parent() else {
        return (None, None);
    };
    let (tx, rx) = mpsc::channel();
    let Ok(mut watcher) =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                let _ = tx.send(event.paths);
            }
        })
    else {
        return (None, None);
    };
    if watcher.watch(parent, RecursiveMode::NonRecursive).is_err() {
        return (None, None);
    }
    (Some(watcher), Some(rx))
}

fn spawn_settings_watcher(path: PathBuf, rx: Receiver<Vec<PathBuf>>, cx: &App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(SETTINGS_REFRESH_INTERVAL)
                .await;

            let mut relevant = false;
            while let Ok(paths) = rx.try_recv() {
                relevant |= paths.is_empty() || paths.iter().any(|event_path| event_path == &path);
            }
            if !relevant {
                continue;
            }

            match load_settings_after_change(&path) {
                Ok(settings) => {
                    let _ = cx.update(|cx| {
                        if cx.global::<SettingsState>().value != settings {
                            cx.global_mut::<SettingsState>().value = settings;
                        }
                    });
                }
                Err(error) => {
                    eprintln!("Unable to reload Explorer settings: {error}");
                }
            }
        }
    })
    .detach();
}

fn load_settings_after_change(path: &Path) -> io::Result<ExplorerSettings> {
    if path.exists() {
        return load_settings_from_path(path);
    }

    let defaults = ExplorerSettings::default();
    save_settings_to_path(path, &defaults)?;
    Ok(defaults)
}

fn load_or_create_settings(path: &Path) -> ExplorerSettings {
    if !path.exists() {
        let defaults = ExplorerSettings::default();
        if let Err(error) = save_settings_to_path(path, &defaults) {
            eprintln!("Unable to create Explorer settings: {error}");
        }
        return defaults;
    }

    load_settings_from_path(path).unwrap_or_else(|error| {
        eprintln!("Unable to load Explorer settings: {error}");
        ExplorerSettings::default()
    })
}

fn load_settings_from_path(path: &Path) -> io::Result<ExplorerSettings> {
    let settings = serde_json::from_str::<ExplorerSettings>(&fs::read_to_string(path)?)
        .map_err(io::Error::other)?;
    validate_settings(&settings)?;
    Ok(settings)
}

fn validate_settings(settings: &ExplorerSettings) -> io::Result<()> {
    for location in &settings.sidebar_items {
        if let SidebarLocation::Custom { path, .. } = location {
            validate_configured_path(path)?;
        }
    }
    if let StartupLocation::Custom { path } = &settings.startup_location {
        validate_configured_path(path)?;
    }
    Ok(())
}

fn validate_configured_path(path: &Path) -> io::Result<()> {
    if path.is_absolute() || is_tilde_path(path) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "configured paths must be absolute or begin with ~/: {}",
                path.display()
            ),
        ))
    }
}

fn expand_configured_path(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    if path == Path::new("~") {
        return crate::explorer::user_home_dir();
    }

    let text = path.to_str()?;
    let remainder = text.strip_prefix("~/")?;
    crate::explorer::user_home_dir().map(|home| home.join(remainder))
}

fn is_tilde_path(path: &Path) -> bool {
    path == Path::new("~")
        || path
            .to_str()
            .is_some_and(|text| text.starts_with("~/") && text.len() > 2)
}

fn save_settings_to_path(path: &Path, settings: &ExplorerSettings) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    fs::write(path, json)
}

pub(crate) fn settings_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(SETTINGS_FILE_NAME))
}

pub(crate) fn config_dir() -> Option<PathBuf> {
    config_dir_for(current_config_platform(), env_path)
}

fn current_config_platform() -> ConfigPlatform {
    if cfg!(target_os = "macos") {
        ConfigPlatform::MacOS
    } else if cfg!(target_os = "windows") {
        ConfigPlatform::Windows
    } else {
        ConfigPlatform::Linux
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    non_empty_path(env::var_os(name)?)
}

fn non_empty_path(value: OsString) -> Option<PathBuf> {
    (!value.as_os_str().is_empty()).then(|| PathBuf::from(value))
}

pub(crate) fn config_dir_for(
    platform: ConfigPlatform,
    mut env_path: impl FnMut(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    match platform {
        ConfigPlatform::MacOS => {
            env_path("HOME").map(|home| home.join(".config").join(LINUX_CONFIG_DIR_NAME))
        }
        ConfigPlatform::Linux => env_path("XDG_CONFIG_HOME")
            .map(|config_home| config_home.join(LINUX_CONFIG_DIR_NAME))
            .or_else(|| {
                env_path("HOME").map(|home| home.join(".config").join(LINUX_CONFIG_DIR_NAME))
            }),
        ConfigPlatform::Windows => env_path("APPDATA")
            .map(|appdata| appdata.join(APP_ID))
            .or_else(|| {
                env_path("USERPROFILE")
                    .map(|profile| profile.join("AppData").join("Roaming").join(APP_ID))
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn defaults_match_generated_settings_contract() {
        let settings = ExplorerSettings::default();
        assert!(!settings.show_hidden_files);
        assert!(settings.show_file_name_extensions);
        assert_eq!(settings.startup_location, StartupLocation::Downloads);
        assert_eq!(settings.sidebar_items.len(), 4);
    }

    #[test]
    fn settings_state_exposes_configured_settings_path() {
        let path = PathBuf::from("configured").join(SETTINGS_FILE_NAME);
        let state = SettingsState {
            value: ExplorerSettings::default(),
            path: path.clone(),
            _watcher: None,
        };

        assert_eq!(state.settings_path(), Some(path.as_path()));
    }

    #[test]
    fn settings_state_hides_unavailable_settings_path() {
        let state = SettingsState::for_test(ExplorerSettings::default());

        assert_eq!(state.settings_path(), None);
    }

    #[test]
    fn settings_default_missing_fields_and_ignore_unknown_fields() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"show_hidden_files":true,"future_option":42}"#)
                .expect("deserialize partial settings");
        assert!(settings.show_hidden_files);
        assert!(settings.show_file_name_extensions);
        assert_eq!(settings.sidebar_items.len(), 4);
    }

    #[test]
    fn settings_round_trip_pretty_json() {
        let path = unique_temp_dir("round-trip").join(SETTINGS_FILE_NAME);
        let settings = ExplorerSettings::default();
        save_settings_to_path(&path, &settings).expect("save settings");
        assert_eq!(load_settings_from_path(&path).unwrap(), settings);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\n  \"sidebar_items\"")
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_settings_are_created_with_defaults() {
        let path = unique_temp_dir("create").join(SETTINGS_FILE_NAME);
        let settings = load_or_create_settings(&path);
        assert_eq!(settings, ExplorerSettings::default());
        assert_eq!(
            load_settings_from_path(&path).unwrap(),
            ExplorerSettings::default()
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn invalid_existing_settings_are_retained_while_defaults_are_used() {
        let path = unique_temp_dir("retain-invalid").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ invalid").unwrap();

        assert_eq!(load_or_create_settings(&path), ExplorerSettings::default());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ invalid");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn live_reload_recreates_deleted_file_and_rejects_malformed_edits() {
        let path = unique_temp_dir("live-reload").join(SETTINGS_FILE_NAME);
        let defaults = load_settings_after_change(&path).expect("recreate deleted settings");
        assert_eq!(defaults, ExplorerSettings::default());
        assert_eq!(load_settings_from_path(&path).unwrap(), defaults);

        fs::write(&path, "{ malformed").unwrap();
        assert!(load_settings_after_change(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ malformed");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn malformed_and_relative_custom_paths_are_rejected() {
        let dir = unique_temp_dir("invalid");
        fs::create_dir_all(&dir).unwrap();
        let malformed = dir.join("malformed.json");
        fs::write(&malformed, "{").unwrap();
        assert!(load_settings_from_path(&malformed).is_err());

        let relative = dir.join("relative.json");
        fs::write(
            &relative,
            r#"{"sidebar_items":[{"kind":"custom","path":"relative"}]}"#,
        )
        .unwrap();
        assert!(load_settings_from_path(&relative).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn configured_paths_accept_absolute_and_tilde_only() {
        let absolute = if cfg!(target_os = "windows") {
            Path::new(r"C:\Users\Ada")
        } else {
            Path::new("/home/ada")
        };
        assert!(validate_configured_path(absolute).is_ok());
        assert!(validate_configured_path(Path::new("~")).is_ok());
        assert!(validate_configured_path(Path::new("~/Downloads")).is_ok());
        assert!(validate_configured_path(Path::new("~other/Downloads")).is_err());
        assert!(validate_configured_path(Path::new("Downloads")).is_err());
    }

    #[test]
    fn startup_path_uses_existing_custom_directory_and_falls_back_for_missing_one() {
        let dir = unique_temp_dir("startup");
        fs::create_dir_all(&dir).unwrap();
        let state = SettingsState::for_test(ExplorerSettings {
            startup_location: StartupLocation::Custom { path: dir.clone() },
            ..ExplorerSettings::default()
        });
        assert_eq!(state.startup_path(), dir);

        let missing = unique_temp_dir("missing-startup");
        let state = SettingsState::for_test(ExplorerSettings {
            startup_location: StartupLocation::Custom {
                path: missing.clone(),
            },
            ..ExplorerSettings::default()
        });
        assert_ne!(state.startup_path(), missing);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pinning_inserts_at_requested_positions_and_rejects_duplicates() {
        let dir = unique_temp_dir("pin-sidebar");
        let first = dir.join("first");
        let second = dir.join("second");
        let third = dir.join("third");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::create_dir_all(&third).unwrap();

        let mut settings = ExplorerSettings {
            sidebar_items: Vec::new(),
            ..ExplorerSettings::default()
        };
        assert!(pin_sidebar_path_in_settings(
            first.clone(),
            0,
            &mut settings
        ));
        assert!(!pin_sidebar_path_in_settings(
            first.clone(),
            0,
            &mut settings
        ));
        assert!(!pin_sidebar_path_in_settings(
            dir.join("missing"),
            0,
            &mut settings
        ));
        assert!(pin_sidebar_path_in_settings(
            second.clone(),
            0,
            &mut settings
        ));
        assert!(pin_sidebar_path_in_settings(
            third.clone(),
            1,
            &mut settings
        ));
        assert_eq!(
            settings
                .sidebar_items
                .iter()
                .filter_map(SidebarLocation::resolve)
                .collect::<Vec<_>>(),
            vec![second, third, first]
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pinning_standard_directory_uses_dedicated_kind() {
        let downloads = SidebarLocation::Downloads.resolve().unwrap();
        if !downloads.exists() {
            fs::create_dir_all(&downloads).unwrap();
        }

        let mut settings = ExplorerSettings {
            sidebar_items: Vec::new(),
            ..ExplorerSettings::default()
        };

        assert!(pin_sidebar_path_in_settings(
            downloads.clone(),
            0,
            &mut settings
        ));
        assert_eq!(settings.sidebar_items.len(), 1);
        assert_eq!(settings.sidebar_items[0], SidebarLocation::Downloads);
    }

    #[test]
    fn sidebar_reorder_indices_move_before_and_after_targets() {
        assert_eq!(sidebar_reorder_index(4, 3, 1, true), Some(1));
        assert_eq!(sidebar_reorder_index(4, 0, 2, false), Some(2));
        assert_eq!(sidebar_reorder_index(4, 1, 1, true), None);
        assert_eq!(sidebar_reorder_index(4, 0, 1, true), None);
        assert_eq!(sidebar_reorder_index(4, 4, 1, true), None);
    }

    #[test]
    fn sidebar_reorder_preserves_invisible_configured_items() {
        let missing = unique_temp_dir("missing-sidebar");
        let mut settings = ExplorerSettings {
            sidebar_items: vec![
                SidebarLocation::Home,
                SidebarLocation::Custom {
                    path: missing.clone(),
                    label: None,
                },
                SidebarLocation::Downloads,
            ],
            ..ExplorerSettings::default()
        };

        assert_eq!(
            reorder_sidebar_item_in_settings(2, 0, true, &mut settings),
            Some(0)
        );
        assert_eq!(
            settings.sidebar_items,
            vec![
                SidebarLocation::Downloads,
                SidebarLocation::Home,
                SidebarLocation::Custom {
                    path: missing,
                    label: None,
                },
            ]
        );
    }

    #[test]
    fn sidebar_unpin_removes_requested_item_and_preserves_order() {
        let first = PathBuf::from("/custom/first");
        let second = PathBuf::from("/custom/second");
        let mut settings = ExplorerSettings {
            sidebar_items: vec![
                SidebarLocation::Home,
                SidebarLocation::Custom {
                    path: first.clone(),
                    label: None,
                },
                SidebarLocation::Custom {
                    path: second.clone(),
                    label: Some("Second".to_owned()),
                },
                SidebarLocation::Downloads,
            ],
            ..ExplorerSettings::default()
        };

        assert_eq!(
            unpin_sidebar_item_in_settings(1, &mut settings),
            Some(SidebarLocation::Custom {
                path: first,
                label: None,
            })
        );
        assert_eq!(
            settings.sidebar_items,
            vec![
                SidebarLocation::Home,
                SidebarLocation::Custom {
                    path: second,
                    label: Some("Second".to_owned()),
                },
                SidebarLocation::Downloads,
            ]
        );
    }

    #[test]
    fn sidebar_unpin_ignores_invalid_indices() {
        let mut settings = ExplorerSettings {
            sidebar_items: vec![SidebarLocation::Home, SidebarLocation::Downloads],
            ..ExplorerSettings::default()
        };
        let original = settings.sidebar_items.clone();

        assert_eq!(unpin_sidebar_item_in_settings(2, &mut settings), None);
        assert_eq!(
            unpin_sidebar_item_in_settings(usize::MAX, &mut settings),
            None
        );
        assert_eq!(settings.sidebar_items, original);
    }

    #[test]
    fn config_paths_follow_platform_conventions() {
        assert_eq!(
            test_config_dir(ConfigPlatform::MacOS, &[("HOME", "home")]),
            Some(
                PathBuf::from("home")
                    .join(".config")
                    .join(LINUX_CONFIG_DIR_NAME)
            )
        );
        assert_eq!(
            test_config_dir(ConfigPlatform::Linux, &[("XDG_CONFIG_HOME", "xdg")]),
            Some(PathBuf::from("xdg").join(LINUX_CONFIG_DIR_NAME))
        );
        assert_eq!(
            test_config_dir(ConfigPlatform::Windows, &[("APPDATA", "appdata")]),
            Some(PathBuf::from("appdata").join(APP_ID))
        );
    }

    fn test_config_dir(platform: ConfigPlatform, vars: &[(&str, &str)]) -> Option<PathBuf> {
        config_dir_for(platform, |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| PathBuf::from(value))
        })
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("explorer-settings-{name}-{nanos}"))
    }
}
