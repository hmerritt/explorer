use std::{
    cmp::Ordering,
    ffi::OsStr,
    fs,
    ops::Range,
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Local};
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, FontFallbacks, IntoElement, Pixels, Render,
    SharedString, Styled, Window, div, font, prelude::*, px, rgb, uniform_list,
};

const COLUMN_NAME_WIDTH: f32 = 440.0;
const COLUMN_DATE_WIDTH: f32 = 244.0;
const COLUMN_TYPE_WIDTH: f32 = 202.0;
const COLUMN_SIZE_WIDTH: f32 = 124.0;
const NAVBAR_HEIGHT: f32 = 52.0;
const NAV_ICON_SIZE_PHYSICAL: f32 = 18.0;
const NAV_ICON_ENABLED_COLOR: u32 = 0x1f1f1f;
const NAV_ICON_DISABLED_COLOR: u32 = 0x9a9a9a;
const NAV_BUTTON_HOVER_BG: u32 = 0xe8e8e8;
const NAV_BUTTON_ACTIVE_OPACITY: f32 = 0.7;
const FILEPATH_PC_ICON_FONT: &str = "Segoe MDL2 Assets";
const FILEPATH_PC_ICON_FALLBACK_FONT: &str = "Segoe Fluent Icons";
const HEADER_HEIGHT: f32 = 32.0;
const ROW_HEIGHT: f32 = 28.0;
const EMPTY_FOLDER_TEXT_SIZE: f32 = 12.0;
const EMPTY_FOLDER_TOP_MARGIN: f32 = 20.0;
const EMPTY_FOLDER_MESSAGE: &str = "This folder is empty.";
const KB_BYTES: u64 = 1024;
const MB_BYTES: u64 = KB_BYTES * 1024;
const GB_BYTES: u64 = MB_BYTES * 1024;
const TB_BYTES: u64 = GB_BYTES * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    path: PathBuf,
    name: String,
    is_dir: bool,
    modified: Option<SystemTime>,
    size: Option<u64>,
}

impl FileEntry {
    fn from_path(path: PathBuf) -> Option<Self> {
        let metadata = fs::metadata(&path).ok()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        let is_dir = metadata.is_dir();

        Some(Self {
            path,
            name,
            is_dir,
            modified: metadata.modified().ok(),
            size: (!is_dir).then_some(metadata.len()),
        })
    }

    #[cfg(test)]
    fn test(name: &str, is_dir: bool, size: Option<u64>, modified: Option<SystemTime>) -> Self {
        Self {
            path: PathBuf::from(name),
            name: name.to_owned(),
            is_dir,
            modified,
            size,
        }
    }

    fn type_label(&self) -> String {
        if self.is_dir {
            return "File folder".to_owned();
        }

        let Some(extension) = self.path.extension().and_then(OsStr::to_str) else {
            return "File".to_owned();
        };

        format!("{} File", extension.to_uppercase())
    }
}

pub struct ExplorerView {
    path: PathBuf,
    entries: Vec<FileEntry>,
    selected_path: Option<PathBuf>,
    read_error: Option<String>,
    back_stack: Vec<PathBuf>,
    forward_stack: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryMode {
    Record,
    Preserve,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NavIcon {
    Back,
    Forward,
    Up,
    Refresh,
}

impl NavIcon {
    fn glyph(self) -> &'static str {
        match self {
            Self::Back => "\u{E72B}",
            Self::Forward => "\u{E72A}",
            Self::Up => "\u{E74A}",
            Self::Refresh => "\u{E72C}",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FilepathIcon {
    Pc,
}

impl FilepathIcon {
    fn glyph(self) -> &'static str {
        match self {
            Self::Pc => "\u{E977}",
        }
    }
}

impl ExplorerView {
    pub fn new(initial_path: PathBuf) -> Self {
        let mut view = Self {
            path: initial_path,
            entries: Vec::new(),
            selected_path: None,
            read_error: None,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
        };
        view.reload();
        view
    }

    pub fn reload(&mut self) {
        match load_entries(&self.path) {
            Ok(entries) => {
                self.entries = entries;
                self.read_error = None;
                if let Some(selected_path) = &self.selected_path {
                    if !self
                        .entries
                        .iter()
                        .any(|entry| &entry.path == selected_path)
                    {
                        self.selected_path = None;
                    }
                }
            }
            Err(error) => {
                self.entries.clear();
                self.selected_path = None;
                self.read_error = Some(error.to_string());
            }
        }
    }

    fn navigate_to_directory(&mut self, path: PathBuf, history_mode: HistoryMode) {
        if path == self.path {
            self.reload();
            return;
        }

        if matches!(history_mode, HistoryMode::Record) {
            self.back_stack.push(self.path.clone());
            self.forward_stack.clear();
        }

        self.path = path;
        self.selected_path = None;
        self.read_error = None;
        self.reload();
    }

    fn navigate_back(&mut self) {
        if let Some(path) = self.back_stack.pop() {
            self.forward_stack.push(self.path.clone());
            self.navigate_to_directory(path, HistoryMode::Preserve);
        }
    }

    fn navigate_forward(&mut self) {
        if let Some(path) = self.forward_stack.pop() {
            self.back_stack.push(self.path.clone());
            self.navigate_to_directory(path, HistoryMode::Preserve);
        }
    }

    fn navigate_up(&mut self) {
        if let Some(parent) = self.path.parent().map(Path::to_path_buf) {
            self.navigate_to_directory(parent, HistoryMode::Record);
        }
    }

    fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }

    fn can_go_up(&self) -> bool {
        self.path.parent().is_some()
    }

    fn current_folder_name(&self) -> String {
        self.path
            .file_name()
            .and_then(OsStr::to_str)
            .map(str::to_owned)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    fn handle_entry_click(&mut self, entry: &FileEntry, click_count: usize) {
        self.selected_path = Some(entry.path.clone());
        if click_count >= 2 && entry.is_dir {
            self.navigate_to_directory(entry.path.clone(), HistoryMode::Record);
        }
    }

    fn should_show_empty_folder_message(&self) -> bool {
        self.entries.is_empty() && self.read_error.is_none()
    }

    fn content_branch(&self) -> ExplorerContentBranch {
        if self.read_error.is_some() {
            ExplorerContentBranch::Error
        } else if self.should_show_empty_folder_message() {
            ExplorerContentBranch::Empty
        } else {
            ExplorerContentBranch::List
        }
    }

    fn render_navbar(&self, scale_factor: f32, cx: &mut Context<Self>) -> Div {
        let folder_name = self.current_folder_name();

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(NAVBAR_HEIGHT))
            .w_full()
            .bg(rgb(0xf3f3f3))
            .px(px(10.0))
            .gap(px(10.0))
            .child(nav_button(
                "back",
                NavIcon::Back,
                self.can_go_back(),
                scale_factor,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.navigate_back();
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "forward",
                NavIcon::Forward,
                self.can_go_forward(),
                scale_factor,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.navigate_forward();
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "up",
                NavIcon::Up,
                self.can_go_up(),
                scale_factor,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.navigate_up();
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "refresh",
                NavIcon::Refresh,
                true,
                scale_factor,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.reload();
                    cx.notify();
                }),
            ))
            .child(directory_bar(&folder_name))
    }

    fn render_header(&self) -> Div {
        div()
            .flex()
            .flex_row()
            .h(px(HEADER_HEIGHT))
            .w_full()
            .bg(rgb(0xffffff))
            .border_b_1()
            .border_color(rgb(0xf2f2f2))
            .text_size(px(12.0))
            .text_color(rgb(0x1f4e79))
            .child(header_cell("Name", COLUMN_NAME_WIDTH, true))
            .child(header_cell("Date modified", COLUMN_DATE_WIDTH, false))
            .child(header_cell("Type", COLUMN_TYPE_WIDTH, false))
            .child(header_cell("Size", COLUMN_SIZE_WIDTH, false))
    }

    fn render_row(&self, ix: usize, scale_factor: f32, cx: &mut Context<Self>) -> AnyElement {
        let entry = self.entries[ix].clone();
        let is_selected = self.selected_path.as_ref() == Some(&entry.path);
        let clicked_entry = entry.clone();

        div()
            .id(("explorer-entry", ix))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h(px(ROW_HEIGHT))
            .w_full()
            .bg(if is_selected {
                rgb(0xcce8ff)
            } else {
                rgb(0xffffff)
            })
            .when(!is_selected, |this| {
                this.hover(|style| style.bg(rgb(0xe5f3ff)))
            })
            .border_1()
            .border_color(rgb(0xffffff))
            .cursor_default()
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                this.handle_entry_click(&clicked_entry, event.click_count());
                cx.notify();
            }))
            .child(name_cell(&entry, scale_factor))
            .child(text_cell(
                format_modified(entry.modified),
                COLUMN_DATE_WIDTH,
                false,
            ))
            .child(text_cell(entry.type_label(), COLUMN_TYPE_WIDTH, false))
            .child(text_cell(format_size(entry.size), COLUMN_SIZE_WIDTH, true))
            .into_any_element()
    }
}

impl Render for ExplorerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale_factor = window.scale_factor();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x000000))
            .overflow_hidden()
            .child(self.render_navbar(scale_factor, cx))
            .child(self.render_header())
            .child(
                match self.content_branch() {
                    ExplorerContentBranch::Error => div().child(
                        div()
                            .p_4()
                            .text_size(px(14.0))
                            .text_color(rgb(0x6f1d1d))
                            .child(self.read_error.clone().unwrap_or_default()),
                    ),
                    ExplorerContentBranch::Empty => div().child(render_empty_folder_message()),
                    ExplorerContentBranch::List => div().child(
                        uniform_list(
                            "explorer-entries",
                            self.entries.len(),
                            cx.processor(|this, range: Range<usize>, window, cx| {
                                let scale_factor = window.scale_factor();
                                let mut rows = Vec::with_capacity(range.end - range.start);
                                for ix in range {
                                    rows.push(this.render_row(ix, scale_factor, cx));
                                }
                                rows
                            }),
                        )
                        .h_full(),
                    ),
                }
                .id("explorer-scroll")
                .flex_1()
                .w_full()
                .overflow_y_scroll(),
            )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExplorerContentBranch {
    Error,
    Empty,
    List,
}

pub fn default_start_path() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn load_entries(path: &Path) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter_map(|entry| FileEntry::from_path(entry.path()))
        .collect::<Vec<_>>();

    sort_entries(&mut entries);
    Ok(entries)
}

fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(compare_entries);
}

fn compare_entries(a: &FileEntry, b: &FileEntry) -> Ordering {
    match (a.is_dir, b.is_dir) {
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

fn format_modified(modified: Option<SystemTime>) -> String {
    let Some(modified) = modified else {
        return String::new();
    };

    let local: DateTime<Local> = modified.into();
    local.format("%d/%m/%Y %H:%M").to_string()
}

fn format_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return String::new();
    };

    if size < KB_BYTES {
        return format!("{} bytes", format_u64_with_commas(size));
    }

    let (value, precision, unit) = if size < MB_BYTES {
        (size as f64 / KB_BYTES as f64, 1, "KB")
    } else if size < GB_BYTES {
        (size as f64 / MB_BYTES as f64, 2, "MB")
    } else if size < TB_BYTES {
        (size as f64 / GB_BYTES as f64, 2, "GB")
    } else {
        (size as f64 / TB_BYTES as f64, 2, "TB")
    };

    format!("{} {unit}", format_decimal_with_commas(value, precision))
}

fn format_decimal_with_commas(value: f64, precision: usize) -> String {
    let formatted = format!("{value:.precision$}");
    let Some((integer, fraction)) = formatted.split_once('.') else {
        return format_integer_string_with_commas(&formatted);
    };

    format!(
        "{}.{}",
        format_integer_string_with_commas(integer),
        fraction
    )
}

fn format_u64_with_commas(value: u64) -> String {
    format_integer_string_with_commas(&value.to_string())
}

fn format_integer_string_with_commas(value: &str) -> String {
    let mut formatted = String::with_capacity(value.len() + value.len() / 3);

    for (ix, ch) in value.chars().rev().enumerate() {
        if ix > 0 && ix % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(ch);
    }

    formatted.chars().rev().collect()
}

fn render_empty_folder_message() -> Div {
    div()
        .w_full()
        .mt(px(EMPTY_FOLDER_TOP_MARGIN))
        .text_center()
        .text_size(px(EMPTY_FOLDER_TEXT_SIZE))
        .text_color(rgb(0x9a9a9a))
        .child(EMPTY_FOLDER_MESSAGE)
}

fn device_px(value: f32, scale_factor: f32) -> Pixels {
    px(device_px_value(value, scale_factor))
}

fn device_px_value(value: f32, scale_factor: f32) -> f32 {
    if scale_factor <= 0.0 {
        value
    } else {
        value / scale_factor
    }
}

fn nav_button(
    id: &'static str,
    icon: NavIcon,
    enabled: bool,
    scale_factor: f32,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(34.0))
        .h(px(34.0))
        .rounded(px(4.0))
        .cursor_default()
        .when(enabled, |this| {
            this.hover(|style| style.bg(rgb(NAV_BUTTON_HOVER_BG)))
                .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
                .on_click(on_click)
        })
        .child(
            div()
                .font(nav_icon_font())
                .text_size(device_px(NAV_ICON_SIZE_PHYSICAL, scale_factor))
                .text_color(if enabled {
                    rgb(NAV_ICON_ENABLED_COLOR)
                } else {
                    rgb(NAV_ICON_DISABLED_COLOR)
                })
                .child(icon.glyph()),
        )
        .into_any_element()
}

fn nav_icon_font() -> gpui::Font {
    let mut font = font("Segoe Fluent Icons");
    font.fallbacks = Some(FontFallbacks::from_fonts(vec![
        "Segoe MDL2 Assets".to_owned(),
    ]));
    font
}

fn filepath_icon_font() -> gpui::Font {
    let mut font = font(FILEPATH_PC_ICON_FONT);
    font.fallbacks = Some(FontFallbacks::from_fonts(vec![
        FILEPATH_PC_ICON_FALLBACK_FONT.to_owned(),
    ]));
    font
}

fn filepath_pc_icon() -> Div {
    div()
        .font(filepath_icon_font())
        .text_size(px(18.0))
        .text_color(rgb(0x5b5b5b))
        .child(FilepathIcon::Pc.glyph())
}

fn directory_bar(folder_name: &str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(34.0))
        .flex_1()
        .rounded(px(6.0))
        .bg(rgb(0xffffff))
        .px(px(16.0))
        .gap(px(14.0))
        .text_size(px(15.0))
        .text_color(rgb(0x1f1f1f))
        .child(filepath_pc_icon())
        .child(
            div()
                .text_size(px(20.0))
                .text_color(rgb(0x303030))
                .child("›"),
        )
        .child(SharedString::from(folder_name.to_owned()))
        .child(
            div()
                .text_size(px(20.0))
                .text_color(rgb(0x303030))
                .child("›"),
        )
}

fn header_cell(label: &'static str, width: f32, first: bool) -> Div {
    div()
        .relative()
        .flex()
        .items_start()
        .h_full()
        .w(px(width))
        .flex_shrink_0()
        .pl(px(if first { 36.0 } else { 8.0 }))
        .pt(px(8.0))
        .border_r_1()
        .border_color(rgb(0xe7e7e7))
        .child(label)
}

fn name_cell(entry: &FileEntry, scale_factor: f32) -> Div {
    div()
        .flex()
        .items_center()
        .h_full()
        .w(px(COLUMN_NAME_WIDTH))
        .flex_shrink_0()
        .overflow_hidden()
        .pl(px(16.0))
        .child(if entry.is_dir {
            folder_icon(scale_factor)
        } else {
            file_icon(scale_factor)
        })
        .child(
            div()
                .ml(device_px(8.0, scale_factor))
                .truncate()
                .text_size(px(12.0))
                .child(SharedString::from(entry.name.clone())),
        )
}

fn text_cell(text: String, width: f32, right: bool) -> Div {
    let cell = div()
        .flex()
        .items_center()
        .h_full()
        .w(px(width))
        .flex_shrink_0()
        .overflow_hidden()
        .px(px(8.0))
        .text_size(px(12.0))
        .text_color(rgb(0x595959))
        .child(SharedString::from(text));

    if right {
        cell.justify_end()
    } else {
        cell.justify_start()
    }
}

fn folder_icon(scale_factor: f32) -> Div {
    div()
        .relative()
        .w(device_px(22.0, scale_factor))
        .h(device_px(17.0, scale_factor))
        .flex_shrink_0()
        .child(
            div()
                .absolute()
                .left(device_px(1.0, scale_factor))
                .top(device_px(0.0, scale_factor))
                .w(device_px(9.0, scale_factor))
                .h(device_px(5.0, scale_factor))
                .bg(rgb(0xf5c242)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(0.0, scale_factor))
                .top(device_px(4.0, scale_factor))
                .w(device_px(22.0, scale_factor))
                .h(device_px(13.0, scale_factor))
                .bg(rgb(0xffcc4d)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(0.0, scale_factor))
                .top(device_px(14.0, scale_factor))
                .w(device_px(22.0, scale_factor))
                .h(device_px(3.0, scale_factor))
                .bg(rgb(0xf3b839)),
        )
}

fn file_icon(scale_factor: f32) -> Div {
    div()
        .relative()
        .w(device_px(22.0, scale_factor))
        .h(device_px(17.0, scale_factor))
        .flex_shrink_0()
        .border_1()
        .border_color(rgb(0x9a9a9a))
        .bg(rgb(0xffffff))
        .child(
            div()
                .absolute()
                .right(device_px(0.0, scale_factor))
                .top(device_px(0.0, scale_factor))
                .w(device_px(5.0, scale_factor))
                .h(device_px(5.0, scale_factor))
                .border_l_1()
                .border_b_1()
                .border_color(rgb(0xc8c8c8))
                .bg(rgb(0xf4f4f4)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static TEST_DIR_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = TEST_DIR_ID.fetch_add(1, AtomicOrdering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "universal-explorer-test-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp test directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

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

    #[test]
    fn folders_render_blank_size() {
        assert_eq!(format_size(None), "");
    }

    #[test]
    fn files_below_kilobytes_render_as_bytes() {
        assert_eq!(format_size(Some(0)), "0 bytes");
        assert_eq!(format_size(Some(350)), "350 bytes");
        assert_eq!(format_size(Some(1023)), "1,023 bytes");
    }

    #[test]
    fn kilobytes_render_with_one_decimal_place() {
        assert_eq!(format_size(Some(KB_BYTES)), "1.0 KB");
        assert_eq!(format_size(Some(KB_BYTES + 512)), "1.5 KB");
        assert_eq!(format_size(Some(MB_BYTES - 1)), "1,024.0 KB");
    }

    #[test]
    fn megabytes_gigabytes_and_terabytes_render_with_two_decimal_places() {
        assert_eq!(format_size(Some(MB_BYTES)), "1.00 MB");
        assert_eq!(format_size(Some(MB_BYTES + 512 * KB_BYTES)), "1.50 MB");
        assert_eq!(format_size(Some(GB_BYTES)), "1.00 GB");
        assert_eq!(format_size(Some(GB_BYTES + 512 * MB_BYTES)), "1.50 GB");
        assert_eq!(format_size(Some(TB_BYTES)), "1.00 TB");
        assert_eq!(format_size(Some(TB_BYTES + 512 * GB_BYTES)), "1.50 TB");
    }

    #[test]
    fn large_file_sizes_include_commas_and_stay_capped_at_terabytes() {
        assert_eq!(format_size(Some(1024 * MB_BYTES)), "1.00 GB");
        assert_eq!(format_size(Some(1024 * GB_BYTES)), "1.00 TB");
        assert_eq!(format_size(Some(1024 * TB_BYTES)), "1,024.00 TB");
    }

    #[test]
    fn device_pixel_values_convert_to_logical_pixels() {
        assert_eq!(device_px_value(22.0, 1.0), 22.0);
        assert!((device_px_value(22.0, 1.5) - 14.666_667).abs() < 0.000_01);
        assert!((device_px_value(17.0, 1.5) - 11.333_333).abs() < 0.000_01);
    }

    #[test]
    fn device_pixel_conversion_handles_invalid_scale() {
        assert_eq!(device_px_value(22.0, 0.0), 22.0);
        assert_eq!(device_px_value(22.0, -1.0), 22.0);
    }

    #[test]
    fn nav_icons_use_windows_explorer_glyphs() {
        assert_eq!(NavIcon::Back.glyph(), "\u{E72B}");
        assert_eq!(NavIcon::Forward.glyph(), "\u{E72A}");
        assert_eq!(NavIcon::Up.glyph(), "\u{E74A}");
        assert_eq!(NavIcon::Refresh.glyph(), "\u{E72C}");
    }

    #[test]
    fn filepath_pc_icon_uses_windows_glyph_and_font() {
        assert_eq!(FilepathIcon::Pc.glyph(), "\u{E977}");
        assert_eq!(FILEPATH_PC_ICON_FONT, "Segoe MDL2 Assets");
    }

    #[test]
    fn nav_icon_size_converts_from_physical_pixels() {
        assert_eq!(device_px_value(NAV_ICON_SIZE_PHYSICAL, 1.0), 18.0);
        assert_eq!(device_px_value(NAV_ICON_SIZE_PHYSICAL, 1.5), 12.0);
    }

    #[test]
    fn nav_button_active_opacity_dims_button() {
        assert_eq!(NAV_BUTTON_ACTIVE_OPACITY, 0.7);
        assert!(NAV_BUTTON_ACTIVE_OPACITY < 1.0);
    }

    #[test]
    fn empty_directory_without_error_shows_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("empty"));
        view.entries.clear();
        view.read_error = None;

        assert!(view.should_show_empty_folder_message());
    }

    #[test]
    fn read_error_suppresses_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("missing"));
        view.entries.clear();
        view.read_error = Some("missing".to_owned());

        assert!(!view.should_show_empty_folder_message());
    }

    #[test]
    fn non_empty_directory_suppresses_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("non-empty"));
        view.entries = vec![FileEntry::test("file.txt", false, Some(1), None)];
        view.read_error = None;

        assert!(!view.should_show_empty_folder_message());
    }

    #[test]
    fn content_branch_prioritizes_error_empty_then_list() {
        let mut view = ExplorerView::new(PathBuf::from("branch"));

        view.entries.clear();
        view.read_error = Some("error".to_owned());
        assert_eq!(view.content_branch(), ExplorerContentBranch::Error);

        view.read_error = None;
        assert_eq!(view.content_branch(), ExplorerContentBranch::Empty);

        view.entries = vec![FileEntry::test("file.txt", false, Some(1), None)];
        assert_eq!(view.content_branch(), ExplorerContentBranch::List);
    }

    #[test]
    fn empty_folder_message_uses_compact_text() {
        assert_eq!(EMPTY_FOLDER_TEXT_SIZE, 12.0);
        assert_eq!(EMPTY_FOLDER_TOP_MARGIN, 20.0);
        assert_eq!(EMPTY_FOLDER_MESSAGE, "This folder is empty.");
    }

    #[test]
    fn modified_time_uses_local_explorer_format() {
        let local = Local.with_ymd_and_hms(2026, 5, 31, 21, 48, 12).unwrap();
        assert_eq!(format_modified(Some(local.into())), "31/05/2026 21:48");
    }

    #[test]
    fn navigating_to_valid_directory_updates_path_and_clears_selection() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");
        fs::write(child.join("inside.txt"), b"data").expect("create child file");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.selected_path = Some(child.clone());

        view.navigate_to_directory(child.clone(), HistoryMode::Record);

        assert_eq!(view.path, child);
        assert_eq!(view.selected_path, None);
        assert_eq!(view.read_error, None);
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].name, "inside.txt");
    }

    #[test]
    fn navigating_to_missing_directory_sets_read_error_and_empty_entries() {
        let temp = TempDir::new();
        let missing = temp.path().join("missing");
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.selected_path = Some(temp.path().join("anything"));

        view.navigate_to_directory(missing.clone(), HistoryMode::Record);

        assert_eq!(view.path, missing);
        assert_eq!(view.selected_path, None);
        assert!(view.read_error.is_some());
        assert!(view.entries.is_empty());
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn single_click_selects_without_navigating() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");
        let entry = FileEntry {
            path: child.clone(),
            name: "child".to_owned(),
            is_dir: true,
            modified: None,
            size: None,
        };
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        view.handle_entry_click(&entry, 1);

        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_path, Some(child));
        assert!(view.back_stack.is_empty());
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn double_click_navigates_only_directories() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        let file = temp.path().join("file.txt");
        fs::create_dir_all(&child).expect("create child directory");
        fs::write(&file, b"data").expect("create file");

        let file_entry = FileEntry {
            path: file.clone(),
            name: "file.txt".to_owned(),
            is_dir: false,
            modified: None,
            size: Some(4),
        };
        let dir_entry = FileEntry {
            path: child.clone(),
            name: "child".to_owned(),
            is_dir: true,
            modified: None,
            size: None,
        };
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        view.handle_entry_click(&file_entry, 2);
        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_path, Some(file));
        assert!(view.back_stack.is_empty());
        assert!(view.forward_stack.is_empty());

        view.handle_entry_click(&dir_entry, 2);
        assert_eq!(view.path, child);
        assert_eq!(view.selected_path, None);
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn folder_navigation_records_back_and_clears_forward() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.forward_stack.push(temp.path().join("forward"));

        view.navigate_to_directory(child.clone(), HistoryMode::Record);

        assert_eq!(view.path, child);
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn back_and_forward_move_between_paths() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.navigate_to_directory(child.clone(), HistoryMode::Record);

        view.navigate_back();
        assert_eq!(view.path, temp.path());
        assert!(view.back_stack.is_empty());
        assert_eq!(view.forward_stack, vec![child.clone()]);

        view.navigate_forward();
        assert_eq!(view.path, child);
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn up_navigates_to_parent_and_records_history() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        let grandchild = child.join("grandchild");
        fs::create_dir_all(&grandchild).expect("create nested directories");

        let mut view = ExplorerView::new(grandchild.clone());

        view.navigate_up();

        assert_eq!(view.path, child);
        assert_eq!(view.back_stack, vec![grandchild]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn refresh_preserves_path_and_history() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");
        let back = temp.path().join("back");
        let forward = temp.path().join("forward");

        let mut view = ExplorerView::new(child.clone());
        view.back_stack.push(back.clone());
        view.forward_stack.push(forward.clone());

        view.reload();

        assert_eq!(view.path, child);
        assert_eq!(view.back_stack, vec![back]);
        assert_eq!(view.forward_stack, vec![forward]);
    }
}
