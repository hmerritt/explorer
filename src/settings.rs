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
use serde_json::Value;

pub(crate) const APP_ID: &str = "com.hmerritt.explorer";
pub(crate) const DEFAULT_DATE_FORMAT: &str = "%Y/%m/%d %H:%M";
const LINUX_CONFIG_DIR_NAME: &str = "explorer";
const SETTINGS_FILE_NAME: &str = "settings.json";
const SETTINGS_REFRESH_INTERVAL: Duration = Duration::from_millis(150);
pub(crate) const SIDEBAR_DEFAULT_WIDTH: u32 = 225;
pub(crate) const SIDEBAR_MIN_WIDTH: u32 = 100;

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
    #[serde(default = "default_date_format")]
    pub date_format: String,
    pub show_hidden_files: bool,
    pub show_file_name_extensions: bool,
    pub focus_new_tab_immediately: bool,
    pub resolve_icons: bool,
    pub startup_location: StartupLocation,
    #[serde(
        default = "default_sidebar_width",
        deserialize_with = "deserialize_sidebar_width"
    )]
    pub sidebar_width: u32,
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
            date_format: default_date_format(),
            show_hidden_files: false,
            show_file_name_extensions: true,
            focus_new_tab_immediately: false,
            resolve_icons: true,
            startup_location: StartupLocation::Downloads,
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
        }
    }
}

pub(crate) struct SettingsState {
    pub(crate) value: ExplorerSettings,
    document: Value,
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
        let document = settings_document(&value);
        Self {
            value,
            document,
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
            document: settings_document(&ExplorerSettings::default()),
            path: PathBuf::new(),
            _watcher: None,
        });
        return;
    };

    let loaded = load_or_create_settings(&path);
    let (watcher, rx) = settings_watcher(&path);
    cx.set_global(SettingsState {
        value: loaded.value,
        document: loaded.document,
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

pub(crate) fn set_sidebar_width(value: u32, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| {
        settings.sidebar_width = normalized_sidebar_width(value);
    });
}

pub(crate) fn normalized_sidebar_width(value: u32) -> u32 {
    value.max(SIDEBAR_MIN_WIDTH)
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
        sync_settings_document(&mut state.document, &state.value);
        if !state.path.as_os_str().is_empty()
            && let Err(error) = save_document_to_path(&state.path, &mut state.document)
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
                Ok(loaded) => {
                    let _ = cx.update(|cx| {
                        if cx.global::<SettingsState>().value != loaded.value {
                            cx.global_mut::<SettingsState>().value = loaded.value;
                        }
                        cx.global_mut::<SettingsState>().document = loaded.document;
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

struct LoadedSettings {
    value: ExplorerSettings,
    document: Value,
}

fn load_settings_after_change(path: &Path) -> io::Result<LoadedSettings> {
    if path.exists() {
        return load_settings_document_from_path(path);
    }

    let defaults = ExplorerSettings::default();
    let mut document = settings_document(&defaults);
    save_document_to_path(path, &mut document)?;
    Ok(LoadedSettings {
        value: defaults,
        document,
    })
}

fn load_or_create_settings(path: &Path) -> LoadedSettings {
    if !path.exists() {
        let defaults = ExplorerSettings::default();
        let mut document = settings_document(&defaults);
        if let Err(error) = save_document_to_path(path, &mut document) {
            eprintln!("Unable to create Explorer settings: {error}");
        }
        return LoadedSettings {
            value: defaults,
            document,
        };
    }

    load_settings_document_from_path(path).unwrap_or_else(|error| {
        eprintln!("Unable to load Explorer settings: {error}");
        let value = ExplorerSettings::default();
        LoadedSettings {
            document: settings_document(&value),
            value,
        }
    })
}

#[cfg(test)]
fn load_settings_from_path(path: &Path) -> io::Result<ExplorerSettings> {
    load_settings_document_from_path(path).map(|loaded| loaded.value)
}

fn load_settings_document_from_path(path: &Path) -> io::Result<LoadedSettings> {
    let source = fs::read_to_string(path)?;
    let mut document = serde_json::from_str::<Value>(&source).map_err(io::Error::other)?;
    let value =
        serde_json::from_value::<ExplorerSettings>(document.clone()).map_err(io::Error::other)?;
    validate_settings(&value)?;

    let known = settings_document(&value);
    let object = document.as_object_mut().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "settings must be a JSON object")
    })?;
    for (key, value) in known
        .as_object()
        .expect("serialized settings are an object")
    {
        object.entry(key.clone()).or_insert_with(|| value.clone());
    }

    sort_json_objects(&mut document);
    let normalized = serde_json::to_string_pretty(&document).map_err(io::Error::other)?;
    if source != normalized
        && let Err(error) = fs::write(path, normalized)
    {
        eprintln!("Unable to normalize Explorer settings: {error}");
    }

    Ok(LoadedSettings { value, document })
}

fn validate_settings(settings: &ExplorerSettings) -> io::Result<()> {
    chrono::format::StrftimeItems::new(&settings.date_format)
        .parse()
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid date_format: {error}"),
            )
        })?;
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

#[cfg(test)]
fn save_settings_to_path(path: &Path, settings: &ExplorerSettings) -> io::Result<()> {
    let mut document = settings_document(settings);
    save_document_to_path(path, &mut document)
}

fn save_document_to_path(path: &Path, document: &mut Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    sort_json_objects(document);
    let json = serde_json::to_string_pretty(document).map_err(io::Error::other)?;
    fs::write(path, json)
}

fn settings_document(settings: &ExplorerSettings) -> Value {
    serde_json::to_value(settings).expect("ExplorerSettings serialization cannot fail")
}

fn sync_settings_document(document: &mut Value, settings: &ExplorerSettings) {
    let known = settings_document(settings);
    let Some(document) = document.as_object_mut() else {
        *document = known;
        return;
    };
    let known = known
        .as_object()
        .expect("serialized ExplorerSettings is an object");

    for (key, value) in known {
        if key == "sidebar_items" {
            sync_sidebar_items(document.entry(key.clone()).or_insert(Value::Null), settings);
        } else {
            merge_known_value(document.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

fn sync_sidebar_items(document: &mut Value, settings: &ExplorerSettings) {
    let existing = document.as_array().cloned().unwrap_or_default();
    let mut used = vec![false; existing.len()];
    let mut items = Vec::with_capacity(settings.sidebar_items.len());

    for item in &settings.sidebar_items {
        let known = serde_json::to_value(item).expect("SidebarLocation serialization cannot fail");
        let matching = existing.iter().enumerate().find_map(|(index, value)| {
            (!used[index]
                && serde_json::from_value::<SidebarLocation>(value.clone())
                    .ok()
                    .as_ref()
                    == Some(item))
            .then_some(index)
        });
        if let Some(index) = matching {
            used[index] = true;
            let mut value = existing[index].clone();
            merge_known_value(&mut value, &known);
            items.push(value);
        } else {
            items.push(known);
        }
    }

    *document = Value::Array(items);
}

fn merge_known_value(document: &mut Value, known: &Value) {
    match (document, known) {
        (Value::Object(document), Value::Object(known)) => {
            for (key, value) in known {
                merge_known_value(document.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (document, known) => *document = known.clone(),
    }
}

fn sort_json_objects(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for value in object.values_mut() {
                sort_json_objects(value);
            }
            object.sort_keys();
        }
        Value::Array(values) => {
            for value in values {
                sort_json_objects(value);
            }
        }
        _ => {}
    }
}

fn default_sidebar_width() -> u32 {
    SIDEBAR_DEFAULT_WIDTH
}

fn default_date_format() -> String {
    DEFAULT_DATE_FORMAT.to_owned()
}

fn deserialize_sidebar_width<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    u32::deserialize(deserializer).map(normalized_sidebar_width)
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
        assert_eq!(settings.date_format, DEFAULT_DATE_FORMAT);
        assert!(settings.show_file_name_extensions);
        assert!(!settings.focus_new_tab_immediately);
        assert!(settings.resolve_icons);
        assert_eq!(settings.startup_location, StartupLocation::Downloads);
        assert_eq!(settings.sidebar_width, SIDEBAR_DEFAULT_WIDTH);
        assert_eq!(settings.sidebar_items.len(), 4);
    }

    #[test]
    fn settings_state_exposes_configured_settings_path() {
        let path = PathBuf::from("configured").join(SETTINGS_FILE_NAME);
        let state = SettingsState {
            value: ExplorerSettings::default(),
            document: settings_document(&ExplorerSettings::default()),
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
        assert_eq!(settings.date_format, DEFAULT_DATE_FORMAT);
        assert!(settings.show_file_name_extensions);
        assert!(!settings.focus_new_tab_immediately);
        assert!(settings.resolve_icons);
        assert_eq!(settings.sidebar_width, SIDEBAR_DEFAULT_WIDTH);
        assert_eq!(settings.sidebar_items.len(), 4);
    }

    #[test]
    fn sidebar_width_is_normalized_from_settings() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"sidebar_width":50}"#).expect("deserialize settings");

        assert_eq!(settings.sidebar_width, SIDEBAR_MIN_WIDTH);
        assert_eq!(normalized_sidebar_width(99), SIDEBAR_MIN_WIDTH);
        assert_eq!(normalized_sidebar_width(100), SIDEBAR_MIN_WIDTH);
        assert_eq!(normalized_sidebar_width(250), 250);
    }

    #[gpui::test]
    fn set_sidebar_width_persists_clamped_value(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("sidebar-width").join(SETTINGS_FILE_NAME);
        cx.set_global(SettingsState {
            value: ExplorerSettings::default(),
            document: settings_document(&ExplorerSettings::default()),
            path: path.clone(),
            _watcher: None,
        });

        let sidebar_width = cx.update(|cx| {
            set_sidebar_width(50, cx);
            cx.global::<SettingsState>().value.sidebar_width
        });

        assert_eq!(sidebar_width, SIDEBAR_MIN_WIDTH);
        assert_eq!(
            load_settings_from_path(&path).unwrap().sidebar_width,
            SIDEBAR_MIN_WIDTH
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
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
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\n  \"focus_new_tab_immediately\": false")
        );
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\n  \"resolve_icons\": true")
        );
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\n  \"date_format\": \"%Y/%m/%d %H:%M\"")
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn invalid_date_format_is_rejected() {
        let dir = unique_temp_dir("invalid-date-format");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILE_NAME);
        fs::write(&path, r#"{"date_format":"%Q"}"#).unwrap();

        assert!(load_settings_from_path(&path).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_and_literal_date_formats_are_valid() {
        for date_format in ["", "Modified today"] {
            let settings = ExplorerSettings {
                date_format: date_format.to_owned(),
                ..ExplorerSettings::default()
            };
            assert!(validate_settings(&settings).is_ok());
        }
    }

    #[test]
    fn missing_settings_are_created_with_defaults() {
        let path = unique_temp_dir("create").join(SETTINGS_FILE_NAME);
        let loaded = load_or_create_settings(&path);
        assert_eq!(loaded.value, ExplorerSettings::default());
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

        assert_eq!(
            load_or_create_settings(&path).value,
            ExplorerSettings::default()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ invalid");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn live_reload_recreates_deleted_file_and_rejects_malformed_edits() {
        let path = unique_temp_dir("live-reload").join(SETTINGS_FILE_NAME);
        let defaults = load_settings_after_change(&path).expect("recreate deleted settings");
        assert_eq!(defaults.value, ExplorerSettings::default());
        assert_eq!(load_settings_from_path(&path).unwrap(), defaults.value);

        fs::write(&path, "{ malformed").unwrap();
        assert!(load_settings_after_change(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ malformed");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn valid_partial_settings_are_completed_sorted_and_preserve_unknown_fields() {
        let path = unique_temp_dir("normalize").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"show_hidden_files":true,"future_option":{"z":1,"a":2}}"#,
        )
        .unwrap();

        let loaded = load_settings_document_from_path(&path).unwrap();
        assert!(loaded.value.show_hidden_files);

        let normalized = fs::read_to_string(&path).unwrap();
        let document: Value = serde_json::from_str(&normalized).unwrap();
        let object = document.as_object().unwrap();
        assert_eq!(
            object.len(),
            settings_document(&loaded.value).as_object().unwrap().len() + 1
        );
        assert_eq!(object["future_option"]["a"], 2);
        assert_eq!(object["future_option"]["z"], 1);
        assert!(
            normalized.find("\"date_format\"").unwrap()
                < normalized.find("\"future_option\"").unwrap()
        );
        assert!(normalized.find("\"a\"").unwrap() < normalized.find("\"z\"").unwrap());
        assert!(
            normalized.find("\"sidebar_items\"").unwrap()
                < normalized.find("\"startup_location\"").unwrap()
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[gpui::test]
    fn app_setting_updates_preserve_unknown_fields(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("preserve-unknown").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"future_option":{"z":1,"a":2},"show_hidden_files":false}"#,
        )
        .unwrap();
        let loaded = load_settings_document_from_path(&path).unwrap();
        cx.set_global(SettingsState {
            value: loaded.value,
            document: loaded.document,
            path: path.clone(),
            _watcher: None,
        });

        cx.update(|cx| set_show_hidden_files(true, cx));

        let document: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(document["future_option"]["a"], 2);
        assert_eq!(document["future_option"]["z"], 1);
        assert_eq!(document["show_hidden_files"], true);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn sidebar_edits_keep_unknown_fields_attached_to_remaining_items() {
        let mut document: Value = serde_json::from_str(
            r#"{"sidebar_items":[{"kind":"home","note":"home"},{"kind":"downloads","note":"downloads"}]}"#,
        )
        .unwrap();
        let mut settings: ExplorerSettings = serde_json::from_value(document.clone()).unwrap();

        assert_eq!(
            reorder_sidebar_item_in_settings(1, 0, true, &mut settings),
            Some(0)
        );
        sync_settings_document(&mut document, &settings);
        assert_eq!(document["sidebar_items"][0]["note"], "downloads");
        assert_eq!(document["sidebar_items"][1]["note"], "home");

        assert_eq!(
            unpin_sidebar_item_in_settings(1, &mut settings),
            Some(SidebarLocation::Home)
        );
        sync_settings_document(&mut document, &settings);
        assert_eq!(document["sidebar_items"].as_array().unwrap().len(), 1);
        assert_eq!(document["sidebar_items"][0]["note"], "downloads");
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
