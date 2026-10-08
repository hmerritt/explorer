use super::{
    Scenario,
    fixtures::Fixtures,
    report::{Sample, WorkerResult},
};
use crate::explorer::{
    ExplorerTabs,
    benchmark_support::{self as adapter, UiSnapshot},
};
use gpui::{
    AnyWindowHandle, App, BenchmarkFrame, Bounds, Entity, ParentElement, Pixels, Point, Styled,
    Window, canvas, point, px,
};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    fs,
    io::Write,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

pub(super) const READY_LINE: &str = "EXPLORER_BENCH_READY";
pub(super) const ACTION_TIMEOUT: Duration = Duration::from_secs(30);
const SCROLL_EVENTS: usize = 120;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct WorkerRequest {
    pub scenario: Scenario,
    pub fixture_root: PathBuf,
    pub output: PathBuf,
    pub scroll_seconds: f64,
}

thread_local! { static ACTIVE: RefCell<Weak<RefCell<Driver>>> = const { RefCell::new(Weak::new()) }; }

fn active() -> Option<Rc<RefCell<Driver>>> {
    ACTIVE.with(|slot| slot.borrow().upgrade())
}

pub(crate) fn track_entry_bounds(
    element: gpui::Stateful<gpui::Div>,
    path: &Path,
) -> gpui::Stateful<gpui::Div> {
    if active().is_none() {
        return element;
    }
    let path = path.to_path_buf();
    element.child(
        canvas(
            |bounds, _, _| bounds,
            move |_, bounds, _, _| {
                if let Some(driver) = active() {
                    driver
                        .borrow_mut()
                        .entry_bounds
                        .insert(path.clone(), bounds);
                }
            },
        )
        .absolute()
        .size_full(),
    )
}

pub(crate) fn entry_position(path: &Path) -> Option<Point<Pixels>> {
    active()?
        .borrow()
        .entry_bounds
        .get(path)
        .map(|bounds| point(bounds.origin.x + px(8.), bounds.origin.y + px(8.)))
}

pub(crate) fn visible_entries() -> Vec<PathBuf> {
    active()
        .map(|driver| driver.borrow().entry_bounds.keys().cloned().collect())
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
struct Expected {
    path: PathBuf,
    generation: Option<u64>,
    entries: usize,
    tabs: usize,
    tab: Option<u64>,
    selected: Option<Vec<usize>>,
    sort: Option<String>,
    query: Option<String>,
    recursive: bool,
    media: Option<String>,
    image: bool,
    scroll_direction: Option<bool>, // true = down
    scroll_start: f32,
}

impl Expected {
    fn directory(path: PathBuf, entries: usize) -> Self {
        Self {
            path,
            generation: None,
            entries,
            tabs: 1,
            tab: None,
            selected: None,
            sort: None,
            query: None,
            recursive: false,
            media: None,
            image: false,
            scroll_direction: None,
            scroll_start: 0.,
        }
    }
    fn matches(&self, state: &UiSnapshot) -> bool {
        !state.loading
            && state.error.is_none()
            && state.path == self.path
            && self.generation == Some(state.generation)
            && state.entries == self.entries
            && state.tabs == self.tabs
            && self.tab.is_none_or(|tab| state.tab == tab)
            && self
                .selected
                .as_ref()
                .is_none_or(|indices| &state.selected == indices)
            && self
                .sort
                .as_ref()
                .is_none_or(|sort| &state.sort == sort && state.sorted)
            && self.query.as_ref().is_none_or(|query| {
                &state.query == query
                    && !state.searching
                    && (!self.recursive || state.recursive_results)
            })
            && self.scroll_direction.is_none_or(|down| {
                if down {
                    state.scroll_top > self.scroll_start + 1.
                } else {
                    state.scroll_top < self.scroll_start - 1.
                }
            })
    }
}

#[derive(Clone, Debug)]
struct Step {
    operation: String,
    target: Option<PathBuf>,
    expected: Expected,
    measured: bool,
}
impl Step {
    fn scroll_workload(&self) -> bool {
        matches!(self.operation.as_str(), "scroll_down" | "scroll_up")
    }
    fn new(operation: &str, path: PathBuf, entries: usize, measured: bool) -> Self {
        Self {
            operation: operation.into(),
            target: None,
            expected: Expected::directory(path, entries),
            measured,
        }
    }
    fn navigate(path: PathBuf, entries: usize, measured: bool) -> Self {
        let mut step = Self::new("navigate", path.clone(), entries, measured);
        step.target = Some(path);
        step
    }
}

struct Pending {
    step: Step,
    tag: u64,
    started: Instant,
    frames: Vec<BenchmarkFrame>,
    scroll_events: usize,
    refresh_requested: bool,
}

struct Driver {
    request: WorkerRequest,
    steps: VecDeque<Step>,
    pending: Option<Pending>,
    next_tag: u64,
    frame_queue: VecDeque<BenchmarkFrame>,
    entry_bounds: HashMap<PathBuf, Bounds<Pixels>>,
    image_window: Option<AnyWindowHandle>,
    error: Option<String>,
    result: WorkerResult,
    ready_sent: bool,
    finished: bool,
}

fn program(request: &WorkerRequest) -> VecDeque<Step> {
    let fixture = Fixtures {
        root: request.fixture_root.clone(),
    };
    let scenario = &request.scenario;
    let flow = scenario.flow.as_str();
    let base = fixture.mixed(scenario.entries);
    let mut initial = base.clone();
    let mut initial_count = scenario.entries;
    let media_folder = match flow {
        "image_viewer" => Some("viewer"),
        "hover_image" => Some("image"),
        "hover_video" => Some("video"),
        "hover_text" => Some("text"),
        "hover_pdf" => Some("pdf"),
        "hover_epub" => Some("epub"),
        _ => None,
    };
    if let Some(folder) = media_folder {
        initial = fixture.media(folder);
        initial_count = 1;
    }
    // video fixtures keep their completion marker alongside the media file.
    let mut steps = VecDeque::from([Step::new(
        "idle",
        initial.clone(),
        initial_count,
        flow == "startup",
    )]);
    if flow == "startup" {
        return steps;
    }
    let child = base.join("child");
    let mut measured = Step::new(flow, initial.clone(), initial_count, true);
    match flow {
        "open" => {
            measured.target = Some(child.clone());
            measured.expected = Expected::directory(child, 24);
        }
        "back" | "up" => steps.push_back(Step::navigate(child, 24, false)),
        "forward" => {
            steps.push_back(Step::navigate(child.clone(), 24, false));
            steps.push_back(Step::new("back", base.clone(), scenario.entries, false));
            measured.expected = Expected::directory(child, 24);
        }
        flow if flow.starts_with("sort_") => {
            let parts = flow.split('_').collect::<Vec<_>>();
            let column = match parts[1] {
                "name" => "Name",
                "date" => "DateModified",
                "type" => "Type",
                _ => "Size",
            };
            let descending = parts[2] == "desc";
            if descending && parts[1] != "name" || !descending && parts[1] == "name" {
                steps.push_back(Step::new(flow, base.clone(), scenario.entries, false));
            }
            measured.expected.sort = Some(format!(
                "{column}/{}",
                if descending {
                    "Descending"
                } else {
                    "Ascending"
                }
            ));
        }
        "filter" | "recursive_search" => {
            measured.expected.entries = scenario.entries.saturating_sub(1) / 10
                + if flow == "recursive_search" { 3 } else { 0 };
            measured.expected.query = Some("needle".into());
            measured.expected.recursive = flow == "recursive_search";
        }
        "single_selection" => measured.expected.selected = Some(vec![0]),
        "range_selection" => {
            steps.push_back(Step::new(
                "single_selection",
                base.clone(),
                scenario.entries,
                false,
            ));
            measured.expected.selected = Some((0..10).collect());
        }
        "select_all" => measured.expected.selected = Some((0..scenario.entries).collect()),
        "new_tab" => {
            measured.expected.tabs = 2;
            measured.expected.tab = Some(2);
        }
        "switch_tab" | "close_tab" => {
            let mut prep = Step::new("new_tab", base.clone(), scenario.entries, false);
            prep.expected.tabs = 2;
            prep.expected.tab = Some(2);
            steps.push_back(prep);
            measured.expected.tabs = if flow == "switch_tab" { 2 } else { 1 };
            measured.expected.tab = Some(1);
        }
        "scroll_down" | "scroll_up" => {
            if flow == "scroll_up" {
                let mut prep = Step::new("scroll_prepare", base.clone(), scenario.entries, false);
                prep.expected.scroll_direction = Some(true);
                steps.push_back(prep);
            }
            measured.expected.scroll_direction = Some(flow == "scroll_down");
        }
        "image_thumbnails" | "video_thumbnails" => {
            let folder = fixture.media(flow);
            measured = Step::navigate(folder.clone(), 12, true);
            measured.expected.media = Some(flow.into());
            if scenario.cache == "primed" {
                let mut prep = measured.clone();
                prep.measured = false;
                steps.push_back(prep);
                steps.push_back(Step::navigate(base.clone(), scenario.entries, false));
            }
        }
        "image_viewer" => {
            measured.expected.image = true;
            if scenario.cache == "primed" {
                let mut prep = measured.clone();
                prep.measured = false;
                steps.push_back(prep);
                steps.push_back(Step::new("close_image", initial.clone(), 1, false));
            }
        }
        flow if flow.starts_with("hover_") => {
            measured.operation = "hover".into();
            measured.expected.media = Some(flow.into());
            if scenario.cache == "primed" {
                let mut prep = measured.clone();
                prep.measured = false;
                steps.push_back(prep);
                steps.push_back(Step::new("clear_hover", initial.clone(), 1, false));
            }
        }
        _ => {}
    }
    steps.push_back(measured);
    steps
}

pub(super) fn prepare(request: &WorkerRequest, config: &Path) -> Result<(), String> {
    let mut settings = crate::settings::ExplorerSettings::default();
    settings.app.start = program(request).front().unwrap().expected.path.clone();
    #[cfg(target_os = "windows")]
    {
        settings.app.tray = false;
    }
    settings.updater.enabled = false;
    settings.view.mode = if request.scenario.view == "large_icons" {
        crate::settings::FileViewMode::LargeIcons
    } else {
        crate::settings::FileViewMode::Details
    };
    settings.view.mode_media = settings.view.mode;
    settings.view.show_dotfiles = false;
    settings.view.show_hidden = false;
    settings.sidebar.pinned = vec![settings.app.start.clone()];
    settings.sidebar.remote.clear();
    settings.sidebar.hide_groups = vec![
        crate::settings::SidebarGroupKind::Drives,
        crate::settings::SidebarGroupKind::Network,
        crate::settings::SidebarGroupKind::Wsl,
    ];
    fs::write(
        config.join("settings.json"),
        serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

pub(super) fn run(request: WorkerRequest) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("workers require the release profile".into());
    }
    let config = super::config_root().ok_or("worker requires an isolated configuration root")?;
    if config.parent() != Some(request.output.as_path()) || !config.join("settings.json").is_file()
    {
        return Err(
            "worker configuration must be prepared inside its sample output directory".into(),
        );
    }
    if !super::scenarios(true).contains(&request.scenario)
        || !request.scroll_seconds.is_finite()
        || !(0.1..=10.).contains(&request.scroll_seconds)
    {
        return Err("invalid worker scenario or scroll duration".into());
    }
    let driver = Rc::new(RefCell::new(Driver {
        steps: program(&request),
        request,
        pending: None,
        next_tag: 1,
        frame_queue: VecDeque::new(),
        entry_bounds: HashMap::new(),
        image_window: None,
        error: None,
        result: WorkerResult::default(),
        ready_sent: false,
        finished: false,
    }));
    ACTIVE.with(|slot| *slot.borrow_mut() = Rc::downgrade(&driver));
    let initial_path = driver.borrow().steps.front().unwrap().expected.path.clone();
    crate::app::run_benchmark(initial_path, {
        let driver = driver.clone();
        move |tabs, window, cx| install(driver, tabs, window, cx)
    });
    if !driver.borrow().finished {
        return Err("application exited before scenario completed".into());
    }
    if let Some(error) = &driver.borrow().result.error {
        return Err(error.clone());
    }
    Ok(())
}

fn install(
    driver: Rc<RefCell<Driver>>,
    tabs: Entity<ExplorerTabs>,
    window: &mut Window,
    cx: &mut App,
) {
    {
        let mut state = driver.borrow_mut();
        state.result.scale_factor = Some(window.scale_factor());
        let size = window.viewport_size();
        state.result.viewport = Some([size.width.into(), size.height.into()]);
        state.result.display_backend = format!("{} {}", std::env::consts::OS, cx.compositor_name())
            .trim()
            .into();
    }
    let before = Rc::downgrade(&driver);
    let captured_tabs = tabs.clone();
    window.observe_benchmark_frames(
        move |_, cx| {
            let Some(driver) = before.upgrade() else {
                return 0;
            };
            // Read state without holding a Driver borrow: media checks consult recorded bounds.
            let expected = driver.borrow().pending.as_ref().map(|p| {
                (
                    p.step.expected.clone(),
                    p.tag,
                    p.scroll_events,
                    p.step.scroll_workload(),
                )
            });
            let Some((expected, tag, events, scroll_workload)) = expected else {
                return 0;
            };
            let snapshot = adapter::snapshot(&captured_tabs, cx, expected.sort.is_some());
            let mut ready = !expected.image && expected.matches(&snapshot);
            if scroll_workload && events < SCROLL_EVENTS {
                ready = false;
            }
            if ready && let Some(media) = &expected.media {
                match adapter::media_ready(&captured_tabs, media, cx) {
                    Ok(value) => ready = value,
                    Err(error) => {
                        driver.borrow_mut().error = Some(error);
                        ready = false;
                    }
                }
            }
            driver.borrow_mut().entry_bounds.clear();
            if ready { tag } else { 0 }
        },
        submission_callback(Rc::downgrade(&driver), false),
    );
    start_step(&driver, &tabs, window, cx);
    schedule(driver, tabs, window, cx);
}

fn submission_callback(driver: Weak<RefCell<Driver>>, image: bool) -> impl FnMut(BenchmarkFrame) {
    move |frame| {
        let Some(driver) = driver.upgrade() else {
            return;
        };
        let mut state = driver.borrow_mut();
        if state
            .pending
            .as_ref()
            .is_some_and(|p| p.step.expected.image == image)
        {
            if frame.tag == 1 && frame.drew_scene && !state.ready_sent {
                state.ready_sent = true;
                println!("{READY_LINE}");
                let _ = std::io::stdout().flush();
            }
            state.frame_queue.push_back(frame);
        }
    }
}

pub(crate) fn observe_image_window(
    window: &mut Window,
    ready: impl Fn(&App) -> Result<bool, String> + 'static,
) {
    let Some(driver) = active() else {
        return;
    };
    driver.borrow_mut().image_window = Some(window.window_handle());
    let before = Rc::downgrade(&driver);
    window.observe_benchmark_frames(
        move |_, cx| {
            let Some(driver) = before.upgrade() else {
                return 0;
            };
            let pending = driver
                .borrow()
                .pending
                .as_ref()
                .map(|p| (p.step.expected.image, p.tag));
            if let Some((true, tag)) = pending {
                match ready(cx) {
                    Ok(true) => return tag,
                    Ok(false) => {}
                    Err(error) => driver.borrow_mut().error = Some(error),
                }
            }
            0
        },
        submission_callback(Rc::downgrade(&driver), true),
    );
}

fn start_step(
    driver: &Rc<RefCell<Driver>>,
    tabs: &Entity<ExplorerTabs>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(mut step) = driver.borrow_mut().steps.pop_front() else {
        return;
    };
    let old = adapter::snapshot(tabs, cx, false);
    step.expected.scroll_start = old.scroll_top;
    let operation = step.operation.clone();
    let target = step.target.clone();
    let tag = driver.borrow().next_tag;
    {
        let mut state = driver.borrow_mut();
        state.next_tag += 1;
        state.pending = Some(Pending {
            step,
            tag,
            started: Instant::now(),
            frames: Vec::new(),
            scroll_events: 0,
            refresh_requested: false,
        });
    }
    let result = match operation.as_str() {
        "scroll_down" | "scroll_up" => Ok(()),
        "scroll_prepare" => adapter::scroll(tabs, -2400., window, cx),
        "close_image" => {
            let handle = driver.borrow_mut().image_window.take();
            if let Some(handle) = handle {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
            window.activate_window();
            Ok(())
        }
        _ => adapter::apply(tabs, &operation, target.as_deref(), window, cx),
    };
    let current = adapter::snapshot(tabs, cx, false);
    if let Some(pending) = &mut driver.borrow_mut().pending {
        pending.step.expected.generation = Some(current.generation);
    }
    if let Err(error) = result {
        driver.borrow_mut().error = Some(error);
    }
    window.refresh();
}

fn schedule(driver: Rc<RefCell<Driver>>, tabs: Entity<ExplorerTabs>, window: &Window, cx: &App) {
    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
            let _ = cx.update(|window, cx| tick(driver, tabs, window, cx));
        })
        .detach();
}

fn tick(
    driver: Rc<RefCell<Driver>>,
    tabs: Entity<ExplorerTabs>,
    window: &mut Window,
    cx: &mut App,
) {
    if driver.borrow().finished {
        return;
    }
    let frames = std::mem::take(&mut driver.borrow_mut().frame_queue);
    let mut completed = false;
    for frame in frames {
        let mut state = driver.borrow_mut();
        if let Some(pending) = &mut state.pending {
            if frame.submitted_at < pending.started {
                continue;
            }
            let matches = frame_completes(pending.tag, pending.started, &frame);
            pending.frames.push(frame);
            if matches {
                completed = true;
                break;
            }
        }
    }
    if completed {
        let pending = driver.borrow_mut().pending.take().unwrap();
        if pending.step.measured {
            let sample = sample(&pending);
            driver.borrow_mut().result.sample = Some(sample);
            finish(&driver, window, cx);
            return;
        }
        start_step(&driver, &tabs, window, cx);
    }
    let timed_out = driver
        .borrow()
        .pending
        .as_ref()
        .is_some_and(|p| action_timed_out(p.started, Instant::now()));
    let snapshot = adapter::snapshot(&tabs, cx, false);
    if let Some(error) = &snapshot.error {
        driver.borrow_mut().error = Some(error.clone());
    }
    if timed_out {
        driver.borrow_mut().error =
            Some("action timeout (30 seconds): expected state was not submitted".into());
        let _ = fs::write(
            driver.borrow().request.output.join("observed-state.txt"),
            format!("{snapshot:?}"),
        );
    }
    if driver.borrow().error.is_some() {
        finish(&driver, window, cx);
        return;
    }
    let scroll = {
        let mut state = driver.borrow_mut();
        let seconds = state.request.scroll_seconds;
        state.pending.as_mut().and_then(|p| {
            if !["scroll_down", "scroll_up"].contains(&p.step.operation.as_str()) {
                return None;
            }
            let due = ((p.started.elapsed().as_secs_f64() / seconds).min(1.) * SCROLL_EVENTS as f64)
                .floor() as usize;
            let count = due.saturating_sub(p.scroll_events);
            p.scroll_events += count;
            Some((
                count,
                if p.step.operation == "scroll_down" {
                    -20.
                } else {
                    20.
                },
            ))
        })
    };
    if let Some((count, delta)) = scroll {
        for _ in 0..count {
            if let Err(error) = adapter::scroll(&tabs, delta, window, cx) {
                driver.borrow_mut().error = Some(error);
                break;
            }
        }
    }
    // A load that preserves unchanged rows may not invalidate the scene. Request
    // exactly one final repaint once the awaited state becomes ready.
    let should_refresh = driver.borrow().pending.as_ref().is_some_and(|p| {
        !p.step.expected.image
            && !p.refresh_requested
            && p.step.expected.matches(&snapshot)
            && (!p.step.scroll_workload() || p.scroll_events == SCROLL_EVENTS)
    });
    if should_refresh {
        if let Some(p) = &mut driver.borrow_mut().pending {
            p.refresh_requested = true;
        }
        window.refresh();
    }
    schedule(driver, tabs, window, cx);
}

fn frame_completes(tag: u64, started: Instant, frame: &BenchmarkFrame) -> bool {
    frame.tag == tag && frame.drew_scene && frame.submitted_at >= started
}

fn action_timed_out(started: Instant, now: Instant) -> bool {
    now.duration_since(started) >= ACTION_TIMEOUT
}

fn sample(pending: &Pending) -> Sample {
    let first_interval = pending
        .frames
        .iter()
        .position(|frame| frame.submitted_at >= pending.started);
    Sample {
        action_to_submission_ms: pending.frames.last().map_or(0., |f| {
            f.submitted_at.duration_since(pending.started).as_secs_f64() * 1000.
        }),
        cpu_frame_work_ms: pending
            .frames
            .iter()
            .filter(|f| f.drew_scene)
            .map(|f| f.cpu_draw.as_secs_f64() * 1000.)
            .collect(),
        renderer_submission_ms: pending
            .frames
            .iter()
            .map(|f| f.renderer_submission.as_secs_f64() * 1000.)
            .collect(),
        // The first interval crosses the measurement boundary and must be omitted.
        submission_intervals_ms: pending
            .frames
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != first_interval)
            .filter_map(|(_, f)| f.submission_interval.map(|v| v.as_secs_f64() * 1000.))
            .collect(),
        input_events: pending.scroll_events,
    }
}

fn finish(driver: &Rc<RefCell<Driver>>, window: &mut Window, cx: &mut App) {
    let mut state = driver.borrow_mut();
    state.result.error = state.error.take();
    state.finished = true;
    if state.result.error.is_some()
        && let Some(pending) = &state.pending
    {
        let _ = fs::write(
            state.request.output.join("partial-sample.json"),
            serde_json::to_vec_pretty(&sample(pending)).unwrap(),
        );
        let _ = fs::write(
            state.request.output.join("expected-state.txt"),
            format!("{:?}", pending.step.expected),
        );
    }
    match serde_json::to_vec_pretty(&state.result)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            fs::write(state.request.output.join("result.json"), bytes).map_err(|e| e.to_string())
        }) {
        Ok(()) => {}
        Err(error) => {
            state.result.error = Some(error);
        }
    }
    window.remove_benchmark_frame_observer();
    cx.quit();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> UiSnapshot {
        UiSnapshot {
            path: PathBuf::from("fixture"),
            generation: 7,
            loading: false,
            entries: 100,
            selected: vec![],
            tab: 1,
            tabs: 1,
            query: String::new(),
            searching: false,
            recursive_results: false,
            sort: "Name/Ascending".into(),
            sorted: true,
            scroll_top: 0.,
            error: None,
        }
    }
    #[test]
    fn completion_rejects_loading_wrong_generation_and_wrong_outcomes() {
        let mut expected = Expected::directory(PathBuf::from("fixture"), 100);
        expected.generation = Some(7);
        let mut state = snapshot();
        assert!(expected.matches(&state));
        state.generation = 6;
        assert!(!expected.matches(&state));
        state.generation = 8;
        assert!(!expected.matches(&state));
        state.generation = 7;
        state.loading = true;
        assert!(!expected.matches(&state));
        state.loading = false;
        state.entries = 99;
        assert!(!expected.matches(&state));
        state.entries = 100;
        expected.selected = Some(vec![0, 1]);
        assert!(!expected.matches(&state));
        state.selected = vec![0, 1];
        assert!(expected.matches(&state));
    }
    #[test]
    fn completed_frames_must_have_captured_the_action_tag_before_drawing() {
        let now = Instant::now();
        let mut frame = BenchmarkFrame {
            tag: 3,
            submitted_at: now,
            cpu_draw: Duration::ZERO,
            renderer_submission: Duration::ZERO,
            submission_interval: None,
            drew_scene: true,
        };
        assert!(frame_completes(3, now, &frame));
        assert!(!frame_completes(4, now, &frame));
        frame.drew_scene = false;
        assert!(!frame_completes(3, now, &frame));
        frame.drew_scene = true;
        assert!(!frame_completes(3, now + Duration::from_millis(1), &frame));
    }
    #[test]
    fn action_timeout_has_a_precise_boundary_without_sleeping() {
        let start = Instant::now();
        assert!(!action_timed_out(
            start,
            start + ACTION_TIMEOUT - Duration::from_nanos(1)
        ));
        assert!(action_timed_out(start, start + ACTION_TIMEOUT));
    }
    #[test]
    fn programs_finish_with_one_measurement_and_valid_preparations() {
        for scenario in super::super::scenarios(true) {
            let request = WorkerRequest {
                scenario,
                fixture_root: PathBuf::from("fixtures"),
                output: PathBuf::from("sample"),
                scroll_seconds: 2.,
            };
            let steps = program(&request);
            assert_eq!(steps.iter().filter(|s| s.measured).count(), 1);
            assert!(steps.back().unwrap().measured);
            if request.scenario.flow == "range_selection" {
                assert_eq!(steps.len(), 3);
            }
            if request.scenario.flow == "forward" {
                assert_eq!(steps.back().unwrap().expected.entries, 24);
            }
        }
    }

    #[test]
    fn scrolling_preparation_does_not_wait_for_the_measured_event_count() {
        let mut step = Step::new("scroll_prepare", PathBuf::from("fixture"), 100, false);
        step.expected.scroll_direction = Some(true);
        assert!(!step.scroll_workload());
        step.operation = "scroll_up".into();
        assert!(step.scroll_workload());
    }

    #[test]
    fn prepared_settings_round_trip_and_stay_inside_the_given_root() {
        let temp = tempfile::tempdir().unwrap();
        let request = WorkerRequest {
            scenario: super::super::scenarios(false).remove(0),
            fixture_root: temp.path().join("fixtures"),
            output: temp.path().join("sample"),
            scroll_seconds: 2.,
        };
        let config = temp.path().join("isolated");
        fs::create_dir(&config).unwrap();
        prepare(&request, &config).unwrap();
        let parsed: crate::settings::ExplorerSettings =
            serde_json::from_slice(&fs::read(config.join("settings.json")).unwrap()).unwrap();
        assert_eq!(
            parsed.app.start,
            program(&request).front().unwrap().expected.path
        );
        assert!(!parsed.updater.enabled);
        assert!(!parsed.view.show_dotfiles);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
