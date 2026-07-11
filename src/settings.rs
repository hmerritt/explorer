use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use gpui::{App, BorrowAppContext, Font, Global, SharedString, font};
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeMap};
use serde_json::Value;

pub(crate) const APP_ID: &str = "com.hmerritt.explorer";
pub(crate) const DEFAULT_DATE_FORMAT: &str = "%Y/%m/%d %H:%M";
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) const DEFAULT_FILESYSTEM_NAME: &str = "Filesystem";
pub(crate) const DEFAULT_FONT: &str = "default";
pub(crate) const DEFAULT_CACHE_CLEANUP_INTERVAL_DAYS: u32 = 30;
const SYSTEM_UI_FONT: &str = ".SystemUIFont";
const LINUX_CONFIG_DIR_NAME: &str = "explorer";
const SETTINGS_FILE_NAME: &str = "settings.json";
const SETTINGS_REFRESH_INTERVAL: Duration = Duration::from_millis(150);
const SETTINGS_JSON_INDENT: usize = 2;
const SETTINGS_JSON_MAX_WIDTH: usize = 120;
pub(crate) const SIDEBAR_DEFAULT_WIDTH: u32 = 225;
pub(crate) const SIDEBAR_MIN_WIDTH: u32 = 100;
pub(crate) const FILE_COLUMN_MIN_WIDTH: u32 = 48;
const WINDOWS_TERMINAL_ICON_URL: &str = "https://raw.githubusercontent.com/microsoft/terminal/9853bc96076e811cef5eab4469095fc9be58201e/res/terminal/images/Square44x44Logo.targetsize-48.png";
const CMUX_ICON_URL: &str = "https://cmux.com/brand/app-icon-light.png";
const GHOSTTY_ICON_URL: &str = "https://ghostty.org/favicon-32.png";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigPlatform {
    MacOS,
    Linux,
    Windows,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum LegacySidebarLocation {
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(untagged)]
enum SidebarItemSetting {
    Path(PathBuf),
    Legacy(LegacySidebarLocation),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveHideKind {
    Wsl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarGroupKind {
    Pinned,
    Drives,
    Wsl,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressSlash {
    #[default]
    Forward,
    Back,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct ExplorerSettings {
    pub app: AppSettings,
    pub contextmenu: ContextMenuSettings,
    pub sidebar: SidebarSettings,
    pub tabs: TabSettings,
    pub view: ViewSettings,
}

impl Serialize for ExplorerSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(5))?;
        map.serialize_entry("app", &SerializableAppSettings::new(self))?;
        map.serialize_entry("contextmenu", &self.contextmenu)?;
        map.serialize_entry("sidebar", &SerializableSidebarSettings::new(self))?;
        map.serialize_entry("tabs", &self.tabs)?;
        map.serialize_entry("view", &self.view)?;
        map.end()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextMenuSettings {
    pub items: Vec<CustomContextMenuItem>,
}

impl Default for ContextMenuSettings {
    fn default() -> Self {
        Self {
            items: default_context_menu_items(),
        }
    }
}

impl Serialize for ContextMenuSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.items.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ContextMenuSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::Array(_) => Ok(Self {
                items: serde_json::from_value(value).map_err(de::Error::custom)?,
            }),
            Value::Object(mut object) => {
                let mut items: Vec<CustomContextMenuItem> = object
                    .remove("file_folder")
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(de::Error::custom)?
                    .unwrap_or_default();
                let mut directory_items = object
                    .remove("directory")
                    .map(serde_json::from_value::<Vec<CustomContextMenuItem>>)
                    .transpose()
                    .map_err(de::Error::custom)?
                    .unwrap_or_default();
                add_directory_only_to_context_menu_items(&mut directory_items);
                items.extend(directory_items);
                Ok(Self { items })
            }
            Value::Null => Ok(Self::default()),
            _ => Err(de::Error::custom(
                "contextmenu must be an array of items or a legacy object",
            )),
        }
    }
}

struct SerializableAppSettings<'a> {
    settings: &'a AppSettings,
    slash: AddressSlash,
}

impl<'a> SerializableAppSettings<'a> {
    fn new(settings: &'a ExplorerSettings) -> Self {
        Self {
            settings: &settings.app,
            slash: settings_address_slash(settings),
        }
    }

    fn with_slash(settings: &'a AppSettings, slash: AddressSlash) -> Self {
        Self { settings, slash }
    }
}

impl Serialize for SerializableAppSettings<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry(
            "cache_cleanup_interval_days",
            &self.settings.cache_cleanup_interval_days,
        )?;
        map.serialize_entry("new_window_behaviour", &self.settings.new_window_behaviour)?;
        map.serialize_entry(
            "start",
            &format_configured_path(&self.settings.start, self.slash),
        )?;
        map.end()
    }
}

struct SerializableSidebarSettings<'a> {
    settings: &'a SidebarSettings,
    slash: AddressSlash,
}

impl<'a> SerializableSidebarSettings<'a> {
    fn new(settings: &'a ExplorerSettings) -> Self {
        Self {
            settings: &settings.sidebar,
            slash: settings_address_slash(settings),
        }
    }
}

impl Serialize for SerializableSidebarSettings<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("expanded_groups", &self.settings.expanded_groups)?;
        map.serialize_entry("hide", &self.settings.hide)?;
        map.serialize_entry(
            "items",
            &SerializableSidebarItems {
                items: &self.settings.items,
                slash: self.slash,
            },
        )?;
        map.serialize_entry("width", &self.settings.width)?;
        map.end()
    }
}

struct SerializableSidebarItems<'a> {
    items: &'a [PathBuf],
    slash: AddressSlash,
}

impl Serialize for SerializableSidebarItems<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let items = self
            .items
            .iter()
            .map(|path| format_configured_path(path, self.slash))
            .collect::<Vec<_>>();
        items.serialize(serializer)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CustomContextMenuItem {
    Action {
        label: String,
        action: ContextMenuAction,
        icon: Option<PathBuf>,
        only: Vec<String>,
    },
    Item {
        label: String,
        exe: PathBuf,
        icon: Option<PathBuf>,
        args: Vec<String>,
        only: Vec<String>,
    },
    Submenu {
        label: String,
        icon: Option<PathBuf>,
        items: Vec<CustomContextMenuItem>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMenuAction {
    Compress,
}

fn compress_context_menu_item() -> CustomContextMenuItem {
    CustomContextMenuItem::Action {
        label: "Compress".to_owned(),
        action: ContextMenuAction::Compress,
        icon: None,
        only: vec!["*file".to_owned(), "*folder".to_owned()],
    }
}

fn default_context_menu_items() -> Vec<CustomContextMenuItem> {
    default_context_menu_items_for(
        current_config_platform(),
        |executable| resolve_context_menu_executable(Path::new(executable)).is_some(),
        macos_application_is_available,
    )
}

fn default_context_menu_items_for(
    platform: ConfigPlatform,
    executable_available: impl Fn(&str) -> bool,
    macos_application_available: impl Fn(&str) -> bool,
) -> Vec<CustomContextMenuItem> {
    let only = || vec!["*directory".to_owned(), "*folders".to_owned()];
    let terminal = match platform {
        ConfigPlatform::Windows => {
            return vec![
                if executable_available("7zG") {
                    CustomContextMenuItem::Item {
                        label: "Add to archive...".to_owned(),
                        exe: PathBuf::from("7zG"),
                        icon: None,
                        args: vec![
                            "a".to_owned(),
                            "-ad".to_owned(),
                            "-saa".to_owned(),
                            "{path}".to_owned(),
                            "{paths}".to_owned(),
                        ],
                        only: vec!["*file".to_owned(), "*folder".to_owned()],
                    }
                } else {
                    compress_context_menu_item()
                },
                CustomContextMenuItem::Item {
                    label: "Terminal".to_owned(),
                    exe: PathBuf::from("wt"),
                    icon: Some(PathBuf::from(WINDOWS_TERMINAL_ICON_URL)),
                    args: vec!["-d".to_owned(), "{paths}".to_owned()],
                    only: only(),
                },
            ];
        }
        ConfigPlatform::MacOS => {
            let (label, application, icon) = if macos_application_available("cmux") {
                ("cmux", "cmux", Some(CMUX_ICON_URL))
            } else if macos_application_available("Ghostty") {
                ("Ghostty", "Ghostty", Some(GHOSTTY_ICON_URL))
            } else {
                ("Terminal", "Terminal", None)
            };
            CustomContextMenuItem::Item {
                label: label.to_owned(),
                exe: PathBuf::from("/usr/bin/open"),
                icon: icon.map(PathBuf::from),
                args: vec!["-a".to_owned(), application.to_owned(), "{path}".to_owned()],
                only: only(),
            }
        }
        ConfigPlatform::Linux => {
            if executable_available("ghostty") {
                CustomContextMenuItem::Item {
                    label: "Ghostty".to_owned(),
                    exe: PathBuf::from("ghostty"),
                    icon: Some(PathBuf::from(GHOSTTY_ICON_URL)),
                    args: vec!["--working-directory".to_owned(), "{path}".to_owned()],
                    only: only(),
                }
            } else if executable_available("xdg-terminal-exec") {
                CustomContextMenuItem::Item {
                    label: "Terminal".to_owned(),
                    exe: PathBuf::from("xdg-terminal-exec"),
                    icon: None,
                    args: vec!["{cwd}".to_owned()],
                    only: only(),
                }
            } else if executable_available("x-terminal-emulator") {
                CustomContextMenuItem::Item {
                    label: "Terminal".to_owned(),
                    exe: PathBuf::from("x-terminal-emulator"),
                    icon: None,
                    args: vec!["{cwd}".to_owned()],
                    only: only(),
                }
            } else {
                return vec![compress_context_menu_item()];
            }
        }
    };
    vec![compress_context_menu_item(), terminal]
}

#[cfg(target_os = "macos")]
fn macos_application_is_available(name: &str) -> bool {
    use cocoa::{
        base::{id, nil},
        foundation::NSString,
    };
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let available = (|| {
            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            if workspace == nil {
                return false;
            }
            let name = NSString::alloc(nil).init_str(name);
            let _: id = msg_send![name, autorelease];
            let path: id = msg_send![workspace, fullPathForApplication: name];
            path != nil
        })();
        let _: () = msg_send![pool, drain];
        available
    }
}

#[cfg(not(target_os = "macos"))]
fn macos_application_is_available(_name: &str) -> bool {
    false
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContextMenuConfiguredIcon {
    Image(PathBuf),
    NativePath(PathBuf),
    Url(String),
}

impl Serialize for CustomContextMenuItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Action {
                label,
                action,
                icon,
                only,
            } => {
                let mut length = 2;
                if icon.is_some() {
                    length += 1;
                }
                if !only.is_empty() {
                    length += 1;
                }
                let mut map = serializer.serialize_map(Some(length))?;
                map.serialize_entry("label", label)?;
                map.serialize_entry("action", action)?;
                if let Some(icon) = icon {
                    map.serialize_entry("icon", icon)?;
                }
                if !only.is_empty() {
                    map.serialize_entry("only", only)?;
                }
                map.end()
            }
            Self::Item {
                label,
                exe,
                icon,
                args,
                only,
            } => {
                let mut length = 2;
                if icon.is_some() {
                    length += 1;
                }
                if !args.is_empty() {
                    length += 1;
                }
                if !only.is_empty() {
                    length += 1;
                }
                let mut map = serializer.serialize_map(Some(length))?;
                map.serialize_entry("label", label)?;
                map.serialize_entry("exe", exe)?;
                if let Some(icon) = icon {
                    map.serialize_entry("icon", icon)?;
                }
                if !args.is_empty() {
                    map.serialize_entry("args", args)?;
                }
                if !only.is_empty() {
                    map.serialize_entry("only", only)?;
                }
                map.end()
            }
            Self::Submenu { label, icon, items } => {
                let mut map = serializer.serialize_map(Some(if icon.is_some() { 3 } else { 2 }))?;
                map.serialize_entry("label", label)?;
                if let Some(icon) = icon {
                    map.serialize_entry("icon", icon)?;
                }
                map.serialize_entry("items", items)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for CustomContextMenuItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| de::Error::custom("contextmenu items must be objects"))?;
        let label = object
            .remove("label")
            .ok_or_else(|| de::Error::missing_field("label"))
            .and_then(|value| serde_json::from_value(value).map_err(de::Error::custom))?;
        let icon = object
            .remove("icon")
            .map(serde_json::from_value)
            .transpose()
            .map_err(de::Error::custom)?;

        let action = object.remove("action");
        let items = object.remove("items");
        if action.is_some() && items.is_some() {
            return Err(de::Error::custom(
                "contextmenu items cannot contain both action and items",
            ));
        }
        if let Some(items) = items {
            let items = serde_json::from_value(items).map_err(de::Error::custom)?;
            return Ok(Self::Submenu { label, icon, items });
        }

        if let Some(action) = action {
            if object.contains_key("exe")
                || object.contains_key("executable")
                || object.contains_key("args")
            {
                return Err(de::Error::custom(
                    "contextmenu action items cannot contain exe or args",
                ));
            }
            let action = serde_json::from_value(action).map_err(de::Error::custom)?;
            let only = object
                .remove("only")
                .map(serde_json::from_value)
                .transpose()
                .map_err(de::Error::custom)?
                .unwrap_or_default();
            return Ok(Self::Action {
                label,
                action,
                icon,
                only,
            });
        }

        let exe = object
            .remove("exe")
            .or_else(|| object.remove("executable"))
            .ok_or_else(|| de::Error::missing_field("exe"))
            .and_then(|value| serde_json::from_value(value).map_err(de::Error::custom))?;
        let only = object
            .remove("only")
            .map(serde_json::from_value)
            .transpose()
            .map_err(de::Error::custom)?
            .unwrap_or_default();
        let args = object
            .remove("args")
            .map(deserialize_context_menu_args)
            .transpose()
            .map_err(de::Error::custom)?
            .unwrap_or_default();

        Ok(Self::Item {
            label,
            exe,
            icon,
            args,
            only,
        })
    }
}

fn deserialize_context_menu_args(value: Value) -> Result<Vec<String>, String> {
    match value {
        Value::Array(_) => serde_json::from_value(value).map_err(|error| error.to_string()),
        Value::String(args) => shlex::split(&args)
            .ok_or_else(|| "contextmenu args strings must use valid shell quoting".to_owned()),
        _ => Err("contextmenu args must be an array or string".to_owned()),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct AppSettings {
    #[serde(
        default = "default_cache_cleanup_interval_days",
        deserialize_with = "deserialize_cache_cleanup_interval_days"
    )]
    pub cache_cleanup_interval_days: u32,
    #[serde(default)]
    pub new_window_behaviour: NewWindowBehaviour,
    #[serde(
        default = "default_app_start_path",
        deserialize_with = "deserialize_app_start_path"
    )]
    pub start: PathBuf,
}

impl Serialize for AppSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializableAppSettings::with_slash(self, AddressSlash::Forward).serialize(serializer)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct SidebarSettings {
    #[serde(
        default = "default_sidebar_expanded_groups",
        deserialize_with = "deserialize_sidebar_expanded_groups"
    )]
    pub expanded_groups: Vec<SidebarGroupKind>,
    #[serde(default, deserialize_with = "deserialize_drive_hide_kinds")]
    pub hide: Vec<DriveHideKind>,
    #[serde(
        default = "default_sidebar_items",
        deserialize_with = "deserialize_sidebar_items"
    )]
    pub items: Vec<PathBuf>,
    #[serde(
        default = "default_sidebar_width",
        deserialize_with = "deserialize_sidebar_width"
    )]
    pub width: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct TabSettings {
    pub focus_new: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ViewSettings {
    #[cfg(target_os = "windows")]
    #[serde(default)]
    pub address_slash: AddressSlash,
    #[serde(default = "default_date_format")]
    pub date_format: String,
    #[serde(
        default = "default_file_columns",
        deserialize_with = "deserialize_file_column_settings"
    )]
    pub file_columns: FileColumnSettings,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[serde(default = "default_filesystem_name")]
    pub filesystem_name: String,
    #[serde(default = "default_font")]
    pub font: String,
    pub mode: FileViewMode,
    #[serde(default = "default_media_view_mode")]
    pub mode_media: FileViewMode,
    #[serde(default)]
    pub remote_mode_media: FileViewMode,
    #[serde(default)]
    pub remote_thumbnails: bool,
    pub native_icons: bool,
    pub show_extensions: bool,
    pub show_folder_sizes: bool,
    #[serde(default = "default_show_dotfiles")]
    pub show_dotfiles: bool,
    pub show_hidden: bool,
    #[serde(
        default = "default_file_sort",
        deserialize_with = "deserialize_file_sort_settings"
    )]
    pub sort: FileSortSettings,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NewWindowBehaviour {
    Open,
    #[default]
    Focus,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileViewMode {
    #[default]
    Details,
    LargeIcons,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileColumnKind {
    DateModified,
    Type,
    Size,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSortColumn {
    #[default]
    Name,
    DateModified,
    Type,
    Size,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileSortSettings {
    pub column: FileSortColumn,
    pub direction: SortDirection,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FileColumnSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_width: Option<u32>,
    pub order: Vec<FileColumnKind>,
    pub widths: BTreeMap<FileColumnKind, u32>,
}

impl Default for ExplorerSettings {
    fn default() -> Self {
        Self {
            app: AppSettings::default(),
            contextmenu: ContextMenuSettings::default(),
            sidebar: SidebarSettings::default(),
            tabs: TabSettings::default(),
            view: ViewSettings::default(),
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            cache_cleanup_interval_days: DEFAULT_CACHE_CLEANUP_INTERVAL_DAYS,
            new_window_behaviour: NewWindowBehaviour::Focus,
            start: default_app_start_path(),
        }
    }
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            expanded_groups: default_sidebar_expanded_groups(),
            hide: Vec::new(),
            items: default_sidebar_items(),
            width: SIDEBAR_DEFAULT_WIDTH,
        }
    }
}

impl Default for TabSettings {
    fn default() -> Self {
        Self { focus_new: false }
    }
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            #[cfg(target_os = "windows")]
            address_slash: AddressSlash::Forward,
            date_format: default_date_format(),
            file_columns: default_file_columns(),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            filesystem_name: default_filesystem_name(),
            font: default_font(),
            mode: FileViewMode::Details,
            mode_media: default_media_view_mode(),
            remote_mode_media: FileViewMode::Details,
            remote_thumbnails: false,
            native_icons: true,
            show_extensions: true,
            show_folder_sizes: false,
            show_dotfiles: true,
            show_hidden: false,
            sort: default_file_sort(),
        }
    }
}

impl Default for FileColumnSettings {
    fn default() -> Self {
        default_file_columns()
    }
}

impl Default for FileSortSettings {
    fn default() -> Self {
        default_file_sort()
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
        expand_configured_path(&self.value.app.start)
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

impl LegacySidebarLocation {
    fn configured_path(self) -> Option<PathBuf> {
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
            Self::Custom { path, .. } => Some(path),
        }
    }
}

impl CustomContextMenuItem {
    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Action { label, .. } | Self::Item { label, .. } | Self::Submenu { label, .. } => {
                label
            }
        }
    }

    pub(crate) fn resolved_executable(&self) -> Option<PathBuf> {
        match self {
            Self::Item { exe, .. } => resolve_context_menu_executable(exe),
            Self::Action { .. } | Self::Submenu { .. } => None,
        }
    }

    pub(crate) fn resolved_executable_icon_path(&self, executable: &Path) -> PathBuf {
        match self {
            Self::Item { .. } => context_menu_executable_icon_path(executable),
            Self::Action { .. } | Self::Submenu { .. } => executable.to_path_buf(),
        }
    }

    pub(crate) fn resolved_icon(&self) -> Option<ContextMenuConfiguredIcon> {
        match self {
            Self::Action { icon, .. } | Self::Item { icon, .. } | Self::Submenu { icon, .. } => {
                resolve_context_menu_icon(icon.as_deref())
            }
        }
    }
}

fn add_directory_only_to_context_menu_items(items: &mut [CustomContextMenuItem]) {
    for item in items {
        add_directory_only_to_context_menu_item(item);
    }
}

fn add_directory_only_to_context_menu_item(item: &mut CustomContextMenuItem) {
    match item {
        CustomContextMenuItem::Action { only, .. } | CustomContextMenuItem::Item { only, .. } => {
            add_directory_only_filter(only)
        }
        CustomContextMenuItem::Submenu { items, .. } => {
            add_directory_only_to_context_menu_items(items);
        }
    }
}

fn add_directory_only_filter(only: &mut Vec<String>) {
    if !only
        .iter()
        .any(|value| value.trim().eq_ignore_ascii_case("*directory"))
    {
        only.push("*directory".to_owned());
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

pub(crate) fn set_show_hidden(value: bool, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| settings.view.show_hidden = value);
}

pub(crate) fn set_show_dotfiles(value: bool, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| settings.view.show_dotfiles = value);
}

pub(crate) fn set_show_folder_sizes(value: bool, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| settings.view.show_folder_sizes = value);
}

pub(crate) fn set_show_extensions(value: bool, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| settings.view.show_extensions = value);
}

pub(crate) fn set_view_mode(mode: FileViewMode, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| settings.view.mode = mode);
}

pub(crate) fn set_file_sort(sort: FileSortSettings, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| settings.view.sort = sort);
}

pub(crate) fn set_sidebar_width(value: u32, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| {
        settings.sidebar.width = normalized_sidebar_width(value);
    });
}

pub(crate) fn set_sidebar_group_expanded(
    kind: SidebarGroupKind,
    expanded: bool,
    cx: &mut impl BorrowAppContext,
) -> bool {
    update_settings(cx, |settings| {
        set_sidebar_group_expanded_in_settings(kind, expanded, settings)
    })
}

pub(crate) fn set_file_column_width(
    kind: FileColumnKind,
    value: u32,
    cx: &mut impl BorrowAppContext,
) {
    update_settings(cx, |settings| {
        settings
            .view
            .file_columns
            .widths
            .insert(kind, normalized_file_column_width(value));
        normalize_file_column_settings(&mut settings.view.file_columns);
    });
}

pub(crate) fn set_name_column_width(value: u32, cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| {
        settings.view.file_columns.name_width = Some(normalized_name_column_width(value));
        normalize_file_column_settings(&mut settings.view.file_columns);
    });
}

pub(crate) fn clear_name_column_width(cx: &mut impl BorrowAppContext) {
    update_settings(cx, |settings| {
        settings.view.file_columns.name_width = None;
        normalize_file_column_settings(&mut settings.view.file_columns);
    });
}

pub(crate) fn reorder_file_column(
    dragged: FileColumnKind,
    target: FileColumnKind,
    before: bool,
    cx: &mut impl BorrowAppContext,
) -> bool {
    update_settings(cx, |settings| {
        reorder_file_column_in_settings(&mut settings.view.file_columns, dragged, target, before)
    })
}

pub(crate) fn normalized_sidebar_width(value: u32) -> u32 {
    value.max(SIDEBAR_MIN_WIDTH)
}

pub(crate) fn normalized_file_column_width(value: u32) -> u32 {
    value.max(FILE_COLUMN_MIN_WIDTH)
}

pub(crate) fn normalized_name_column_width(value: u32) -> u32 {
    value.max(crate::explorer::constants::COLUMN_NAME_MIN_WIDTH as u32)
}

pub(crate) fn normalized_cache_cleanup_interval_days(value: u32) -> u32 {
    value.max(1)
}

pub(crate) fn can_pin_sidebar_path(path: &Path, settings: &ExplorerSettings) -> bool {
    path.is_dir()
        && !settings
            .sidebar
            .items
            .iter()
            .filter_map(|path| expand_configured_path(path))
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
) -> Option<PathBuf> {
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

fn reorder_file_column_in_settings(
    settings: &mut FileColumnSettings,
    dragged: FileColumnKind,
    target: FileColumnKind,
    before: bool,
) -> bool {
    normalize_file_column_settings(settings);
    if dragged == target {
        return false;
    }

    let Some(dragged_index) = settings.order.iter().position(|kind| *kind == dragged) else {
        return false;
    };
    let Some(mut target_index) = settings.order.iter().position(|kind| *kind == target) else {
        return false;
    };
    if dragged_index < target_index {
        target_index -= 1;
    }

    let insert_index = if before {
        target_index
    } else {
        target_index + 1
    };
    let dragged = settings.order.remove(dragged_index);
    settings.order.insert(insert_index, dragged);
    true
}

fn pin_sidebar_path_in_settings(
    path: PathBuf,
    insertion_index: usize,
    settings: &mut ExplorerSettings,
) -> bool {
    if !can_pin_sidebar_path(&path, settings) {
        return false;
    }
    let insertion_index = insertion_index.min(settings.sidebar.items.len());
    settings.sidebar.items.insert(insertion_index, path);
    true
}

fn reorder_sidebar_item_in_settings(
    source_index: usize,
    target_index: usize,
    before: bool,
    settings: &mut ExplorerSettings,
) -> Option<usize> {
    let new_index = sidebar_reorder_index(
        settings.sidebar.items.len(),
        source_index,
        target_index,
        before,
    )?;
    let item = settings.sidebar.items.remove(source_index);
    settings.sidebar.items.insert(new_index, item);
    Some(new_index)
}

fn unpin_sidebar_item_in_settings(
    configured_index: usize,
    settings: &mut ExplorerSettings,
) -> Option<PathBuf> {
    (configured_index < settings.sidebar.items.len())
        .then(|| settings.sidebar.items.remove(configured_index))
}

fn set_sidebar_group_expanded_in_settings(
    kind: SidebarGroupKind,
    expanded: bool,
    settings: &mut ExplorerSettings,
) -> bool {
    let groups = &mut settings.sidebar.expanded_groups;
    if expanded {
        if groups.contains(&kind) {
            false
        } else {
            groups.push(kind);
            true
        }
    } else {
        let len = groups.len();
        groups.retain(|group| *group != kind);
        groups.len() != len
    }
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
    if source.trim().is_empty() {
        let value = ExplorerSettings::default();
        let mut document = settings_document(&value);
        save_document_to_path(path, &mut document)?;
        return Ok(LoadedSettings { value, document });
    }

    let mut document = serde_json::from_str::<Value>(&source).map_err(io::Error::other)?;
    let value =
        serde_json::from_value::<ExplorerSettings>(document.clone()).map_err(io::Error::other)?;
    validate_settings(&value)?;

    if !document.is_object() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "settings must be a JSON object",
        ));
    }
    sync_settings_document(&mut document, &value);

    sort_json_objects(&mut document);
    let normalized = format_settings_document(&document).map_err(io::Error::other)?;
    if source != normalized
        && let Err(error) = fs::write(path, normalized)
    {
        eprintln!("Unable to normalize Explorer settings: {error}");
    }

    Ok(LoadedSettings { value, document })
}

fn validate_settings(settings: &ExplorerSettings) -> io::Result<()> {
    if settings.view.font.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "font must not be empty",
        ));
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if settings.view.filesystem_name.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "filesystem_name must not be empty",
        ));
    }
    chrono::format::StrftimeItems::new(&settings.view.date_format)
        .parse()
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid date_format: {error}"),
            )
        })?;
    for path in &settings.sidebar.items {
        validate_configured_path(path)?;
    }
    validate_configured_path(&settings.app.start)?;
    validate_custom_context_menu_items(&settings.contextmenu.items)?;
    Ok(())
}

fn validate_custom_context_menu_items(items: &[CustomContextMenuItem]) -> io::Result<()> {
    for item in items {
        if item.label().trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "contextmenu labels must not be empty",
            ));
        }

        match item {
            CustomContextMenuItem::Action { icon, only, .. } => {
                validate_context_menu_icon(icon.as_deref())?;
                validate_context_menu_only_extensions(only)?;
            }
            CustomContextMenuItem::Item {
                exe, icon, only, ..
            } => {
                validate_context_menu_executable(exe)?;
                validate_context_menu_icon(icon.as_deref())?;
                validate_context_menu_only_extensions(only)?;
            }
            CustomContextMenuItem::Submenu { icon, items, .. } => {
                validate_context_menu_icon(icon.as_deref())?;
                validate_custom_context_menu_items(items)?;
            }
        }
    }
    Ok(())
}

fn validate_configured_path(path: &Path) -> io::Result<()> {
    if configured_path_shape_is_valid(path) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "configured paths must be absolute or begin with ~/ or ~\\: {}",
                path.display()
            ),
        ))
    }
}

fn validate_context_menu_executable(path: &Path) -> io::Result<()> {
    if path.is_absolute() || is_tilde_path(path) || is_path_executable_name(path) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "contextmenu executables must be absolute, begin with ~/, or be an executable name from PATH: {}",
                path.display()
            ),
        ))
    }
}

fn validate_context_menu_icon(path: Option<&Path>) -> io::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };

    if context_menu_icon_path_is_url(path) {
        return Ok(());
    }

    if context_menu_icon_path_is_image(path) {
        return validate_configured_path(path);
    }

    validate_context_menu_executable(path).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "contextmenu icons must be absolute, begin with ~/, or be an executable name from PATH: {}",
                path.display()
            ),
        )
    })
}

fn validate_context_menu_only_extensions(extensions: &[String]) -> io::Result<()> {
    for extension in extensions {
        resolve_context_menu_only_filter(extension).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "contextmenu only values must be file extensions or known aliases: {extension}"
                ),
            )
        })?;
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContextMenuOnlyFilter {
    Extension(String),
    Alias(&'static [&'static str]),
    Directory,
    File,
    Folder,
}

pub(crate) fn resolve_context_menu_only_filter(extension: &str) -> Option<ContextMenuOnlyFilter> {
    let extension = extension.trim();
    if let Some(alias) = extension.strip_prefix('*') {
        match alias.to_ascii_lowercase().as_str() {
            "directory" => return Some(ContextMenuOnlyFilter::Directory),
            "file" | "files" => return Some(ContextMenuOnlyFilter::File),
            "folder" | "folders" => return Some(ContextMenuOnlyFilter::Folder),
            _ => {}
        }

        return context_menu_only_alias_extensions(alias).map(ContextMenuOnlyFilter::Alias);
    }

    let extension = extension.strip_prefix('.').unwrap_or(extension);
    (!extension.is_empty()
        && !extension.contains('/')
        && !extension.contains('\\')
        && !extension.contains('*'))
    .then(|| ContextMenuOnlyFilter::Extension(extension.to_ascii_lowercase()))
}

fn context_menu_only_alias_extensions(alias: &str) -> Option<&'static [&'static str]> {
    match alias.to_ascii_lowercase().as_str() {
        "audio" => Some(CONTEXT_MENU_AUDIO_EXTENSIONS),
        "image" | "photo" => Some(CONTEXT_MENU_IMAGE_EXTENSIONS),
        "video" => Some(CONTEXT_MENU_VIDEO_EXTENSIONS),
        _ => None,
    }
}

const CONTEXT_MENU_AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "wave", "flac", "aac", "m4a", "wma", "opus", "oga", "mid", "midi", "aif", "aiff",
    "aifc", "ape", "amr", "au", "snd", "ac3", "dts", "ra",
];

const CONTEXT_MENU_IMAGE_EXTENSIONS: &[&str] = &[
    "bmp", "gif", "jpg", "jpeg", "jpe", "jfif", "png", "apng", "webp", "tif", "tiff", "svg",
    "svgz", "heic", "heif", "avif", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "psd", "xcf",
];

const CONTEXT_MENU_VIDEO_EXTENSIONS: &[&str] = &[
    "webm", "mkv", "flv", "vob", "ogv", "ogg", "rrc", "gifv", "mng", "mov", "avi", "qt", "wmv",
    "yuv", "rm", "asf", "amv", "m2ts", "mp4", "m4p", "m4v", "mpg", "mp2", "mpeg", "mpe", "mpv",
    "svi", "3gp", "3g2", "mxf", "roq", "nsv", "f4v", "f4p", "f4a", "f4b",
];

pub(crate) fn expand_configured_path(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    if path == Path::new("~") {
        return crate::explorer::user_home_dir();
    }

    let text = path.to_str()?;
    let remainder = text
        .strip_prefix("~/")
        .or_else(|| text.strip_prefix(r"~\"))?;
    crate::explorer::user_home_dir().map(|home| home.join(remainder))
}

fn resolve_context_menu_executable(path: &Path) -> Option<PathBuf> {
    resolve_context_menu_executable_with(
        path,
        current_config_platform(),
        |name| env::var_os(name),
        |path| path.is_file(),
    )
}

fn resolve_context_menu_executable_with(
    path: &Path,
    platform: ConfigPlatform,
    mut env_var: impl FnMut(&str) -> Option<OsString>,
    mut is_file: impl FnMut(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(path) = expand_configured_path(path) {
        return is_file(&path).then_some(path);
    }
    if !is_path_executable_name(path) {
        return None;
    }

    let path_var = env_var("PATH")?;
    let pathext = (platform == ConfigPlatform::Windows)
        .then(|| windows_path_extensions(env_var("PATHEXT")))
        .unwrap_or_default();

    for directory in env::split_paths(&path_var) {
        let direct = directory.join(path);
        if is_file(&direct) {
            return Some(direct);
        }

        if platform == ConfigPlatform::Windows && path.extension().is_none() {
            for extension in &pathext {
                let candidate = directory.join(format!("{}{}", path.to_string_lossy(), extension));
                if is_file(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

fn resolve_context_menu_icon(path: Option<&Path>) -> Option<ContextMenuConfiguredIcon> {
    let path = path?;
    if let Some(url) = context_menu_icon_url(path) {
        return Some(ContextMenuConfiguredIcon::Url(url.to_owned()));
    }

    if context_menu_icon_path_is_image(path) {
        return expand_configured_path(path).map(ContextMenuConfiguredIcon::Image);
    }

    resolve_context_menu_executable(path)
        .map(|path| ContextMenuConfiguredIcon::NativePath(context_menu_executable_icon_path(&path)))
}

fn context_menu_executable_icon_path(executable: &Path) -> PathBuf {
    context_menu_executable_icon_path_with(
        executable,
        current_config_platform(),
        |path| fs::read_to_string(path),
        |path| path.is_file(),
    )
}

fn context_menu_executable_icon_path_with(
    executable: &Path,
    platform: ConfigPlatform,
    mut read_to_string: impl FnMut(&Path) -> io::Result<String>,
    mut is_file: impl FnMut(&Path) -> bool,
) -> PathBuf {
    if platform != ConfigPlatform::Windows {
        return executable.to_path_buf();
    }

    let shim_path = executable.with_extension("shim");
    let Some(target) = read_to_string(&shim_path)
        .ok()
        .and_then(|contents| scoop_shim_target_path(&contents))
        .filter(|target| is_file(target))
    else {
        return executable.to_path_buf();
    };

    target
}

fn context_menu_icon_path_is_image(path: &Path) -> bool {
    (path.is_absolute() || is_tilde_path(path))
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                ["png", "svg", "ico"]
                    .iter()
                    .any(|supported| extension.eq_ignore_ascii_case(supported))
            })
}

fn context_menu_icon_path_is_url(path: &Path) -> bool {
    context_menu_icon_url(path).is_some()
}

fn context_menu_icon_url(path: &Path) -> Option<&str> {
    let text = path.to_str()?.trim();
    (text.starts_with("https://") || text.starts_with("http://")).then_some(text)
}

fn scoop_shim_target_path(contents: &str) -> Option<PathBuf> {
    contents.lines().find_map(|line| {
        let line = line.trim();
        let value = line.strip_prefix("path")?.trim_start();
        let value = value.strip_prefix('=')?.trim_start();
        let value = value.strip_prefix('"')?;
        let (path, remainder) = value.split_once('"')?;
        remainder.trim().is_empty().then(|| PathBuf::from(path))
    })
}

fn is_path_executable_name(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    !text.is_empty() && text != "." && text != ".." && !text.contains('/') && !text.contains('\\')
}

fn windows_path_extensions(value: Option<OsString>) -> Vec<String> {
    value
        .and_then(|value| value.into_string().ok())
        .map(|value| {
            value
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(|extension| {
                    if extension.starts_with('.') {
                        extension.to_owned()
                    } else {
                        format!(".{extension}")
                    }
                })
                .collect()
        })
        .unwrap_or_else(|| {
            [".COM", ".EXE", ".BAT", ".CMD"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        })
}

fn is_tilde_path(path: &Path) -> bool {
    path == Path::new("~")
        || path.to_str().is_some_and(|text| {
            (text.starts_with("~/") || text.starts_with(r"~\")) && text.len() > 2
        })
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
    let json = format_settings_document(document).map_err(io::Error::other)?;
    fs::write(path, json)
}

fn format_settings_document(document: &Value) -> serde_json::Result<String> {
    let mut formatted = String::new();
    format_json_value(document, 0, true, 0, 0, &mut formatted)?;
    Ok(formatted)
}

fn format_json_value(
    value: &Value,
    depth: usize,
    force_expanded: bool,
    line_prefix_width: usize,
    line_suffix_width: usize,
    formatted: &mut String,
) -> serde_json::Result<()> {
    if !force_expanded && is_simple_json_value(value) {
        let compact = compact_json_value(value)?;
        if line_prefix_width + compact.chars().count() + line_suffix_width
            <= SETTINGS_JSON_MAX_WIDTH
        {
            formatted.push_str(&compact);
            return Ok(());
        }
    }

    match value {
        Value::Array(values) => format_json_array(values, depth, formatted),
        Value::Object(object) => format_json_object(object, depth, formatted),
        _ => {
            formatted.push_str(&compact_json_value(value)?);
            Ok(())
        }
    }
}

fn format_json_array(
    values: &[Value],
    depth: usize,
    formatted: &mut String,
) -> serde_json::Result<()> {
    if values.is_empty() {
        formatted.push_str("[]");
        return Ok(());
    }

    formatted.push('[');
    for (index, value) in values.iter().enumerate() {
        formatted.push('\n');
        push_json_indent(formatted, depth + 1);
        format_json_value(
            value,
            depth + 1,
            false,
            json_indent_width(depth + 1),
            trailing_comma_width(index, values.len()),
            formatted,
        )?;
        if index + 1 < values.len() {
            formatted.push(',');
        }
    }
    formatted.push('\n');
    push_json_indent(formatted, depth);
    formatted.push(']');
    Ok(())
}

fn format_json_object(
    object: &serde_json::Map<String, Value>,
    depth: usize,
    formatted: &mut String,
) -> serde_json::Result<()> {
    if object.is_empty() {
        formatted.push_str("{}");
        return Ok(());
    }

    formatted.push('{');
    for (index, (key, value)) in object.iter().enumerate() {
        formatted.push('\n');
        push_json_indent(formatted, depth + 1);
        let key = serde_json::to_string(key)?;
        formatted.push_str(&key);
        formatted.push_str(": ");
        format_json_value(
            value,
            depth + 1,
            false,
            json_indent_width(depth + 1) + key.chars().count() + 2,
            trailing_comma_width(index, object.len()),
            formatted,
        )?;
        if index + 1 < object.len() {
            formatted.push(',');
        }
    }
    formatted.push('\n');
    push_json_indent(formatted, depth);
    formatted.push('}');
    Ok(())
}

fn compact_json_value(value: &Value) -> serde_json::Result<String> {
    match value {
        Value::Array(values) => {
            let mut compact = String::from("[");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    compact.push_str(", ");
                }
                compact.push_str(&compact_json_value(value)?);
            }
            compact.push(']');
            Ok(compact)
        }
        Value::Object(object) => {
            let mut compact = String::from("{");
            for (index, (key, value)) in object.iter().enumerate() {
                if index > 0 {
                    compact.push_str(", ");
                }
                compact.push_str(&serde_json::to_string(key)?);
                compact.push_str(": ");
                compact.push_str(&compact_json_value(value)?);
            }
            compact.push('}');
            Ok(compact)
        }
        _ => serde_json::to_string(value),
    }
}

fn is_simple_json_value(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => true,
        Value::Array(values) => {
            values.is_empty()
                || values.iter().all(is_json_scalar)
                || values.iter().all(is_scalar_object)
        }
        Value::Object(object) => object.values().all(is_json_scalar),
    }
}

fn is_json_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn is_scalar_object(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.values().all(is_json_scalar))
}

fn push_json_indent(formatted: &mut String, depth: usize) {
    formatted.push_str(&" ".repeat(json_indent_width(depth)));
}

fn json_indent_width(depth: usize) -> usize {
    depth * SETTINGS_JSON_INDENT
}

fn trailing_comma_width(index: usize, len: usize) -> usize {
    usize::from(index + 1 < len)
}

fn settings_document(settings: &ExplorerSettings) -> Value {
    serde_json::to_value(settings).expect("ExplorerSettings serialization cannot fail")
}

fn sync_settings_document(document: &mut Value, settings: &ExplorerSettings) {
    *document = settings_document(settings);
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

fn settings_address_slash(settings: &ExplorerSettings) -> AddressSlash {
    #[cfg(target_os = "windows")]
    {
        settings.view.address_slash
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = settings;
        AddressSlash::Forward
    }
}

pub(crate) fn filesystem_name(settings: &ExplorerSettings) -> String {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        settings.view.filesystem_name.clone()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = settings;
        "Filesystem".to_owned()
    }
}

fn format_configured_path(path: &Path, slash: AddressSlash) -> String {
    let text = path.display().to_string();

    #[cfg(target_os = "windows")]
    {
        match slash {
            AddressSlash::Forward => text.replace('\\', "/"),
            AddressSlash::Back => text.replace('/', "\\"),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = slash;
        text
    }
}

fn default_app_start_path() -> PathBuf {
    let home = crate::explorer::user_home_dir();
    crate::explorer::user_downloads_dir(home.as_deref())
        .or_else(|| home.map(|home| home.join("Downloads")))
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(platform_root_path)
}

fn platform_root_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(r"C:\")
    }

    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from("/")
    }
}

fn default_sidebar_items() -> Vec<PathBuf> {
    let home = crate::explorer::user_home_dir();
    default_sidebar_items_from_paths(
        home.clone(),
        crate::explorer::user_desktop_dir(home.as_deref()),
        crate::explorer::user_documents_dir(home.as_deref()),
        crate::explorer::user_downloads_dir(home.as_deref()),
        crate::explorer::macos_applications_dir(),
        crate::explorer::macos_bin_dir(home.as_deref()),
    )
}

fn default_sidebar_items_from_paths(
    home: Option<PathBuf>,
    desktop: Option<PathBuf>,
    documents: Option<PathBuf>,
    downloads: Option<PathBuf>,
    applications: Option<PathBuf>,
    bin: Option<PathBuf>,
) -> Vec<PathBuf> {
    [home, desktop, documents, downloads, applications, bin]
        .into_iter()
        .flatten()
        .collect()
}

fn default_sidebar_width() -> u32 {
    SIDEBAR_DEFAULT_WIDTH
}

fn default_sidebar_expanded_groups() -> Vec<SidebarGroupKind> {
    vec![SidebarGroupKind::Pinned]
}

fn default_date_format() -> String {
    DEFAULT_DATE_FORMAT.to_owned()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn default_filesystem_name() -> String {
    DEFAULT_FILESYSTEM_NAME.to_owned()
}

fn default_font() -> String {
    DEFAULT_FONT.to_owned()
}

fn default_cache_cleanup_interval_days() -> u32 {
    DEFAULT_CACHE_CLEANUP_INTERVAL_DAYS
}

fn default_show_dotfiles() -> bool {
    true
}

fn default_media_view_mode() -> FileViewMode {
    FileViewMode::LargeIcons
}

fn default_file_columns() -> FileColumnSettings {
    let mut widths = BTreeMap::new();
    for kind in default_file_column_order() {
        widths.insert(*kind, default_file_column_width(*kind));
    }
    FileColumnSettings {
        name_width: None,
        order: default_file_column_order().to_vec(),
        widths,
    }
}

fn default_file_sort() -> FileSortSettings {
    FileSortSettings {
        column: FileSortColumn::Name,
        direction: SortDirection::Ascending,
    }
}

pub(crate) fn default_file_column_order() -> &'static [FileColumnKind] {
    &[
        FileColumnKind::DateModified,
        FileColumnKind::Type,
        FileColumnKind::Size,
    ]
}

pub(crate) fn default_file_column_width(kind: FileColumnKind) -> u32 {
    match kind {
        FileColumnKind::DateModified => crate::explorer::constants::COLUMN_DATE_WIDTH as u32,
        FileColumnKind::Type => crate::explorer::constants::COLUMN_TYPE_WIDTH as u32,
        FileColumnKind::Size => crate::explorer::constants::COLUMN_SIZE_WIDTH as u32,
    }
}

fn drive_hide_kind_from_str(value: &str) -> Option<DriveHideKind> {
    match value {
        "wsl" => Some(DriveHideKind::Wsl),
        _ => None,
    }
}

fn sidebar_group_kind_from_str(value: &str) -> Option<SidebarGroupKind> {
    match value {
        "pinned" => Some(SidebarGroupKind::Pinned),
        "drives" => Some(SidebarGroupKind::Drives),
        "wsl" => Some(SidebarGroupKind::Wsl),
        _ => None,
    }
}

fn deserialize_sidebar_items<'de, D>(deserializer: D) -> Result<Vec<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    let items = Vec::<SidebarItemSetting>::deserialize(deserializer)?;
    Ok(items
        .into_iter()
        .filter_map(|item| match item {
            SidebarItemSetting::Path(path) => Some(path),
            SidebarItemSetting::Legacy(location) => location.configured_path(),
        })
        .collect())
}

fn deserialize_drive_hide_kinds<'de, D>(deserializer: D) -> Result<Vec<DriveHideKind>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(drive_hide_kinds_from_value(value))
}

fn deserialize_sidebar_expanded_groups<'de, D>(
    deserializer: D,
) -> Result<Vec<SidebarGroupKind>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(sidebar_group_kinds_from_value(value))
}

fn drive_hide_kinds_from_value(value: Value) -> Vec<DriveHideKind> {
    let Some(values) = value.as_array() else {
        return Vec::new();
    };

    let mut kinds = Vec::new();
    for kind in values
        .iter()
        .filter_map(Value::as_str)
        .filter_map(drive_hide_kind_from_str)
    {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

fn sidebar_group_kinds_from_value(value: Value) -> Vec<SidebarGroupKind> {
    let Some(values) = value.as_array() else {
        return default_sidebar_expanded_groups();
    };

    let mut kinds = Vec::new();
    for kind in values
        .iter()
        .filter_map(Value::as_str)
        .filter_map(sidebar_group_kind_from_str)
    {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

fn file_column_kind_from_str(value: &str) -> Option<FileColumnKind> {
    match value {
        "date_modified" => Some(FileColumnKind::DateModified),
        "type" => Some(FileColumnKind::Type),
        "size" => Some(FileColumnKind::Size),
        _ => None,
    }
}

fn file_sort_column_from_str(value: &str) -> Option<FileSortColumn> {
    match value {
        "name" => Some(FileSortColumn::Name),
        "date_modified" => Some(FileSortColumn::DateModified),
        "type" => Some(FileSortColumn::Type),
        "size" => Some(FileSortColumn::Size),
        _ => None,
    }
}

fn sort_direction_from_str(value: &str) -> Option<SortDirection> {
    match value {
        "ascending" => Some(SortDirection::Ascending),
        "descending" => Some(SortDirection::Descending),
        _ => None,
    }
}

fn deserialize_file_column_settings<'de, D>(deserializer: D) -> Result<FileColumnSettings, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(file_column_settings_from_value(value))
}

fn deserialize_file_sort_settings<'de, D>(deserializer: D) -> Result<FileSortSettings, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(file_sort_settings_from_value(value))
}

fn file_sort_settings_from_value(value: Value) -> FileSortSettings {
    let mut settings = default_file_sort();
    let Some(object) = value.as_object() else {
        return settings;
    };

    if let Some(column) = object
        .get("column")
        .and_then(Value::as_str)
        .and_then(file_sort_column_from_str)
    {
        settings.column = column;
    }

    if let Some(direction) = object
        .get("direction")
        .and_then(Value::as_str)
        .and_then(sort_direction_from_str)
    {
        settings.direction = direction;
    }

    settings
}

fn file_column_settings_from_value(value: Value) -> FileColumnSettings {
    let mut settings = default_file_columns();
    let Some(object) = value.as_object() else {
        return settings;
    };

    if let Some(order) = object.get("order").and_then(Value::as_array) {
        settings.order = order
            .iter()
            .filter_map(Value::as_str)
            .filter_map(file_column_kind_from_str)
            .collect();
    }

    if let Some(widths) = object.get("widths").and_then(Value::as_object) {
        for (key, value) in widths {
            let Some(kind) = file_column_kind_from_str(key) else {
                continue;
            };
            let Some(width) = value.as_u64().and_then(|width| u32::try_from(width).ok()) else {
                continue;
            };
            settings
                .widths
                .insert(kind, normalized_file_column_width(width));
        }
    }

    settings.name_width = object
        .get("name_width")
        .and_then(Value::as_u64)
        .and_then(|width| u32::try_from(width).ok())
        .map(normalized_name_column_width);

    normalize_file_column_settings(&mut settings);
    settings
}

fn normalize_file_column_settings(settings: &mut FileColumnSettings) {
    let mut normalized_order = Vec::with_capacity(default_file_column_order().len());
    for kind in settings.order.iter().copied() {
        if default_file_column_order().contains(&kind) && !normalized_order.contains(&kind) {
            normalized_order.push(kind);
        }
    }
    for kind in default_file_column_order().iter().copied() {
        if !normalized_order.contains(&kind) {
            normalized_order.push(kind);
        }
    }
    settings.order = normalized_order;

    let mut normalized_widths = BTreeMap::new();
    for kind in default_file_column_order().iter().copied() {
        let width = settings
            .widths
            .get(&kind)
            .copied()
            .unwrap_or_else(|| default_file_column_width(kind));
        normalized_widths.insert(kind, normalized_file_column_width(width));
    }
    settings.widths = normalized_widths;
    settings.name_width = settings.name_width.map(normalized_name_column_width);
}

pub(crate) fn resolved_font_family(value: &str) -> SharedString {
    if value == DEFAULT_FONT {
        SYSTEM_UI_FONT.into()
    } else {
        value.to_owned().into()
    }
}

pub(crate) fn app_font(settings: &ExplorerSettings) -> Font {
    font(resolved_font_family(&settings.view.font))
}

pub(crate) fn current_app_font(cx: &App) -> Font {
    cx.try_global::<SettingsState>()
        .map(|state| app_font(&state.value))
        .unwrap_or_else(|| app_font(&ExplorerSettings::default()))
}

fn deserialize_sidebar_width<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    u32::deserialize(deserializer).map(normalized_sidebar_width)
}

fn deserialize_cache_cleanup_interval_days<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    u32::deserialize(deserializer).map(normalized_cache_cleanup_interval_days)
}

fn deserialize_app_start_path<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(app_start_path_from_value(value))
}

fn app_start_path_from_value(value: Value) -> PathBuf {
    value
        .as_str()
        .map(PathBuf::from)
        .filter(|path| configured_path_shape_is_valid(path))
        .unwrap_or_else(default_app_start_path)
}

fn configured_path_shape_is_valid(path: &Path) -> bool {
    path.is_absolute() || is_tilde_path(path)
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
        ConfigPlatform::Windows => env_path("USERPROFILE")
            .map(|profile| profile.join(".config").join(LINUX_CONFIG_DIR_NAME)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn defaults_match_generated_settings_contract() {
        let settings = ExplorerSettings::default();
        assert_eq!(settings.contextmenu.items, default_context_menu_items());
        assert!(settings.view.show_dotfiles);
        assert!(!settings.view.show_hidden);
        assert_eq!(settings.view.date_format, DEFAULT_DATE_FORMAT);
        assert_eq!(settings.view.font, DEFAULT_FONT);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert_eq!(settings.view.filesystem_name, DEFAULT_FILESYSTEM_NAME);
        #[cfg(target_os = "windows")]
        assert_eq!(settings.view.address_slash, AddressSlash::Forward);
        assert!(settings.view.show_extensions);
        assert!(!settings.view.show_folder_sizes);
        assert!(!settings.tabs.focus_new);
        assert_eq!(settings.view.mode, FileViewMode::Details);
        assert_eq!(settings.view.mode_media, FileViewMode::LargeIcons);
        assert_eq!(settings.view.remote_mode_media, FileViewMode::Details);
        assert!(!settings.view.remote_thumbnails);
        assert!(settings.view.native_icons);
        assert_eq!(settings.view.sort, default_file_sort());
        assert_eq!(
            settings.view.file_columns.order,
            default_file_column_order()
        );
        assert_eq!(
            settings
                .view
                .file_columns
                .widths
                .get(&FileColumnKind::DateModified),
            Some(&default_file_column_width(FileColumnKind::DateModified))
        );
        assert_eq!(settings.view.file_columns.name_width, None);
        assert_eq!(settings.app.start, default_app_start_path());
        assert_eq!(settings.app.new_window_behaviour, NewWindowBehaviour::Focus);
        assert_eq!(
            settings.app.cache_cleanup_interval_days,
            DEFAULT_CACHE_CLEANUP_INTERVAL_DAYS
        );
        assert!(settings.sidebar.hide.is_empty());
        assert_eq!(
            settings.sidebar.expanded_groups,
            vec![SidebarGroupKind::Pinned]
        );
        assert_eq!(settings.sidebar.width, SIDEBAR_DEFAULT_WIDTH);
        assert_eq!(
            settings.sidebar.items.len(),
            if cfg!(target_os = "macos") { 6 } else { 4 }
        );
    }

    #[test]
    fn windows_default_contextmenu_adds_archive_before_terminal() {
        assert_eq!(
            default_context_menu_items_for(
                ConfigPlatform::Windows,
                |executable| executable == "7zG",
                |_| false,
            ),
            vec![
                CustomContextMenuItem::Item {
                    label: "Add to archive...".to_owned(),
                    exe: PathBuf::from("7zG"),
                    icon: None,
                    args: vec![
                        "a".to_owned(),
                        "-ad".to_owned(),
                        "-saa".to_owned(),
                        "{path}".to_owned(),
                        "{paths}".to_owned(),
                    ],
                    only: vec!["*file".to_owned(), "*folder".to_owned()],
                },
                CustomContextMenuItem::Item {
                    label: "Terminal".to_owned(),
                    exe: PathBuf::from("wt"),
                    icon: Some(PathBuf::from(WINDOWS_TERMINAL_ICON_URL)),
                    args: vec!["-d".to_owned(), "{paths}".to_owned()],
                    only: vec!["*directory".to_owned(), "*folders".to_owned()],
                },
            ]
        );
        assert!(matches!(
            default_context_menu_items_for(ConfigPlatform::Windows, |_| false, |_| false).as_slice(),
            [CustomContextMenuItem::Action { action: ContextMenuAction::Compress, .. }, CustomContextMenuItem::Item { label, .. }]
                if label == "Terminal"
        ));
    }

    #[test]
    fn macos_default_contextmenu_prefers_cmux_then_ghostty_then_terminal() {
        let cmux = default_context_menu_items_for(
            ConfigPlatform::MacOS,
            |_| false,
            |application| matches!(application, "cmux" | "Ghostty"),
        );
        assert_eq!(
            cmux,
            vec![
                compress_context_menu_item(),
                CustomContextMenuItem::Item {
                    label: "cmux".to_owned(),
                    exe: PathBuf::from("/usr/bin/open"),
                    icon: Some(PathBuf::from(CMUX_ICON_URL)),
                    args: vec!["-a".to_owned(), "cmux".to_owned(), "{path}".to_owned()],
                    only: vec!["*directory".to_owned(), "*folders".to_owned()],
                },
            ]
        );

        let ghostty = default_context_menu_items_for(
            ConfigPlatform::MacOS,
            |_| false,
            |application| application == "Ghostty",
        );
        assert!(matches!(
            ghostty.as_slice(),
            [CustomContextMenuItem::Action { .. }, CustomContextMenuItem::Item { label, icon: Some(icon), args, .. }]
                if label == "Ghostty"
                    && icon == Path::new(GHOSTTY_ICON_URL)
                    && args == &["-a", "Ghostty", "{path}"]
        ));

        let terminal = default_context_menu_items_for(ConfigPlatform::MacOS, |_| false, |_| false);
        assert!(matches!(
            terminal.as_slice(),
            [CustomContextMenuItem::Action { .. }, CustomContextMenuItem::Item { label, icon: None, args, .. }]
                if label == "Terminal" && args == &["-a", "Terminal", "{path}"]
        ));
    }

    #[test]
    fn linux_default_contextmenu_prefers_ghostty_then_xdg_then_legacy() {
        let ghostty = default_context_menu_items_for(ConfigPlatform::Linux, |_| true, |_| false);
        assert!(matches!(
            ghostty.as_slice(),
            [CustomContextMenuItem::Action { .. }, CustomContextMenuItem::Item { label, icon: Some(icon), args, .. }]
                if label == "Ghostty"
                    && icon == Path::new(GHOSTTY_ICON_URL)
                    && args == &["--working-directory", "{path}"]
        ));

        let xdg = default_context_menu_items_for(
            ConfigPlatform::Linux,
            |executable| executable == "xdg-terminal-exec",
            |_| false,
        );
        assert!(matches!(
            xdg.as_slice(),
            [CustomContextMenuItem::Action { .. }, CustomContextMenuItem::Item { exe, args, .. }]
                if exe == Path::new("xdg-terminal-exec") && args == &["{cwd}"]
        ));

        let legacy = default_context_menu_items_for(
            ConfigPlatform::Linux,
            |executable| executable == "x-terminal-emulator",
            |_| false,
        );
        assert!(matches!(
            legacy.as_slice(),
            [CustomContextMenuItem::Action { .. }, CustomContextMenuItem::Item { exe, args, .. }]
                if exe == Path::new("x-terminal-emulator") && args == &["{cwd}"]
        ));
        assert_eq!(
            default_context_menu_items_for(ConfigPlatform::Linux, |_| false, |_| false),
            vec![compress_context_menu_item()]
        );
    }

    #[test]
    fn contextmenu_compress_action_round_trips_and_rejects_ambiguous_items() {
        let item: CustomContextMenuItem = serde_json::from_str(
            r#"{"label":"Zip it","action":"compress","icon":"https://example.com/archive.png","only":["*file","*folder"]}"#,
        )
        .expect("deserialize compress action");
        assert!(matches!(
            &item,
            CustomContextMenuItem::Action {
                label,
                action: ContextMenuAction::Compress,
                icon: Some(icon),
                only,
            } if label == "Zip it"
                && icon == Path::new("https://example.com/archive.png")
                && only == &["*file", "*folder"]
        ));
        let serialized = serde_json::to_value(&item).expect("serialize compress action");
        assert_eq!(serialized["action"], "compress");
        assert!(
            serde_json::from_str::<CustomContextMenuItem>(
                r#"{"label":"Bad","action":"compress","exe":"zip"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<CustomContextMenuItem>(r#"{"label":"Bad","action":"unknown"}"#)
                .is_err()
        );
    }

    #[test]
    fn explicit_empty_contextmenu_is_preserved_while_missing_and_null_use_defaults() {
        let explicit: ExplorerSettings = serde_json::from_str(r#"{"contextmenu":[]}"#)
            .expect("deserialize explicit empty context menu");
        assert!(explicit.contextmenu.items.is_empty());

        let missing: ExplorerSettings =
            serde_json::from_str("{}").expect("deserialize missing context menu");
        assert_eq!(missing.contextmenu, ContextMenuSettings::default());

        let null: ExplorerSettings =
            serde_json::from_str(r#"{"contextmenu":null}"#).expect("deserialize null context menu");
        assert_eq!(null.contextmenu, ContextMenuSettings::default());
    }

    #[test]
    fn sidebar_default_items_append_macos_locations() {
        let home = PathBuf::from("home");
        let desktop = PathBuf::from("Desktop");
        let documents = PathBuf::from("Documents");
        let downloads = PathBuf::from("Downloads");
        let applications = PathBuf::from("Applications");
        let bin = PathBuf::from(".Trash");

        assert_eq!(
            default_sidebar_items_from_paths(
                Some(home.clone()),
                Some(desktop.clone()),
                Some(documents.clone()),
                Some(downloads.clone()),
                Some(applications.clone()),
                Some(bin.clone()),
            ),
            vec![home, desktop, documents, downloads, applications, bin]
        );
    }

    #[test]
    fn sidebar_default_items_omit_unavailable_macos_locations() {
        let home = PathBuf::from("home");
        let desktop = PathBuf::from("Desktop");
        let documents = PathBuf::from("Documents");
        let downloads = PathBuf::from("Downloads");

        assert_eq!(
            default_sidebar_items_from_paths(
                Some(home.clone()),
                Some(desktop.clone()),
                Some(documents.clone()),
                Some(downloads.clone()),
                None,
                None,
            ),
            vec![home, desktop, documents, downloads]
        );
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
    fn settings_default_missing_fields_when_unknown_fields_are_present() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"view":{"show_hidden":true},"future_option":42}"#)
                .expect("deserialize partial settings");
        assert!(settings.view.show_hidden);
        assert!(settings.view.show_dotfiles);
        assert_eq!(settings.view.date_format, DEFAULT_DATE_FORMAT);
        assert_eq!(settings.view.font, DEFAULT_FONT);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert_eq!(settings.view.filesystem_name, DEFAULT_FILESYSTEM_NAME);
        #[cfg(target_os = "windows")]
        assert_eq!(settings.view.address_slash, AddressSlash::Forward);
        assert!(settings.view.show_extensions);
        assert!(!settings.view.show_folder_sizes);
        assert!(!settings.tabs.focus_new);
        assert_eq!(settings.view.mode, FileViewMode::Details);
        assert_eq!(settings.view.mode_media, FileViewMode::LargeIcons);
        assert_eq!(settings.view.remote_mode_media, FileViewMode::Details);
        assert!(!settings.view.remote_thumbnails);
        assert!(settings.view.native_icons);
        assert_eq!(settings.view.file_columns, default_file_columns());
        assert_eq!(settings.view.file_columns.name_width, None);
        assert_eq!(settings.view.sort, default_file_sort());
        assert_eq!(settings.app.start, default_app_start_path());
        assert_eq!(settings.app.new_window_behaviour, NewWindowBehaviour::Focus);
        assert_eq!(settings.contextmenu.items, default_context_menu_items());
        assert!(settings.sidebar.hide.is_empty());
        assert_eq!(
            settings.sidebar.expanded_groups,
            vec![SidebarGroupKind::Pinned]
        );
        assert_eq!(settings.sidebar.width, SIDEBAR_DEFAULT_WIDTH);
        assert_eq!(settings.sidebar.items.len(), 4);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn filesystem_name_deserializes_and_serializes_on_unix() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"view":{"filesystem_name":"System Root"}}"#)
                .expect("deserialize settings");

        assert_eq!(settings.view.filesystem_name, "System Root");
        let value = serde_json::to_value(settings).expect("serialize settings");
        assert_eq!(value["view"]["filesystem_name"], "System Root");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn filesystem_name_is_omitted_on_windows() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"view":{"filesystem_name":"System Root"}}"#)
                .expect("deserialize settings");

        let value = serde_json::to_value(settings).expect("serialize settings");
        assert!(value["view"].get("filesystem_name").is_none());
    }

    #[test]
    fn app_cache_cleanup_interval_deserializes_and_normalizes() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"app":{"cache_cleanup_interval_days":0}}"#)
                .expect("deserialize app settings");
        assert_eq!(settings.app.cache_cleanup_interval_days, 1);

        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"app":{"cache_cleanup_interval_days":45}}"#)
                .expect("deserialize app settings");
        assert_eq!(settings.app.cache_cleanup_interval_days, 45);
    }

    #[test]
    fn app_new_window_behaviour_deserializes_and_defaults_to_focus() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"app":{}}"#).expect("deserialize default app settings");
        assert_eq!(settings.app.new_window_behaviour, NewWindowBehaviour::Focus);

        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"app":{"new_window_behaviour":"open"}}"#)
                .expect("deserialize open app setting");
        assert_eq!(settings.app.new_window_behaviour, NewWindowBehaviour::Open);

        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"app":{"new_window_behaviour":"focus"}}"#)
                .expect("deserialize focus app setting");
        assert_eq!(settings.app.new_window_behaviour, NewWindowBehaviour::Focus);
    }

    #[test]
    fn app_new_window_behaviour_serializes_known_values() {
        let mut app = AppSettings::default();
        app.new_window_behaviour = NewWindowBehaviour::Open;
        assert_eq!(
            serde_json::to_value(&app).expect("serialize open app settings")["new_window_behaviour"],
            "open"
        );

        app.new_window_behaviour = NewWindowBehaviour::Focus;
        assert_eq!(
            serde_json::to_value(&app).expect("serialize focus app settings")["new_window_behaviour"],
            "focus"
        );
    }

    #[test]
    fn app_new_window_behaviour_rejects_unknown_values() {
        let result: Result<ExplorerSettings, _> =
            serde_json::from_str(r#"{"app":{"new_window_behaviour":"reuse"}}"#);
        assert!(result.is_err());
    }

    #[test]
    fn app_start_deserializes_valid_absolute_and_tilde_paths() {
        let start = unique_temp_dir("app-start-absolute");
        let settings: ExplorerSettings = serde_json::from_value(serde_json::json!({
            "app": {
                "start": start.clone()
            }
        }))
        .expect("deserialize absolute app start path");

        assert_eq!(settings.app.start, start);

        let settings: ExplorerSettings = serde_json::from_str(r#"{"app":{"start":"~/Downloads"}}"#)
            .expect("deserialize tilde app start path");

        assert_eq!(settings.app.start, PathBuf::from("~/Downloads"));
    }

    #[test]
    fn app_start_replaces_invalid_values_with_default_downloads_path() {
        for start in [
            serde_json::json!({"kind": "downloads"}),
            serde_json::json!({"kind": "custom", "path": "/tmp/ignored"}),
            serde_json::json!("relative/path"),
            serde_json::json!(42),
            serde_json::json!([]),
            Value::Null,
        ] {
            let settings: ExplorerSettings = serde_json::from_value(serde_json::json!({
                "app": {
                    "start": start
                }
            }))
            .expect("deserialize app start with default fallback");

            assert_eq!(settings.app.start, default_app_start_path());
        }
    }

    #[test]
    fn app_start_load_normalizes_legacy_object_to_default_string() {
        let path = unique_temp_dir("legacy-app-start").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"app":{"start":{"kind":"custom","path":"/tmp/ignored"}}}"#,
        )
        .unwrap();

        let loaded = load_settings_document_from_path(&path).unwrap();
        assert_eq!(loaded.value.app.start, default_app_start_path());

        let normalized = fs::read_to_string(&path).unwrap();
        let document: Value = serde_json::from_str(&normalized).unwrap();
        assert_eq!(
            document["app"]["start"],
            Value::String(format_configured_path(
                &loaded.value.app.start,
                settings_address_slash(&loaded.value),
            ))
        );
        assert!(document["app"]["start"].is_string());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn address_slash_deserializes_backslashes() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"view":{"address_slash":"back"}}"#)
                .expect("deserialize settings");

        assert_eq!(settings.view.address_slash, AddressSlash::Back);
    }

    #[test]
    fn sidebar_hide_deserializes_wsl_and_ignores_unknown_values() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"sidebar":{"hide":["wsl","future_drive",42,"wsl"]}}"#)
                .expect("deserialize sidebar hide settings");

        assert_eq!(settings.sidebar.hide, vec![DriveHideKind::Wsl]);
    }

    #[test]
    fn sidebar_expanded_groups_default_to_pinned() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"sidebar":{}}"#).expect("deserialize sidebar settings");

        assert_eq!(
            settings.sidebar.expanded_groups,
            vec![SidebarGroupKind::Pinned]
        );
    }

    #[test]
    fn sidebar_expanded_groups_deserialize_and_ignore_unknown_values() {
        let settings: ExplorerSettings = serde_json::from_str(
            r#"{"sidebar":{"expanded_groups":["drives","future_group",42,"wsl","drives","macos"]}}"#,
        )
        .expect("deserialize sidebar expanded group settings");

        assert_eq!(
            settings.sidebar.expanded_groups,
            vec![SidebarGroupKind::Drives, SidebarGroupKind::Wsl]
        );
    }

    #[test]
    fn sidebar_width_is_normalized_from_settings() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"sidebar":{"width":50}}"#).expect("deserialize settings");

        assert_eq!(settings.sidebar.width, SIDEBAR_MIN_WIDTH);
        assert_eq!(normalized_sidebar_width(99), SIDEBAR_MIN_WIDTH);
        assert_eq!(normalized_sidebar_width(100), SIDEBAR_MIN_WIDTH);
        assert_eq!(normalized_sidebar_width(250), 250);
    }

    #[test]
    fn view_mode_deserializes_large_icons() {
        let settings: ExplorerSettings = serde_json::from_str(r#"{"view":{"mode":"large_icons"}}"#)
            .expect("deserialize settings");

        assert_eq!(settings.view.mode, FileViewMode::LargeIcons);
    }

    #[test]
    fn media_view_mode_deserializes_large_icons() {
        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"view":{"mode_media":"large_icons"}}"#)
                .expect("deserialize settings");

        assert_eq!(settings.view.mode_media, FileViewMode::LargeIcons);
    }

    #[test]
    fn remote_view_settings_deserialize_and_round_trip() {
        let settings: ExplorerSettings = serde_json::from_str(
            r#"{"view":{"remote_thumbnails":true,"remote_mode_media":"large_icons"}}"#,
        )
        .expect("deserialize settings");

        assert!(settings.view.remote_thumbnails);
        assert_eq!(settings.view.remote_mode_media, FileViewMode::LargeIcons);

        let value = serde_json::to_value(settings).expect("serialize settings");
        assert_eq!(value["view"]["remote_thumbnails"], true);
        assert_eq!(value["view"]["remote_mode_media"], "large_icons");
    }

    #[test]
    fn file_sort_deserializes_known_values_and_defaults_unknowns() {
        let settings: ExplorerSettings = serde_json::from_str(
            r#"{"view":{"sort":{"column":"type","direction":"ascending","future":7}}}"#,
        )
        .expect("deserialize file sort");

        assert_eq!(
            settings.view.sort,
            FileSortSettings {
                column: FileSortColumn::Type,
                direction: SortDirection::Ascending,
            }
        );

        let settings: ExplorerSettings =
            serde_json::from_str(r#"{"view":{"sort":{"column":"future","direction":"sideways"}}}"#)
                .expect("deserialize unknown file sort");

        assert_eq!(settings.view.sort, default_file_sort());
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
            cx.global::<SettingsState>().value.sidebar.width
        });

        assert_eq!(sidebar_width, SIDEBAR_MIN_WIDTH);
        assert_eq!(
            load_settings_from_path(&path).unwrap().sidebar.width,
            SIDEBAR_MIN_WIDTH
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[gpui::test]
    fn set_sidebar_group_expanded_persists_group_list(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("sidebar-expanded").join(SETTINGS_FILE_NAME);
        cx.set_global(SettingsState {
            value: ExplorerSettings::default(),
            document: settings_document(&ExplorerSettings::default()),
            path: path.clone(),
            _watcher: None,
        });

        let changed =
            cx.update(|cx| set_sidebar_group_expanded(SidebarGroupKind::Drives, true, cx));

        assert!(changed);
        assert_eq!(
            load_settings_from_path(&path)
                .unwrap()
                .sidebar
                .expanded_groups,
            vec![SidebarGroupKind::Pinned, SidebarGroupKind::Drives]
        );

        let changed =
            cx.update(|cx| set_sidebar_group_expanded(SidebarGroupKind::Pinned, false, cx));

        assert!(changed);
        assert_eq!(
            load_settings_from_path(&path)
                .unwrap()
                .sidebar
                .expanded_groups,
            vec![SidebarGroupKind::Drives]
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[gpui::test]
    fn set_file_sort_persists_global_sort(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("file-sort").join(SETTINGS_FILE_NAME);
        cx.set_global(SettingsState {
            value: ExplorerSettings::default(),
            document: settings_document(&ExplorerSettings::default()),
            path: path.clone(),
            _watcher: None,
        });
        let sort = FileSortSettings {
            column: FileSortColumn::Type,
            direction: SortDirection::Ascending,
        };

        let stored = cx.update(|cx| {
            set_file_sort(sort, cx);
            cx.global::<SettingsState>().value.view.sort
        });

        assert_eq!(stored, sort);
        assert_eq!(load_settings_from_path(&path).unwrap().view.sort, sort);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn file_column_settings_ignore_unknowns_dedupe_and_append_missing_defaults() {
        let settings: ExplorerSettings = serde_json::from_str(
            r#"{"view":{"file_columns":{"name_width":10,"order":["size","unknown","size"],"widths":{"size":10,"unknown":999}}}}"#,
        )
        .expect("deserialize file columns");

        assert_eq!(
            settings.view.file_columns.order,
            vec![
                FileColumnKind::Size,
                FileColumnKind::DateModified,
                FileColumnKind::Type
            ]
        );
        assert_eq!(
            settings.view.file_columns.widths[&FileColumnKind::Size],
            FILE_COLUMN_MIN_WIDTH
        );
        assert_eq!(
            settings.view.file_columns.widths[&FileColumnKind::Type],
            default_file_column_width(FileColumnKind::Type)
        );
        assert_eq!(
            settings.view.file_columns.name_width,
            Some(crate::explorer::constants::COLUMN_NAME_MIN_WIDTH as u32)
        );
    }

    #[gpui::test]
    fn set_file_column_width_persists_clamped_value(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("file-column-width").join(SETTINGS_FILE_NAME);
        cx.set_global(SettingsState {
            value: ExplorerSettings::default(),
            document: settings_document(&ExplorerSettings::default()),
            path: path.clone(),
            _watcher: None,
        });

        cx.update(|cx| {
            set_file_column_width(FileColumnKind::Type, 10, cx);
        });

        assert_eq!(
            load_settings_from_path(&path)
                .unwrap()
                .view
                .file_columns
                .widths[&FileColumnKind::Type],
            FILE_COLUMN_MIN_WIDTH
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[gpui::test]
    fn set_name_column_width_persists_clamped_value(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("name-column-width").join(SETTINGS_FILE_NAME);
        cx.set_global(SettingsState {
            value: ExplorerSettings::default(),
            document: settings_document(&ExplorerSettings::default()),
            path: path.clone(),
            _watcher: None,
        });

        cx.update(|cx| {
            set_name_column_width(10, cx);
        });

        assert_eq!(
            load_settings_from_path(&path)
                .unwrap()
                .view
                .file_columns
                .name_width,
            Some(crate::explorer::constants::COLUMN_NAME_MIN_WIDTH as u32)
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[gpui::test]
    fn clear_name_column_width_removes_persisted_key(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("clear-name-column-width").join(SETTINGS_FILE_NAME);
        let mut settings = ExplorerSettings::default();
        settings.view.file_columns.name_width = Some(320);
        cx.set_global(SettingsState {
            value: settings.clone(),
            document: settings_document(&settings),
            path: path.clone(),
            _watcher: None,
        });
        save_settings_to_path(&path, &settings).expect("save settings");

        cx.update(|cx| {
            clear_name_column_width(cx);
        });

        let saved = fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("name_width"));
        assert_eq!(
            load_settings_from_path(&path)
                .unwrap()
                .view
                .file_columns
                .name_width,
            None
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[gpui::test]
    fn reorder_file_column_persists_global_order(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("file-column-order").join(SETTINGS_FILE_NAME);
        cx.set_global(SettingsState {
            value: ExplorerSettings::default(),
            document: settings_document(&ExplorerSettings::default()),
            path: path.clone(),
            _watcher: None,
        });

        let changed = cx.update(|cx| {
            reorder_file_column(FileColumnKind::Size, FileColumnKind::DateModified, true, cx)
        });

        assert!(changed);
        assert_eq!(
            load_settings_from_path(&path)
                .unwrap()
                .view
                .file_columns
                .order,
            vec![
                FileColumnKind::Size,
                FileColumnKind::DateModified,
                FileColumnKind::Type
            ]
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn settings_round_trip_pretty_json() {
        let path = unique_temp_dir("round-trip").join(SETTINGS_FILE_NAME);
        let settings = ExplorerSettings::default();
        save_settings_to_path(&path, &settings).expect("save settings");
        assert_eq!(load_settings_from_path(&path).unwrap(), settings);
        let json = fs::read_to_string(&path).unwrap();
        assert!(!json.ends_with('\n'));
        assert!(json.starts_with("{\n  \"app\": {"));
        let document: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            document["contextmenu"],
            serde_json::to_value(&settings.contextmenu).unwrap()
        );
        assert_eq!(
            document["app"]["start"],
            Value::String(format_configured_path(
                &settings.app.start,
                settings_address_slash(&settings),
            ))
        );
        assert_eq!(document["app"]["new_window_behaviour"], "focus");
        assert!(document["app"]["start"].is_string());
        assert_eq!(
            document["sidebar"]["expanded_groups"],
            Value::Array(vec![Value::String("pinned".to_owned())])
        );
        assert!(json.contains("\n    \"expanded_groups\": [\"pinned\"],"));
        assert!(json.contains("\n    \"hide\": [],"));
        let expected_sidebar_items = settings
            .sidebar
            .items
            .iter()
            .map(|path| {
                Value::String(format_configured_path(
                    path,
                    settings_address_slash(&settings),
                ))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            document["sidebar"]["items"],
            Value::Array(expected_sidebar_items)
        );
        assert!(json.contains("\n  \"tabs\": {\"focus_new\": false},"));
        assert!(json.contains("\n      \"order\": [\"date_modified\", \"type\", \"size\"],"));
        assert!(json.contains(
            "\n      \"widths\": {\"date_modified\": 150, \"size\": 120, \"type\": 150}"
        ));
        assert!(!json.contains("name_width"));
        assert!(json.contains("\n    \"font\": \"default\""));
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            assert_eq!(document["view"]["filesystem_name"], DEFAULT_FILESYSTEM_NAME);
            assert!(json.contains("\n    \"filesystem_name\": \"Filesystem\","));
        }
        #[cfg(target_os = "windows")]
        {
            assert!(document["view"].get("filesystem_name").is_none());
            assert!(!json.contains("filesystem_name"));
        }
        assert!(json.contains("\n    \"native_icons\": true"));
        assert!(json.contains("\n    \"remote_mode_media\": \"details\""));
        assert!(json.contains("\n    \"remote_thumbnails\": false"));
        assert!(json.contains("\n    \"show_folder_sizes\": false"));
        assert!(json.contains("\n    \"date_format\": \"%Y/%m/%d %H:%M\""));
        assert!(
            json.contains("\n    \"sort\": {\"column\": \"name\", \"direction\": \"ascending\"}")
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn settings_formatter_wraps_long_and_nested_values() {
        let mut document = serde_json::json!({
            "contextmenu": [
                {
                    "label": "Tools",
                    "items": [
                        {
                            "label": "Inspect",
                            "exe": "~/bin/inspect"
                        }
                    ]
                }
            ]
        });
        document["columns"] = Value::Array(
            (0..16)
                .map(|index| Value::String(format!("column_{index:02}")))
                .collect(),
        );
        sort_json_objects(&mut document);

        let formatted = format_settings_document(&document).unwrap();

        assert!(formatted.contains("\n  \"columns\": [\n    \"column_00\","));
        assert!(!formatted.contains("\"columns\": [\"column_00\", \"column_01\""));
        assert!(formatted.contains("\n  \"contextmenu\": [\n    {\n"));
        assert!(!formatted.contains("\"contextmenu\": [{"));
        assert!(
            formatted
                .lines()
                .all(|line| line.chars().count() <= SETTINGS_JSON_MAX_WIDTH)
        );
    }

    #[test]
    fn invalid_date_format_is_rejected() {
        let dir = unique_temp_dir("invalid-date-format");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILE_NAME);
        fs::write(&path, r#"{"view":{"date_format":"%Q"}}"#).unwrap();

        assert!(load_settings_from_path(&path).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn contextmenu_items_support_recursive_submenus_and_tilde_executables() {
        let settings: ExplorerSettings = serde_json::from_str(
            r#"{
                "contextmenu": [
                    {
                        "label": "Tools",
                        "items": [
                            {
                                "label": "Inspect",
                                "exe": "~/bin/inspect",
                                "args": ["--mode", "deep"],
                                "only": ["txt", ".MD"]
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect("deserialize recursive context menu");

        assert!(validate_settings(&settings).is_ok());
        assert!(matches!(
            &settings.contextmenu.items[0],
            CustomContextMenuItem::Submenu { items, .. }
                if matches!(
                    &items[0],
                    CustomContextMenuItem::Item { exe, args, only, .. }
                        if exe == Path::new("~/bin/inspect")
                            && args == &vec!["--mode".to_owned(), "deep".to_owned()]
                            && only == &vec!["txt".to_owned(), ".MD".to_owned()]
                )
        ));
    }

    #[test]
    fn contextmenu_items_accept_array_and_string_args() {
        let settings: ExplorerSettings = serde_json::from_str(
            r#"{
                "contextmenu": [
                    {
                        "label": "Array args",
                        "exe": "~/bin/inspect",
                        "args": ["--line", "two words", "{path}"]
                    },
                    {
                        "label": "String args",
                        "exe": "~/bin/inspect",
                        "args": "--line 'two words' {paths}"
                    }
                ]
            }"#,
        )
        .expect("deserialize context menu args");

        assert!(matches!(
            &settings.contextmenu.items[0],
            CustomContextMenuItem::Item { args, .. }
                if args == &vec![
                    "--line".to_owned(),
                    "two words".to_owned(),
                    "{path}".to_owned()
                ]
        ));
        assert!(matches!(
            &settings.contextmenu.items[1],
            CustomContextMenuItem::Item { args, .. }
                if args == &vec![
                    "--line".to_owned(),
                    "two words".to_owned(),
                    "{paths}".to_owned()
                ]
        ));
    }

    #[test]
    fn contextmenu_items_accept_optional_icons_on_items_and_submenus() {
        let item_icon = unique_temp_dir("contextmenu-item-icon").join("tool.png");
        let submenu_icon = "https://raw.githubusercontent.com/microsoft/terminal/9853bc96076e811cef5eab4469095fc9be58201e/res/terminal/images/Square44x44Logo.targetsize-48.png";
        let value = serde_json::json!({
            "contextmenu": [
                {
                    "label": "Tools",
                    "icon": submenu_icon,
                    "items": [
                        {
                            "label": "Inspect",
                            "exe": "~/bin/inspect",
                            "icon": item_icon,
                            "args": ["--mode", "deep"]
                        }
                    ]
                }
            ]
        });
        let settings: ExplorerSettings =
            serde_json::from_value(value).expect("deserialize context menu icons");

        assert!(validate_settings(&settings).is_ok());
        assert!(matches!(
            &settings.contextmenu.items[0],
            CustomContextMenuItem::Submenu { icon, items, .. }
                if icon.as_deref() == Some(Path::new(submenu_icon))
                    && matches!(
                        &items[0],
                        CustomContextMenuItem::Item { icon: child_icon, .. }
                            if child_icon.as_deref() == Some(item_icon.as_path())
                    )
        ));
        let serialized =
            serde_json::to_value(&settings.contextmenu).expect("serialize context menu with icons");
        assert_eq!(serialized[0]["icon"], submenu_icon);
        assert_eq!(
            serialized[0]["items"][0]["icon"],
            serde_json::json!(item_icon)
        );
        assert!(matches!(
            settings.contextmenu.items[0].resolved_icon(),
            Some(ContextMenuConfiguredIcon::Url(url)) if url == submenu_icon
        ));
    }

    #[test]
    fn contextmenu_icons_reject_relative_paths_with_separators() {
        for json in [
            r#"{"contextmenu":[{"label":"Tool","exe":"~/tool","icon":"icons/tool.png"}]}"#,
            r#"{"contextmenu":[{"label":"Tools","icon":"icons/tool.exe","items":[]}]}"#,
        ] {
            let settings: ExplorerSettings =
                serde_json::from_str(json).expect("deserialize context menu icon path");
            assert!(validate_settings(&settings).is_err());
        }
    }

    #[test]
    fn contextmenu_rejects_invalid_args_values() {
        for json in [
            r#"{"contextmenu":[{"label":"Tool","exe":"~/tool","args":"--bad 'quote"}]}"#,
            r#"{"contextmenu":[{"label":"Tool","exe":"~/tool","args":42}]}"#,
        ] {
            assert!(serde_json::from_str::<ExplorerSettings>(json).is_err());
        }
    }

    #[test]
    fn contextmenu_items_accept_recursive_submenus_and_path_executables() {
        let settings: ExplorerSettings = serde_json::from_str(
            r#"{
                "contextmenu": [
                    {
                        "label": "Tools",
                        "items": [
                            {
                                "label": "Inspect",
                                "executable": "rustc"
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect("deserialize recursive context menu");

        assert!(settings.contextmenu.items[0].label() == "Tools");
        assert!(matches!(
            &settings.contextmenu.items[0],
            CustomContextMenuItem::Submenu { items, .. }
                if matches!(
                    &items[0],
                    CustomContextMenuItem::Item { exe, only, .. }
                        if exe == Path::new("rustc") && only.is_empty()
                )
        ));
    }

    #[test]
    fn contextmenu_items_accept_legacy_kind_fields() {
        let settings: ExplorerSettings = serde_json::from_str(
            r#"{
                "contextmenu": {
                    "directory": [
                        {
                            "kind": "submenu",
                            "label": "Tools",
                            "items": [
                                {
                                    "kind": "item",
                                    "label": "Inspect",
                                    "exe": "~/bin/inspect"
                                }
                            ]
                        }
                    ]
                }
            }"#,
        )
        .expect("deserialize legacy context menu");

        assert!(matches!(
            &settings.contextmenu.items[0],
            CustomContextMenuItem::Submenu { items, .. }
                if matches!(
                    &items[0],
                    CustomContextMenuItem::Item { exe, only, .. }
                        if exe == Path::new("~/bin/inspect")
                            && only == &vec!["*directory".to_owned()]
                )
        ));
    }

    #[test]
    fn contextmenu_legacy_sections_migrate_to_flat_items() {
        let settings: ExplorerSettings = serde_json::from_str(
            r#"{
                "contextmenu": {
                    "file_folder": [
                        {
                            "label": "Inspect file",
                            "exe": "~/bin/file-tool",
                            "only": ["txt"]
                        }
                    ],
                    "directory": [
                        {
                            "label": "Inspect directory",
                            "exe": "~/bin/directory-tool"
                        },
                        {
                            "label": "Inspect png or directory",
                            "exe": "~/bin/png-tool",
                            "only": [".png"]
                        }
                    ]
                }
            }"#,
        )
        .expect("deserialize legacy context menu sections");

        assert_eq!(settings.contextmenu.items.len(), 3);
        assert!(matches!(
            &settings.contextmenu.items[0],
            CustomContextMenuItem::Item { label, only, .. }
                if label == "Inspect file" && only == &vec!["txt".to_owned()]
        ));
        assert!(matches!(
            &settings.contextmenu.items[1],
            CustomContextMenuItem::Item { label, only, .. }
                if label == "Inspect directory" && only == &vec!["*directory".to_owned()]
        ));
        assert!(matches!(
            &settings.contextmenu.items[2],
            CustomContextMenuItem::Item { label, only, .. }
                if label == "Inspect png or directory"
                    && only == &vec![".png".to_owned(), "*directory".to_owned()]
        ));
    }

    #[test]
    fn contextmenu_items_infer_submenu_when_items_exists() {
        let settings: ExplorerSettings = serde_json::from_str(
            r#"{
                "contextmenu": {
                    "directory": [
                        {
                            "kind": "item",
                            "label": "Tools",
                            "exe": "~/bin/ignored",
                            "items": []
                        }
                    ]
                }
            }"#,
        )
        .expect("deserialize inferred submenu");

        assert!(matches!(
            &settings.contextmenu.items[0],
            CustomContextMenuItem::Submenu { items, .. } if items.is_empty()
        ));
    }

    #[test]
    fn contextmenu_items_without_items_require_executable() {
        let error = serde_json::from_str::<ExplorerSettings>(
            r#"{"contextmenu":[{"label":"Missing command"}]}"#,
        )
        .expect_err("missing executable should fail");

        assert!(error.to_string().contains("exe"));
    }

    #[test]
    fn contextmenu_accepts_missing_but_well_formed_executables() {
        let missing_absolute =
            unique_temp_dir("missing-contextmenu-executable").join("missing-tool");
        let settings = ExplorerSettings {
            contextmenu: ContextMenuSettings {
                items: vec![
                    CustomContextMenuItem::Item {
                        label: "Missing absolute".to_owned(),
                        exe: missing_absolute.clone(),
                        icon: None,
                        args: Vec::new(),
                        only: Vec::new(),
                    },
                    CustomContextMenuItem::Item {
                        label: "Missing PATH command".to_owned(),
                        exe: PathBuf::from("definitely-not-an-explorer-test-command"),
                        icon: None,
                        args: Vec::new(),
                        only: Vec::new(),
                    },
                ],
            },
            ..ExplorerSettings::default()
        };

        assert!(validate_settings(&settings).is_ok());
        assert!(
            settings.contextmenu.items[0]
                .resolved_executable()
                .is_none()
        );
        assert!(
            settings.contextmenu.items[1]
                .resolved_executable()
                .is_none()
        );
    }

    #[test]
    fn settings_load_with_missing_contextmenu_executable() {
        let path = unique_temp_dir("missing-contextmenu-load").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{
                "contextmenu": {
                    "directory": [
                        {
                            "kind": "item",
                            "label": "Missing",
                            "executable": "definitely-not-an-explorer-test-command"
                        }
                    ]
                },
                "view": {
                    "show_hidden": true
                }
            }"#,
        )
        .unwrap();

        let settings = load_settings_from_path(&path).unwrap();

        assert!(settings.view.show_hidden);
        assert_eq!(settings.contextmenu.items.len(), 1);
        assert!(
            settings.contextmenu.items[0]
                .resolved_executable()
                .is_none()
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn contextmenu_only_accepts_known_aliases() {
        let settings = ExplorerSettings {
            contextmenu: ContextMenuSettings {
                items: vec![CustomContextMenuItem::Item {
                    label: "Known tool".to_owned(),
                    exe: PathBuf::from("~/bin/known-tool"),
                    icon: None,
                    args: Vec::new(),
                    only: vec![
                        "*video".to_owned(),
                        "*photo".to_owned(),
                        "*image".to_owned(),
                        "*audio".to_owned(),
                        "*Video".to_owned(),
                        "*files".to_owned(),
                        "*folders".to_owned(),
                        "*Files".to_owned(),
                        "*Folders".to_owned(),
                        "*directory".to_owned(),
                        "*file".to_owned(),
                        "*folder".to_owned(),
                    ],
                }],
            },
            ..ExplorerSettings::default()
        };

        assert!(validate_settings(&settings).is_ok());
    }

    #[test]
    fn contextmenu_rejects_empty_labels_relative_subpaths_and_invalid_only_extensions() {
        for json in [
            r#"{"contextmenu":[{"label":"","exe":"~/tool"}]}"#,
            r#"{"contextmenu":[{"label":"Tool","exe":"tools/relative"}]}"#,
            r#"{"contextmenu":[{"label":"Tool","exe":"~/tool","only":[""]}]}"#,
            r#"{"contextmenu":[{"label":"Tool","exe":"~/tool","only":["."]}]}"#,
            r#"{"contextmenu":[{"label":"Tool","exe":"~/tool","only":["folder/txt"]}]}"#,
            r#"{"contextmenu":[{"label":"Tool","exe":"~/tool","only":["*media"]}]}"#,
            r#"{"contextmenu":[{"label":"Tool","exe":"~/tool","only":["*"]}]}"#,
            r#"{"contextmenu":[{"label":"Tool","exe":"~/tool","only":["txt*"]}]}"#,
            r#"{"contextmenu":[{"label":" ","items":[]}]}"#,
        ] {
            let settings: ExplorerSettings = serde_json::from_str(json).unwrap();
            assert!(validate_settings(&settings).is_err());
        }
    }

    #[test]
    fn contextmenu_sync_removes_unknown_fields_recursively() {
        let mut document: Value = serde_json::from_str(
            r#"{
                "contextmenu": {
                    "directory": [
                        {
                            "kind": "submenu",
                            "label": "Tools",
                            "note": "parent",
                            "items": [
                                {
                                    "kind": "item",
                                    "label": "Inspect",
                                    "executable": "~/bin/inspect",
                                    "args": "--line 'two words'",
                                    "note": "child"
                                }
                            ]
                        }
                    ]
                }
            }"#,
        )
        .unwrap();
        let settings: ExplorerSettings = serde_json::from_value(document.clone()).unwrap();

        sync_settings_document(&mut document, &settings);

        assert!(document["contextmenu"].is_array());
        assert_eq!(
            document["contextmenu"][0]["items"][0]["exe"],
            "~/bin/inspect"
        );
        assert_eq!(
            document["contextmenu"][0]["items"][0]["args"],
            serde_json::json!(["--line", "two words"])
        );
        assert_eq!(
            document["contextmenu"][0]["items"][0]["only"],
            serde_json::json!(["*directory"])
        );
        assert!(document["contextmenu"].get("directory").is_none());
        assert!(document["contextmenu"][0].get("note").is_none());
        assert!(document["contextmenu"][0].get("kind").is_none());
        assert!(document["contextmenu"][0]["items"][0].get("note").is_none());
        assert!(document["contextmenu"][0]["items"][0].get("kind").is_none());
        assert!(
            document["contextmenu"][0]["items"][0]
                .get("executable")
                .is_none()
        );
    }

    #[test]
    fn contextmenu_sync_writes_exe_and_only_for_items() {
        let mut document: Value = serde_json::json!({
            "contextmenu": []
        });
        let icon = unique_temp_dir("contextmenu-sync-icon").join("inspect.png");
        let settings = ExplorerSettings {
            contextmenu: ContextMenuSettings {
                items: vec![CustomContextMenuItem::Item {
                    label: "Inspect".to_owned(),
                    exe: PathBuf::from("~/bin/inspect"),
                    icon: Some(icon.clone()),
                    args: Vec::new(),
                    only: vec!["rs".to_owned(), ".toml".to_owned()],
                }],
            },
            ..ExplorerSettings::default()
        };

        sync_settings_document(&mut document, &settings);

        assert_eq!(document["contextmenu"][0]["exe"], "~/bin/inspect");
        assert_eq!(document["contextmenu"][0]["icon"], serde_json::json!(icon));
        assert_eq!(
            document["contextmenu"][0]["only"],
            serde_json::json!(["rs", ".toml"])
        );
        assert!(document["contextmenu"][0].get("args").is_none());
        assert!(document["contextmenu"][0].get("executable").is_none());
        assert!(document["contextmenu"][0].get("kind").is_none());
    }

    #[test]
    fn contextmenu_load_normalizes_away_legacy_kind_fields() {
        let path = unique_temp_dir("normalize-contextmenu-kind").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{
                "contextmenu": {
                    "directory": [
                        {
                            "kind": "submenu",
                            "label": "Tools",
                            "note": "parent",
                            "items": [
                                {
                                    "kind": "item",
                                    "label": "Inspect",
                                    "exe": "~/bin/inspect",
                                    "note": "child"
                                }
                            ]
                        }
                    ]
                }
            }"#,
        )
        .unwrap();

        let loaded = load_settings_document_from_path(&path).unwrap();
        let normalized = fs::read_to_string(&path).unwrap();
        let document: Value = serde_json::from_str(&normalized).unwrap();

        assert!(matches!(
            &loaded.value.contextmenu.items[0],
            CustomContextMenuItem::Submenu { items, .. }
                if matches!(
                    &items[0],
                    CustomContextMenuItem::Item { exe, only, .. }
                        if exe == Path::new("~/bin/inspect")
                            && only == &vec!["*directory".to_owned()]
                )
        ));
        assert!(document["contextmenu"].is_array());
        assert_eq!(
            document["contextmenu"][0]["items"][0]["only"],
            serde_json::json!(["*directory"])
        );
        assert!(document["contextmenu"].get("directory").is_none());
        assert!(document["contextmenu"][0].get("note").is_none());
        assert!(document["contextmenu"][0].get("kind").is_none());
        assert!(document["contextmenu"][0]["items"][0].get("note").is_none());
        assert!(document["contextmenu"][0]["items"][0].get("kind").is_none());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn font_family_resolves_default_and_custom_values() {
        assert_eq!(resolved_font_family(DEFAULT_FONT), SYSTEM_UI_FONT);
        assert_eq!(resolved_font_family("Inter"), "Inter");
    }

    #[test]
    fn empty_font_values_are_rejected_but_unavailable_names_are_valid() {
        for font in ["", " ", "\t\r\n"] {
            let settings = ExplorerSettings {
                view: ViewSettings {
                    font: font.to_owned(),
                    ..ViewSettings::default()
                },
                ..ExplorerSettings::default()
            };
            assert!(validate_settings(&settings).is_err());
        }

        let settings = ExplorerSettings {
            view: ViewSettings {
                font: "Definitely Not An Installed Font".to_owned(),
                ..ViewSettings::default()
            },
            ..ExplorerSettings::default()
        };
        assert!(validate_settings(&settings).is_ok());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn empty_filesystem_name_values_are_rejected() {
        for filesystem_name in ["", " ", "\t\r\n"] {
            let settings = ExplorerSettings {
                view: ViewSettings {
                    filesystem_name: filesystem_name.to_owned(),
                    ..ViewSettings::default()
                },
                ..ExplorerSettings::default()
            };
            assert!(validate_settings(&settings).is_err());
        }
    }

    #[test]
    fn empty_and_literal_date_formats_are_valid() {
        for date_format in ["", "Modified today"] {
            let settings = ExplorerSettings {
                view: ViewSettings {
                    date_format: date_format.to_owned(),
                    ..ViewSettings::default()
                },
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
    fn empty_existing_settings_are_repopulated_with_defaults() {
        let path = unique_temp_dir("empty-settings").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, " \n\t").unwrap();

        let loaded = load_or_create_settings(&path);
        assert_eq!(loaded.value, ExplorerSettings::default());
        assert_eq!(
            loaded.document,
            settings_document(&ExplorerSettings::default())
        );
        assert_eq!(
            load_settings_from_path(&path).unwrap(),
            ExplorerSettings::default()
        );
        assert!(!fs::read_to_string(&path).unwrap().trim().is_empty());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn live_reload_recreates_deleted_and_empty_files_and_rejects_malformed_edits() {
        let path = unique_temp_dir("live-reload").join(SETTINGS_FILE_NAME);
        let defaults = load_settings_after_change(&path).expect("recreate deleted settings");
        assert_eq!(defaults.value, ExplorerSettings::default());
        assert_eq!(load_settings_from_path(&path).unwrap(), defaults.value);

        fs::write(&path, " \n\t").unwrap();
        let defaults = load_settings_after_change(&path).expect("repopulate empty settings");
        assert_eq!(defaults.value, ExplorerSettings::default());
        assert_eq!(load_settings_from_path(&path).unwrap(), defaults.value);

        fs::write(&path, "{ malformed").unwrap();
        assert!(load_settings_after_change(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ malformed");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn valid_partial_settings_are_completed_sorted_and_remove_unknown_fields() {
        let path = unique_temp_dir("normalize").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"view":{"show_hidden":false,"future_view":7},"future_option":{"z":1,"a":2},"show_hidden_files":true}"#,
        )
        .unwrap();

        let loaded = load_settings_document_from_path(&path).unwrap();
        assert!(!loaded.value.view.show_hidden);

        let normalized = fs::read_to_string(&path).unwrap();
        let document: Value = serde_json::from_str(&normalized).unwrap();
        let object = document.as_object().unwrap();
        assert_eq!(
            object.len(),
            settings_document(&loaded.value).as_object().unwrap().len()
        );
        assert!(object.get("future_option").is_none());
        assert!(object.get("show_hidden_files").is_none());
        assert!(object["view"].get("future_view").is_none());
        assert_eq!(object["view"]["mode"], "details");
        assert_eq!(object["view"]["mode_media"], "large_icons");
        assert_eq!(object["view"]["native_icons"], true);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert_eq!(object["view"]["filesystem_name"], DEFAULT_FILESYSTEM_NAME);
        #[cfg(target_os = "windows")]
        assert!(object["view"].get("filesystem_name").is_none());
        assert_eq!(object["view"]["remote_mode_media"], "details");
        assert_eq!(object["view"]["remote_thumbnails"], false);
        assert_eq!(object["view"]["show_extensions"], true);
        assert_eq!(object["view"]["show_dotfiles"], true);
        assert_eq!(object["view"]["sort"]["column"], "name");
        assert_eq!(object["view"]["sort"]["direction"], "ascending");
        assert_eq!(
            object["app"]["cache_cleanup_interval_days"],
            DEFAULT_CACHE_CLEANUP_INTERVAL_DAYS
        );
        assert_eq!(object["app"]["new_window_behaviour"], "focus");
        assert_eq!(
            object["app"]["start"],
            Value::String(format_configured_path(
                &loaded.value.app.start,
                settings_address_slash(&loaded.value),
            ))
        );
        assert!(normalized.find("\"app\"").unwrap() < normalized.find("\"contextmenu\"").unwrap());
        assert!(normalized.find("\"sidebar\"").unwrap() < normalized.find("\"tabs\"").unwrap());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[gpui::test]
    fn app_setting_updates_remove_unknown_fields(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("remove-unknown").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"future_option":{"z":1,"a":2},"view":{"future_view":7,"show_hidden":false}}"#,
        )
        .unwrap();
        let loaded = load_settings_document_from_path(&path).unwrap();
        cx.set_global(SettingsState {
            value: loaded.value,
            document: loaded.document,
            path: path.clone(),
            _watcher: None,
        });

        cx.update(|cx| set_show_hidden(true, cx));

        let document: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(document.get("future_option").is_none());
        assert!(document["view"].get("future_view").is_none());
        assert_eq!(document["view"]["show_hidden"], true);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[gpui::test]
    fn dotfile_setting_updates_remove_unknown_fields(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("dotfiles-remove-unknown").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"future_option":42,"view":{"future_view":7,"show_dotfiles":true}}"#,
        )
        .unwrap();
        let loaded = load_settings_document_from_path(&path).unwrap();
        cx.set_global(SettingsState {
            value: loaded.value,
            document: loaded.document,
            path: path.clone(),
            _watcher: None,
        });

        cx.update(|cx| set_show_dotfiles(false, cx));

        let document: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(document.get("future_option").is_none());
        assert!(document["view"].get("future_view").is_none());
        assert_eq!(document["view"]["show_dotfiles"], false);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[gpui::test]
    fn view_mode_updates_remove_unknown_fields(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("view-mode-remove-unknown").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"future_option":{"z":1,"a":2},"view":{"future_view":7,"mode":"details"}}"#,
        )
        .unwrap();
        let loaded = load_settings_document_from_path(&path).unwrap();
        cx.set_global(SettingsState {
            value: loaded.value,
            document: loaded.document,
            path: path.clone(),
            _watcher: None,
        });

        cx.update(|cx| set_view_mode(FileViewMode::LargeIcons, cx));

        let document: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(document.get("future_option").is_none());
        assert!(document["view"].get("future_view").is_none());
        assert_eq!(document["view"]["mode"], "large_icons");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[gpui::test]
    fn file_sort_updates_remove_unknown_fields(cx: &mut gpui::TestAppContext) {
        let path = unique_temp_dir("file-sort-remove-unknown").join(SETTINGS_FILE_NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"view":{"future_view":7,"sort":{"column":"name","direction":"descending","future_sort":9}}}"#,
        )
        .unwrap();
        let loaded = load_settings_document_from_path(&path).unwrap();
        cx.set_global(SettingsState {
            value: loaded.value,
            document: loaded.document,
            path: path.clone(),
            _watcher: None,
        });

        cx.update(|cx| {
            set_file_sort(
                FileSortSettings {
                    column: FileSortColumn::Size,
                    direction: SortDirection::Ascending,
                },
                cx,
            )
        });

        let document: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(document["view"].get("future_view").is_none());
        assert!(document["view"]["sort"].get("future_sort").is_none());
        assert_eq!(document["view"]["sort"]["column"], "size");
        assert_eq!(document["view"]["sort"]["direction"], "ascending");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn legacy_sidebar_items_load_and_normalize_to_strings() {
        let mut document: Value = serde_json::from_str(
            r#"{"sidebar":{"items":[{"kind":"home","note":"home"},{"kind":"downloads","note":"downloads"}]}}"#,
        )
        .unwrap();
        let mut settings: ExplorerSettings = serde_json::from_value(document.clone()).unwrap();
        assert_eq!(settings.sidebar.items.len(), 2);

        assert_eq!(
            reorder_sidebar_item_in_settings(1, 0, true, &mut settings),
            Some(0)
        );
        sync_settings_document(&mut document, &settings);
        assert!(document["sidebar"]["items"][0].is_string());
        assert!(document["sidebar"]["items"][1].is_string());

        assert_eq!(
            unpin_sidebar_item_in_settings(1, &mut settings),
            Some(default_sidebar_items()[0].clone())
        );
        sync_settings_document(&mut document, &settings);
        assert_eq!(document["sidebar"]["items"].as_array().unwrap().len(), 1);
        assert!(document["sidebar"]["items"][0].is_string());
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
            r#"{"sidebar":{"items":[{"kind":"custom","path":"relative"}]}}"#,
        )
        .unwrap();
        assert!(load_settings_from_path(&relative).is_err());

        let relative_string = dir.join("relative-string.json");
        fs::write(&relative_string, r#"{"sidebar":{"items":["relative"]}}"#).unwrap();
        assert!(load_settings_from_path(&relative_string).is_err());

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
        assert!(validate_configured_path(Path::new(r"~\Downloads")).is_ok());
        assert!(validate_configured_path(Path::new("~other/Downloads")).is_err());
        assert!(validate_configured_path(Path::new("Downloads")).is_err());
    }

    #[test]
    fn contextmenu_executables_accept_absolute_tilde_and_path_items() {
        let absolute = if cfg!(target_os = "windows") {
            Path::new(r"C:\Tools\inspect.exe")
        } else {
            Path::new("/usr/bin/inspect")
        };
        assert!(validate_context_menu_executable(absolute).is_ok());
        assert!(validate_context_menu_executable(Path::new("~/bin/inspect")).is_ok());
        assert!(validate_context_menu_executable(Path::new("tools/inspect")).is_err());
    }

    #[test]
    fn contextmenu_executable_resolves_from_path() {
        let dir = unique_temp_dir("path-executable");
        let tool = dir.join("inspect");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&tool, "").unwrap();
        let path_var = env::join_paths([dir.as_path()]).unwrap();

        let resolved = resolve_context_menu_executable_with(
            Path::new("inspect"),
            ConfigPlatform::Linux,
            |name| match name {
                "PATH" => Some(path_var.clone()),
                _ => None,
            },
            |path| path.is_file(),
        );

        assert_eq!(resolved, Some(tool));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn windows_contextmenu_executable_resolves_with_pathext() {
        let dir = unique_temp_dir("windows-path-executable");
        let tool = dir.join("zed.exe");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&tool, "").unwrap();
        let path_var = env::join_paths([dir.as_path()]).unwrap();

        let resolved = resolve_context_menu_executable_with(
            Path::new("zed"),
            ConfigPlatform::Windows,
            |name| match name {
                "PATH" => Some(path_var.clone()),
                "PATHEXT" => Some(OsString::from(".com;.exe;.cmd")),
                _ => None,
            },
            |path| path.is_file(),
        );

        assert_eq!(resolved, Some(tool));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn windows_contextmenu_icon_path_uses_scoop_shim_target_when_available() {
        let dir = unique_temp_dir("windows-shim-icon");
        let shim_dir = dir.join("shims");
        let app_dir = dir.join("apps").join("zed").join("current").join("bin");
        let command = shim_dir.join("zed.exe");
        let shim = shim_dir.join("zed.shim");
        let target = app_dir.join("zed.exe");
        fs::create_dir_all(&shim_dir).unwrap();
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(&command, "").unwrap();
        fs::write(&target, "").unwrap();
        fs::write(&shim, format!("path = \"{}\"\n", target.display())).unwrap();

        assert_eq!(
            context_menu_executable_icon_path_with(
                &command,
                ConfigPlatform::Windows,
                |path| fs::read_to_string(path),
                |path| path.is_file(),
            ),
            target
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn windows_contextmenu_icon_path_falls_back_for_missing_or_invalid_scoop_shims() {
        let dir = unique_temp_dir("windows-shim-icon-fallback");
        let shim_dir = dir.join("shims");
        let app_dir = dir.join("apps").join("zed").join("current").join("bin");
        let command = shim_dir.join("zed.exe");
        let shim = shim_dir.join("zed.shim");
        let target = app_dir.join("zed.exe");
        fs::create_dir_all(&shim_dir).unwrap();
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(&command, "").unwrap();

        assert_eq!(
            context_menu_executable_icon_path_with(
                &command,
                ConfigPlatform::Windows,
                |path| fs::read_to_string(path),
                |path| path.is_file(),
            ),
            command
        );

        fs::write(&shim, "path = \n").unwrap();
        assert_eq!(
            context_menu_executable_icon_path_with(
                &command,
                ConfigPlatform::Windows,
                |path| fs::read_to_string(path),
                |path| path.is_file(),
            ),
            command
        );

        fs::write(&shim, format!("path = \"{}\"\n", target.display())).unwrap();
        assert_eq!(
            context_menu_executable_icon_path_with(
                &command,
                ConfigPlatform::Windows,
                |path| fs::read_to_string(path),
                |path| path.is_file(),
            ),
            command
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn contextmenu_executable_rejects_missing_path_items_and_relative_subpaths() {
        let path_var = env::join_paths([unique_temp_dir("empty-path").as_path()]).unwrap();

        assert_eq!(
            resolve_context_menu_executable_with(
                Path::new("missing-tool"),
                ConfigPlatform::Linux,
                |name| match name {
                    "PATH" => Some(path_var.clone()),
                    _ => None,
                },
                |_| false,
            ),
            None
        );
        assert_eq!(
            resolve_context_menu_executable_with(
                Path::new("tools/inspect"),
                ConfigPlatform::Linux,
                |_| None,
                |_| true,
            ),
            None
        );
    }

    #[test]
    fn startup_path_uses_existing_custom_directory_and_falls_back_for_missing_one() {
        let dir = unique_temp_dir("startup");
        fs::create_dir_all(&dir).unwrap();
        let state = SettingsState::for_test(ExplorerSettings {
            app: AppSettings {
                start: dir.clone(),
                ..AppSettings::default()
            },
            ..ExplorerSettings::default()
        });
        assert_eq!(state.startup_path(), dir);

        let missing = unique_temp_dir("missing-startup");
        let state = SettingsState::for_test(ExplorerSettings {
            app: AppSettings {
                start: missing.clone(),
                ..AppSettings::default()
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
            sidebar: SidebarSettings {
                items: Vec::new(),
                ..SidebarSettings::default()
            },
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
        assert_eq!(settings.sidebar.items, vec![second, third, first]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pinning_standard_directory_stores_configured_path() {
        let home = crate::explorer::user_home_dir();
        let downloads = crate::explorer::user_downloads_dir(home.as_deref()).unwrap();
        if !downloads.exists() {
            fs::create_dir_all(&downloads).unwrap();
        }

        let mut settings = ExplorerSettings {
            sidebar: SidebarSettings {
                items: Vec::new(),
                ..SidebarSettings::default()
            },
            ..ExplorerSettings::default()
        };

        assert!(pin_sidebar_path_in_settings(
            downloads.clone(),
            0,
            &mut settings
        ));
        assert_eq!(settings.sidebar.items.len(), 1);
        assert_eq!(settings.sidebar.items[0], downloads);
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
        let defaults = default_sidebar_items();
        let home = defaults[0].clone();
        let downloads = defaults[3].clone();
        let mut settings = ExplorerSettings {
            sidebar: SidebarSettings {
                items: vec![home.clone(), missing.clone(), downloads.clone()],
                ..SidebarSettings::default()
            },
            ..ExplorerSettings::default()
        };

        assert_eq!(
            reorder_sidebar_item_in_settings(2, 0, true, &mut settings),
            Some(0)
        );
        assert_eq!(settings.sidebar.items, vec![downloads, home, missing]);
    }

    #[test]
    fn sidebar_unpin_removes_requested_item_and_preserves_order() {
        let dir = unique_temp_dir("unpin-sidebar");
        let first = dir.join("first");
        let second = dir.join("second");
        let defaults = default_sidebar_items();
        let home = defaults[0].clone();
        let downloads = defaults[3].clone();
        let mut settings = ExplorerSettings {
            sidebar: SidebarSettings {
                items: vec![
                    home.clone(),
                    first.clone(),
                    second.clone(),
                    downloads.clone(),
                ],
                ..SidebarSettings::default()
            },
            ..ExplorerSettings::default()
        };

        assert_eq!(
            unpin_sidebar_item_in_settings(1, &mut settings),
            Some(first)
        );
        assert_eq!(settings.sidebar.items, vec![home, second, downloads]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sidebar_unpin_ignores_invalid_indices() {
        let defaults = default_sidebar_items();
        let mut settings = ExplorerSettings {
            sidebar: SidebarSettings {
                items: vec![defaults[0].clone(), defaults[3].clone()],
                ..SidebarSettings::default()
            },
            ..ExplorerSettings::default()
        };
        let original = settings.sidebar.items.clone();

        assert_eq!(unpin_sidebar_item_in_settings(2, &mut settings), None);
        assert_eq!(
            unpin_sidebar_item_in_settings(usize::MAX, &mut settings),
            None
        );
        assert_eq!(settings.sidebar.items, original);
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
            test_config_dir(ConfigPlatform::Windows, &[("USERPROFILE", "profile")]),
            Some(
                PathBuf::from("profile")
                    .join(".config")
                    .join(LINUX_CONFIG_DIR_NAME)
            )
        );
        assert_eq!(
            test_config_dir(ConfigPlatform::Windows, &[("APPDATA", "appdata")]),
            None
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
