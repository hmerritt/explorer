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
const DEBUG_ALL: u8 = DEBUG_NAV | DEBUG_SEARCH;

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

    fn enable_nav(&mut self) {
        self.flags |= DEBUG_NAV;
    }

    fn enable_search(&mut self) {
        self.flags |= DEBUG_SEARCH;
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
            "all" | "*" => options.enable_all(),
            unknown => warnings.push(format!(
                "Explorer ignoring unknown debug item {unknown:?}; expected nav, search, all, or *."
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
        let (options, warnings) = parse_debug_options(args(&["explorer", "--debug", "nav,search"]));

        assert!(options.nav_timings());
        assert!(options.search_timings());
        assert!(warnings.is_empty());
    }

    #[test]
    fn parses_equals_form() {
        let (options, warnings) = parse_debug_options(args(&["explorer", "--debug=nav"]));

        assert!(options.nav_timings());
        assert!(!options.search_timings());
        assert!(warnings.is_empty());
    }

    #[test]
    fn parses_all_aliases() {
        let (all_options, all_warnings) =
            parse_debug_options(args(&["explorer", "--debug", "all"]));
        let (star_options, star_warnings) = parse_debug_options(args(&["explorer", "--debug=*"]));

        assert!(all_options.nav_timings());
        assert!(all_options.search_timings());
        assert!(star_options.nav_timings());
        assert!(star_options.search_timings());
        assert!(all_warnings.is_empty());
        assert!(star_warnings.is_empty());
    }

    #[test]
    fn unknown_debug_items_warn_without_enabling() {
        let (options, warnings) = parse_debug_options(args(&["explorer", "--debug", "nav,paint"]));

        assert!(options.nav_timings());
        assert!(!options.search_timings());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("paint"));
    }

    #[test]
    fn bare_debug_enables_all_without_warning() {
        let (options, warnings) = parse_debug_options(args(&["explorer", "--debug"]));

        assert!(options.nav_timings());
        assert!(options.search_timings());
        assert!(warnings.is_empty());
    }

    #[test]
    fn empty_equals_debug_enables_all_without_warning() {
        let (options, warnings) = parse_debug_options(args(&["explorer", "--debug="]));

        assert!(options.nav_timings());
        assert!(options.search_timings());
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
