use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

const APPLICATION_NAME: &str = "Explorer";
const APPLICATION_DESCRIPTION: &str =
    "File Explorer for Windows, macOS, and Linux, built with GPUI.";
const PACKAGED_WINDOWS_EXECUTABLE_NAME: &str = "file-explorer.exe";
const PROG_ID: &str = "HMerritt.Explorer.Image.1";
const CAPABILITIES_KEY: &str = r"Software\HMerritt\Explorer\Capabilities";
const REGISTERED_APPLICATIONS_KEY: &str = r"Software\RegisteredApplications";
const REGISTER_FILE_ASSOCIATIONS_ARG: &str = "--register-file-associations";
const UNREGISTER_FILE_ASSOCIATIONS_ARG: &str = "--unregister-file-associations";

const IMAGE_FILE_EXTENSIONS: &[&str] = &[
    ".avif", ".bmp", ".gif", ".ico", ".jfif", ".jpe", ".jpeg", ".jpg", ".png", ".svg", ".tif",
    ".tiff", ".webp",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FileAssociationCommand {
    Register,
    Unregister,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegistryValueKind {
    String,
    None,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistrySetValue {
    key_path: String,
    value_name: Option<String>,
    kind: RegistryValueKind,
    data: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RegistryPlan {
    set_values: Vec<RegistrySetValue>,
    delete_values: Vec<RegistryDeleteValue>,
    delete_trees: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistryDeleteValue {
    key_path: String,
    value_name: String,
}

#[cfg(target_os = "windows")]
pub(crate) fn handle_file_association_command(
    args: impl IntoIterator<Item = OsString>,
) -> std::io::Result<bool> {
    match file_association_command(args) {
        Some(FileAssociationCommand::Register) => {
            let executable_path = std::env::current_exe()?;
            register_file_associations(&executable_path)?;
            println!(
                "Registered Explorer image file associations for {}.",
                executable_path.display()
            );
            Ok(true)
        }
        Some(FileAssociationCommand::Unregister) => {
            unregister_file_associations()?;
            println!("Unregistered Explorer image file associations.");
            Ok(true)
        }
        None => Ok(false),
    }
}

fn file_association_command(
    args: impl IntoIterator<Item = OsString>,
) -> Option<FileAssociationCommand> {
    let mut args = args.into_iter();
    let _program = args.next();
    match args.next().as_ref().map(|arg| arg.to_string_lossy()) {
        Some(arg) if arg == REGISTER_FILE_ASSOCIATIONS_ARG => {
            Some(FileAssociationCommand::Register)
        }
        Some(arg) if arg == UNREGISTER_FILE_ASSOCIATIONS_ARG => {
            Some(FileAssociationCommand::Unregister)
        }
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn register_file_associations(executable_path: &Path) -> std::io::Result<()> {
    apply_registry_plan(&registration_plan(executable_path))?;
    notify_windows_associations_changed();
    Ok(())
}

#[cfg(target_os = "windows")]
fn unregister_file_associations() -> std::io::Result<()> {
    apply_registry_cleanup_plan(&unregistration_plan())?;
    notify_windows_associations_changed();
    Ok(())
}

fn registration_plan(executable_path: &Path) -> RegistryPlan {
    let executable_path = absolute_path_string(executable_path);
    let executable_dir = executable_path
        .parent()
        .map(absolute_path_string)
        .unwrap_or_default();
    let executable_path = registry_path_string(&executable_path);
    let executable_dir = registry_path_string(&executable_dir);
    let app_paths_key = app_paths_key();
    let applications_key = applications_key();
    let prog_id_key = prog_id_key();
    let supported_types_key = format!(r"{applications_key}\SupportedTypes");
    let command = open_command(&executable_path);
    let icon = icon_reference(&executable_path);

    let mut plan = RegistryPlan::default();

    plan.set_default_string(&app_paths_key, &executable_path);
    if !executable_dir.is_empty() {
        plan.set_string(&app_paths_key, "Path", &executable_dir);
    }

    plan.set_string(&applications_key, "FriendlyAppName", APPLICATION_NAME);
    plan.set_default_string(&format!(r"{applications_key}\DefaultIcon"), &icon);
    plan.set_default_string(&format!(r"{applications_key}\shell\open"), "Open");
    plan.set_default_string(&format!(r"{applications_key}\shell\open\command"), &command);

    for extension in IMAGE_FILE_EXTENSIONS {
        plan.set_string(&supported_types_key, extension, "");
    }

    plan.set_default_string(&prog_id_key, "Explorer Image");
    plan.set_string(&prog_id_key, "FriendlyTypeName", "Explorer Image");
    plan.set_default_string(&format!(r"{prog_id_key}\DefaultIcon"), &icon);
    plan.set_default_string(&format!(r"{prog_id_key}\shell\open"), "Open");
    plan.set_default_string(&format!(r"{prog_id_key}\shell\open\command"), &command);

    plan.set_string(CAPABILITIES_KEY, "ApplicationName", APPLICATION_NAME);
    plan.set_string(
        CAPABILITIES_KEY,
        "ApplicationDescription",
        APPLICATION_DESCRIPTION,
    );
    plan.set_string(CAPABILITIES_KEY, "ApplicationIcon", &icon);

    let file_associations_key = format!(r"{CAPABILITIES_KEY}\FileAssociations");
    for extension in IMAGE_FILE_EXTENSIONS {
        plan.set_string(&file_associations_key, extension, PROG_ID);
        plan.set_none(
            &format!(r"Software\Classes\{extension}\OpenWithProgids"),
            PROG_ID,
        );
    }

    plan.set_string(
        REGISTERED_APPLICATIONS_KEY,
        APPLICATION_NAME,
        CAPABILITIES_KEY,
    );

    plan
}

fn unregistration_plan() -> RegistryPlan {
    let mut plan = RegistryPlan::default();

    for extension in IMAGE_FILE_EXTENSIONS {
        plan.delete_values.push(RegistryDeleteValue {
            key_path: format!(r"Software\Classes\{extension}\OpenWithProgids"),
            value_name: PROG_ID.to_owned(),
        });
    }

    plan.delete_values.push(RegistryDeleteValue {
        key_path: REGISTERED_APPLICATIONS_KEY.to_owned(),
        value_name: APPLICATION_NAME.to_owned(),
    });
    plan.delete_trees.push(app_paths_key());
    plan.delete_trees.push(applications_key());
    plan.delete_trees.push(prog_id_key());
    plan.delete_trees
        .push(r"Software\HMerritt\Explorer".to_owned());

    plan
}

impl RegistryPlan {
    fn set_default_string(&mut self, key_path: &str, data: &str) {
        self.set_values.push(RegistrySetValue {
            key_path: key_path.to_owned(),
            value_name: None,
            kind: RegistryValueKind::String,
            data: data.to_owned(),
        });
    }

    fn set_string(&mut self, key_path: &str, value_name: &str, data: &str) {
        self.set_values.push(RegistrySetValue {
            key_path: key_path.to_owned(),
            value_name: Some(value_name.to_owned()),
            kind: RegistryValueKind::String,
            data: data.to_owned(),
        });
    }

    fn set_none(&mut self, key_path: &str, value_name: &str) {
        self.set_values.push(RegistrySetValue {
            key_path: key_path.to_owned(),
            value_name: Some(value_name.to_owned()),
            kind: RegistryValueKind::None,
            data: String::new(),
        });
    }
}

fn app_paths_key() -> String {
    format!(
        r"Software\Microsoft\Windows\CurrentVersion\App Paths\{PACKAGED_WINDOWS_EXECUTABLE_NAME}"
    )
}

fn applications_key() -> String {
    format!(r"Software\Classes\Applications\{PACKAGED_WINDOWS_EXECUTABLE_NAME}")
}

fn prog_id_key() -> String {
    format!(r"Software\Classes\{PROG_ID}")
}

fn absolute_path_string(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn registry_path_string(path: &Path) -> String {
    path.as_os_str().to_string_lossy().replace('/', r"\")
}

fn open_command(executable_path: &str) -> String {
    format!("\"{executable_path}\" \"%1\"")
}

fn icon_reference(executable_path: &str) -> String {
    format!("\"{executable_path}\",0")
}

#[cfg(target_os = "windows")]
fn apply_registry_plan(plan: &RegistryPlan) -> std::io::Result<()> {
    for value in &plan.set_values {
        write_registry_value(value)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn apply_registry_cleanup_plan(plan: &RegistryPlan) -> std::io::Result<()> {
    for value in &plan.delete_values {
        delete_registry_value(value)?;
    }
    for key_path in &plan.delete_trees {
        delete_registry_tree(key_path)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn write_registry_value(value: &RegistrySetValue) -> std::io::Result<()> {
    use windows::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_NONE, REG_OPTION_NON_VOLATILE, REG_SZ,
            RegCloseKey, RegCreateKeyExW, RegSetValueExW,
        },
    };
    use windows::core::PCWSTR;

    let key_path = wide_null_str(&value.key_path);
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    registry_status(status)?;

    let value_name = value.value_name.as_deref().map(wide_null_str);
    let value_name = value_name
        .as_ref()
        .map_or_else(PCWSTR::null, |name| PCWSTR(name.as_ptr()));
    let string_data;
    let none_data = [];
    let (value_type, data) = match value.kind {
        RegistryValueKind::String => {
            string_data = utf16_bytes(&value.data);
            (REG_SZ, string_data.as_slice())
        }
        RegistryValueKind::None => (REG_NONE, none_data.as_slice()),
    };

    let status = unsafe { RegSetValueExW(key, value_name, None, value_type, Some(data)) };
    let close_status = unsafe { RegCloseKey(key) };
    registry_status(status)?;
    if close_status != ERROR_SUCCESS {
        registry_status(close_status)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn delete_registry_value(value: &RegistryDeleteValue) -> std::io::Result<()> {
    use windows::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
        },
    };
    use windows::core::PCWSTR;

    let key_path = wide_null_str(&value.key_path);
    let value_name = wide_null_str(&value.value_name);
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    registry_status(status)?;

    let status = unsafe { RegDeleteValueW(key, PCWSTR(value_name.as_ptr())) };
    let close_status = unsafe { RegCloseKey(key) };
    if status != ERROR_FILE_NOT_FOUND {
        registry_status(status)?;
    }
    if close_status != ERROR_SUCCESS {
        registry_status(close_status)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn delete_registry_tree(key_path: &str) -> std::io::Result<()> {
    use windows::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND},
        System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW},
    };
    use windows::core::PCWSTR;

    let key_path = wide_null_str(key_path);
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key_path.as_ptr())) };
    if matches!(status, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) {
        return Ok(());
    }
    registry_status(status)
}

#[cfg(target_os = "windows")]
fn registry_status(status: windows::Win32::Foundation::WIN32_ERROR) -> std::io::Result<()> {
    use windows::Win32::Foundation::ERROR_SUCCESS;

    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(status.0 as i32))
    }
}

#[cfg(target_os = "windows")]
fn wide_null_str(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn utf16_bytes(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(target_os = "windows")]
fn notify_windows_associations_changed() {
    use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};

    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_registration_plan() -> RegistryPlan {
        registration_plan(Path::new("C:/Program Files/Explorer/file-explorer.exe"))
    }

    fn plan_contains_string(
        plan: &RegistryPlan,
        key_path: &str,
        value_name: Option<&str>,
        data: &str,
    ) -> bool {
        plan.set_values.iter().any(|value| {
            value.key_path == key_path
                && value.value_name.as_deref() == value_name
                && value.kind == RegistryValueKind::String
                && value.data == data
        })
    }

    #[test]
    fn command_parser_recognizes_registration_flags_only_as_first_argument() {
        assert_eq!(
            file_association_command([
                OsString::from("file-explorer.exe"),
                OsString::from(REGISTER_FILE_ASSOCIATIONS_ARG),
            ]),
            Some(FileAssociationCommand::Register)
        );
        assert_eq!(
            file_association_command([
                OsString::from("file-explorer.exe"),
                OsString::from(UNREGISTER_FILE_ASSOCIATIONS_ARG),
            ]),
            Some(FileAssociationCommand::Unregister)
        );
        assert_eq!(
            file_association_command([
                OsString::from("file-explorer.exe"),
                OsString::from("photo.jpg"),
                OsString::from(REGISTER_FILE_ASSOCIATIONS_ARG),
            ]),
            None
        );
    }

    #[test]
    fn registration_plan_uses_unique_windows_executable_name() {
        let plan = test_registration_plan();

        assert!(plan_contains_string(
            &plan,
            r"Software\Microsoft\Windows\CurrentVersion\App Paths\file-explorer.exe",
            None,
            r"C:\Program Files\Explorer\file-explorer.exe"
        ));
        assert!(plan_contains_string(
            &plan,
            r"Software\Classes\Applications\file-explorer.exe",
            Some("FriendlyAppName"),
            APPLICATION_NAME
        ));
        assert!(!format!("{plan:?}").contains(r"Applications\explorer.exe"));
    }

    #[test]
    fn registration_plan_quotes_open_command_and_icon_path() {
        let plan = test_registration_plan();

        assert!(plan_contains_string(
            &plan,
            r"Software\Classes\HMerritt.Explorer.Image.1\shell\open\command",
            None,
            r#""C:\Program Files\Explorer\file-explorer.exe" "%1""#
        ));
        assert!(plan_contains_string(
            &plan,
            r"Software\Classes\Applications\file-explorer.exe\DefaultIcon",
            None,
            r#""C:\Program Files\Explorer\file-explorer.exe",0"#
        ));
    }

    #[test]
    fn registration_plan_claims_expected_image_extensions() {
        let plan = test_registration_plan();

        for extension in [
            ".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".ico", ".tif", ".tiff", ".avif",
            ".svg",
        ] {
            assert!(plan_contains_string(
                &plan,
                r"Software\HMerritt\Explorer\Capabilities\FileAssociations",
                Some(extension),
                PROG_ID
            ));
            assert!(plan.set_values.iter().any(|value| {
                value.key_path == format!(r"Software\Classes\{extension}\OpenWithProgids")
                    && value.value_name.as_deref() == Some(PROG_ID)
                    && value.kind == RegistryValueKind::None
            }));
        }
    }

    #[test]
    fn registration_plan_does_not_write_user_choice() {
        let plan = test_registration_plan();

        assert!(!format!("{plan:?}").contains("UserChoice"));
    }

    #[test]
    fn unregistration_plan_removes_only_explorer_owned_values_and_trees() {
        let plan = unregistration_plan();

        assert!(plan.delete_values.iter().any(|value| {
            value.key_path == REGISTERED_APPLICATIONS_KEY && value.value_name == APPLICATION_NAME
        }));
        assert!(plan.delete_trees.contains(&applications_key()));
        assert!(plan.delete_trees.contains(&prog_id_key()));
        assert!(plan.delete_values.iter().any(|value| value.key_path
            == r"Software\Classes\.jpg\OpenWithProgids"
            && value.value_name == PROG_ID));
        assert!(!format!("{plan:?}").contains("UserChoice"));
        assert!(!format!("{plan:?}").contains(r"Applications\explorer.exe"));
    }
}
