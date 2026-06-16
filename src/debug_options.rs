use std::{
    cell::RefCell,
    ffi::OsString,
    fmt,
    sync::atomic::{AtomicU8, Ordering},
    time::Duration,
};

use gpui::{App, Global};

const DEBUG_NAV: u8 = 1 << 0;
const DEBUG_SEARCH: u8 = 1 << 1;
const DEBUG_ICONS: u8 = 1 << 2;
const DEBUG_ARCHIVE: u8 = 1 << 3;
const DEBUG_ARCHIVE_VERBOSE: u8 = 1 << 4;
const DEBUG_PROPERTIES: u8 = 1 << 5;
const DEBUG_ALL: u8 = DEBUG_NAV | DEBUG_SEARCH | DEBUG_ICONS | DEBUG_ARCHIVE | DEBUG_PROPERTIES;

static PROCESS_DEBUG_FLAGS: AtomicU8 = AtomicU8::new(0);

thread_local! {
    static NAV_TIMING_BATCH: RefCell<Option<NavTimingBatchState>> = const { RefCell::new(None) };
}

struct NavTimingBatchState {
    depth: usize,
    lines: Vec<String>,
}

pub(crate) struct NavTimingBatch {
    active: bool,
}

pub(crate) struct ArchiveTiming {
    _private: (),
}

impl ArchiveTiming {
    pub(crate) fn start(_stage: &'static str, _details: fmt::Arguments<'_>) -> Self {
        Self { _private: () }
    }

    pub(crate) fn ok(&mut self) {
        // Detailed archive summaries replace the legacy stage timer.
    }

    pub(crate) fn cancelled(&mut self) {
        // Detailed archive summaries replace the legacy stage timer.
    }
}

impl Drop for ArchiveTiming {
    fn drop(&mut self) {}
}

impl NavTimingBatch {
    pub(crate) fn start() -> Self {
        if !nav_timings_enabled() {
            return Self { active: false };
        }

        NAV_TIMING_BATCH.with(|batch| {
            let mut batch = batch.borrow_mut();
            if let Some(batch) = batch.as_mut() {
                batch.depth += 1;
            } else {
                *batch = Some(NavTimingBatchState {
                    depth: 1,
                    lines: Vec::new(),
                });
            }
        });

        Self { active: true }
    }
}

impl Drop for NavTimingBatch {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let lines = NAV_TIMING_BATCH.with(|batch| {
            let mut batch = batch.borrow_mut();
            let Some(state) = batch.as_mut() else {
                return Vec::new();
            };

            state.depth -= 1;
            if state.depth > 0 {
                return Vec::new();
            }

            batch.take().map(|state| state.lines).unwrap_or_default()
        });

        for line in lines {
            eprintln!("{line}");
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DebugOptions {
    flags: u8,
}

impl Global for DebugOptions {}

impl DebugOptions {
    pub(crate) fn nav_timings(self) -> bool {
        self.flags & DEBUG_NAV != 0
    }

    pub(crate) fn search_timings(self) -> bool {
        self.flags & DEBUG_SEARCH != 0
    }

    pub(crate) fn icon_timings(self) -> bool {
        self.flags & DEBUG_ICONS != 0
    }

    pub(crate) fn archive_timings(self) -> bool {
        self.flags & DEBUG_ARCHIVE != 0
    }

    pub(crate) fn archive_verbose(self) -> bool {
        self.flags & DEBUG_ARCHIVE_VERBOSE != 0
    }

    pub(crate) fn properties_timings(self) -> bool {
        self.flags & DEBUG_PROPERTIES != 0
    }

    fn enable_nav(&mut self) {
        self.flags |= DEBUG_NAV;
    }

    fn enable_search(&mut self) {
        self.flags |= DEBUG_SEARCH;
    }

    fn enable_icons(&mut self) {
        self.flags |= DEBUG_ICONS;
    }

    fn enable_archive(&mut self) {
        self.flags |= DEBUG_ARCHIVE;
    }

    fn enable_archive_verbose(&mut self) {
        self.flags |= DEBUG_ARCHIVE | DEBUG_ARCHIVE_VERBOSE;
    }

    fn enable_properties(&mut self) {
        self.flags |= DEBUG_PROPERTIES;
    }

    fn enable_all(&mut self) {
        self.flags |= DEBUG_ALL;
    }
}

pub(crate) fn initialize(cx: &mut App, args: impl IntoIterator<Item = OsString>) {
    let (options, warnings) = parse_debug_options(args);
    for warning in warnings {
        eprintln!("{warning}");
    }
    PROCESS_DEBUG_FLAGS.store(options.flags, Ordering::Relaxed);
    cx.set_global(options);
}

pub(crate) fn nav_timings_enabled() -> bool {
    DebugOptions {
        flags: PROCESS_DEBUG_FLAGS.load(Ordering::Relaxed),
    }
    .nav_timings()
}

pub(crate) fn search_timings_enabled() -> bool {
    DebugOptions {
        flags: PROCESS_DEBUG_FLAGS.load(Ordering::Relaxed),
    }
    .search_timings()
}

pub(crate) fn icon_timings_enabled() -> bool {
    DebugOptions {
        flags: PROCESS_DEBUG_FLAGS.load(Ordering::Relaxed),
    }
    .icon_timings()
}

pub(crate) fn archive_timings_enabled() -> bool {
    DebugOptions {
        flags: PROCESS_DEBUG_FLAGS.load(Ordering::Relaxed),
    }
    .archive_timings()
}

pub(crate) fn archive_verbose_enabled() -> bool {
    DebugOptions {
        flags: PROCESS_DEBUG_FLAGS.load(Ordering::Relaxed),
    }
    .archive_verbose()
}

pub(crate) fn properties_timings_enabled() -> bool {
    DebugOptions {
        flags: PROCESS_DEBUG_FLAGS.load(Ordering::Relaxed),
    }
    .properties_timings()
}

#[cfg(feature = "benchmarks")]
pub(crate) fn set_archive_debug_for_benchmark(enabled: bool, verbose: bool) {
    let flags = if verbose {
        DEBUG_ARCHIVE | DEBUG_ARCHIVE_VERBOSE
    } else if enabled {
        DEBUG_ARCHIVE
    } else {
        0
    };
    PROCESS_DEBUG_FLAGS.store(flags, Ordering::Relaxed);
}

pub(crate) fn log_nav_timing(elapsed: Duration, message: fmt::Arguments<'_>) {
    if nav_timings_enabled() {
        let mut line = Some(format!(
            "[nav] {} {message}",
            format_timing_duration(elapsed)
        ));
        let batched = NAV_TIMING_BATCH.with(|batch| {
            let mut batch = batch.borrow_mut();
            let Some(batch) = batch.as_mut() else {
                return false;
            };

            batch.lines.push(line.take().expect("nav timing line"));
            true
        });

        if !batched {
            eprintln!("{}", line.expect("nav timing line"));
        }
    }
}

pub(crate) fn log_recursive_search_timing(
    generation: u64,
    elapsed: Duration,
    message: fmt::Arguments<'_>,
) {
    if search_timings_enabled() {
        eprintln!(
            "[recursive-search:{generation}] {} {message}",
            format_timing_duration(elapsed)
        );
    }
}

pub(crate) fn log_recursive_search_marker(generation: u64, message: fmt::Arguments<'_>) {
    if search_timings_enabled() {
        eprintln!("[recursive-search:{generation}] {:<10} {message}", "-");
    }
}

pub(crate) fn log_icon_timing(message: fmt::Arguments<'_>) {
    if icon_timings_enabled() {
        eprintln!("[file-icons] {message}");
    }
}

pub(crate) fn log_property_timing(elapsed: Duration, message: fmt::Arguments<'_>) {
    if properties_timings_enabled() {
        eprintln!("[properties] {} {message}", format_timing_duration(elapsed));
    }
}

pub(crate) fn log_property_marker(message: fmt::Arguments<'_>) {
    if properties_timings_enabled() {
        eprintln!("[properties] {:<11} {message}", "-");
    }
}

fn format_timing_duration(elapsed: Duration) -> String {
    format!("{:<11.3}ms", elapsed.as_secs_f64() * 1000.0)
}

pub(crate) fn parse_debug_options(
    args: impl IntoIterator<Item = OsString>,
) -> (DebugOptions, Vec<String>) {
    let mut options = DebugOptions::default();
    let mut warnings = Vec::new();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy();
        if arg == "--debug" {
            match args.next() {
                Some(value) => {
                    parse_debug_value(&value.to_string_lossy(), &mut options, &mut warnings)
                }
                None => options.enable_all(),
            }
        } else if let Some(value) = arg.strip_prefix("--debug=") {
            parse_debug_value(value, &mut options, &mut warnings);
        }
    }

    (options, warnings)
}

fn parse_debug_value(value: &str, options: &mut DebugOptions, warnings: &mut Vec<String>) {
    let mut parsed_any = false;
    for item in value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        parsed_any = true;
        match item.to_ascii_lowercase().as_str() {
            "nav" => options.enable_nav(),
            "search" => options.enable_search(),
            "icon" | "icons" => options.enable_icons(),
            "archive" | "extract" => options.enable_archive(),
            "archive-verbose" | "extract-verbose" => options.enable_archive_verbose(),
            "properties" | "property" | "props" => options.enable_properties(),
            "all" | "*" => options.enable_all(),
            unknown => warnings.push(format!(
                "Explorer ignoring unknown debug item {unknown:?}; expected nav, search, icons, archive, archive-verbose, extract, extract-verbose, properties, all, or *."
            )),
        }
    }

    if !parsed_any {
        options.enable_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_comma_separated_debug_items() {
        let (options, warnings) =
            parse_debug_options(args(&["explorer", "--debug", "nav,search,icons"]));

        assert!(options.nav_timings());
        assert!(options.search_timings());
        assert!(options.icon_timings());
        assert!(!options.archive_timings());
        assert!(!options.properties_timings());
        assert!(warnings.is_empty());
    }

    #[test]
    fn parses_equals_form() {
        let (options, warnings) = parse_debug_options(args(&["explorer", "--debug=nav"]));

        assert!(options.nav_timings());
        assert!(!options.search_timings());
        assert!(!options.properties_timings());
        assert!(warnings.is_empty());
    }

    #[test]
    fn parses_icon_debug_items() {
        let (plural_options, plural_warnings) =
            parse_debug_options(args(&["explorer", "--debug=icons"]));
        let (singular_options, singular_warnings) =
            parse_debug_options(args(&["explorer", "--debug=icon"]));

        assert!(plural_options.icon_timings());
        assert!(singular_options.icon_timings());
        assert!(!plural_options.nav_timings());
        assert!(!singular_options.search_timings());
        assert!(!plural_options.properties_timings());
        assert!(!singular_options.properties_timings());
        assert!(plural_warnings.is_empty());
        assert!(singular_warnings.is_empty());
    }

    #[test]
    fn parses_archive_debug_aliases() {
        let (archive_options, archive_warnings) =
            parse_debug_options(args(&["explorer", "--debug=archive"]));
        let (extract_options, extract_warnings) =
            parse_debug_options(args(&["explorer", "--debug=nav,extract"]));

        assert!(archive_options.archive_timings());
        assert!(extract_options.archive_timings());
        assert!(extract_options.nav_timings());
        assert!(!archive_options.nav_timings());
        assert!(!archive_options.properties_timings());
        assert!(archive_warnings.is_empty());
        assert!(extract_warnings.is_empty());
    }

    #[test]
    fn archive_verbose_enables_archive_and_verbose_events() {
        let (options, warnings) =
            parse_debug_options(args(&["explorer", "--debug=archive-verbose"]));

        assert!(options.archive_timings());
        assert!(options.archive_verbose());
        assert!(!options.properties_timings());
        assert!(warnings.is_empty());
    }

    #[test]
    fn parses_properties_debug_items() {
        let (properties_options, properties_warnings) =
            parse_debug_options(args(&["explorer", "--debug=properties"]));
        let (property_options, property_warnings) =
            parse_debug_options(args(&["explorer", "--debug=property"]));
        let (props_options, props_warnings) =
            parse_debug_options(args(&["explorer", "--debug=props"]));

        assert!(properties_options.properties_timings());
        assert!(property_options.properties_timings());
        assert!(props_options.properties_timings());
        assert!(!properties_options.nav_timings());
        assert!(!property_options.search_timings());
        assert!(!props_options.icon_timings());
        assert!(!properties_options.archive_timings());
        assert!(properties_warnings.is_empty());
        assert!(property_warnings.is_empty());
        assert!(props_warnings.is_empty());
    }

    #[test]
    fn parses_all_aliases() {
        let (all_options, all_warnings) =
            parse_debug_options(args(&["explorer", "--debug", "all"]));
        let (star_options, star_warnings) = parse_debug_options(args(&["explorer", "--debug=*"]));

        assert!(all_options.nav_timings());
        assert!(all_options.search_timings());
        assert!(all_options.icon_timings());
        assert!(all_options.archive_timings());
        assert!(all_options.properties_timings());
        assert!(star_options.nav_timings());
        assert!(star_options.search_timings());
        assert!(star_options.icon_timings());
        assert!(star_options.archive_timings());
        assert!(star_options.properties_timings());
        assert!(all_warnings.is_empty());
        assert!(star_warnings.is_empty());
    }

    #[test]
    fn unknown_debug_items_warn_without_enabling() {
        let (options, warnings) = parse_debug_options(args(&["explorer", "--debug", "nav,paint"]));

        assert!(options.nav_timings());
        assert!(!options.search_timings());
        assert!(!options.icon_timings());
        assert!(!options.archive_timings());
        assert!(!options.properties_timings());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("paint"));
        assert!(warnings[0].contains("icons"));
        assert!(warnings[0].contains("archive"));
        assert!(warnings[0].contains("properties"));
    }

    #[test]
    fn bare_debug_enables_all_without_warning() {
        let (options, warnings) = parse_debug_options(args(&["explorer", "--debug"]));

        assert!(options.nav_timings());
        assert!(options.search_timings());
        assert!(options.icon_timings());
        assert!(options.archive_timings());
        assert!(options.properties_timings());
        assert!(warnings.is_empty());
    }

    #[test]
    fn empty_equals_debug_enables_all_without_warning() {
        let (options, warnings) = parse_debug_options(args(&["explorer", "--debug="]));

        assert!(options.nav_timings());
        assert!(options.search_timings());
        assert!(options.icon_timings());
        assert!(options.archive_timings());
        assert!(options.properties_timings());
        assert!(warnings.is_empty());
    }

    #[test]
    fn timing_duration_formats_with_unit_at_end_of_padding() {
        assert_eq!(
            format_timing_duration(Duration::from_micros(34_098)),
            "34.098     ms"
        );
    }

    #[test]
    fn timing_duration_formats_sub_millisecond_values_as_milliseconds() {
        assert_eq!(
            format_timing_duration(Duration::from_micros(123)),
            "0.123      ms"
        );
    }

    #[test]
    fn timing_duration_formats_large_values_with_ms_unit() {
        assert_eq!(
            format_timing_duration(Duration::from_millis(12_345)),
            "12345.000  ms"
        );
    }
}
