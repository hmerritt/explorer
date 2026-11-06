//! The benchmark attaches observers to normal application windows; it does not replace startup.
use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    rc::Rc,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use super::{Event, REQUEST_ENV, Request};
use crate::explorer::{ExplorerTabs, benchmark_support as adapter};
use gpui::{AnyWindowHandle, App, Entity, Keystroke, Window};

thread_local! { static SESSION: RefCell<Option<Rc<RefCell<Session>>>> = const { RefCell::new(None) }; }

struct Session {
    request: Request,
    stream: TcpStream,
    commands: Receiver<String>,
    origin: Instant,
    cycle: u64,
    armed: bool,
    running: bool,
    first: bool,
    loaded: bool,
    input_sent: bool,
    finished: bool,
    target: Vec<usize>,
    window: Option<AnyWindowHandle>,
    polling: bool,
}

fn session() -> Option<Rc<RefCell<Session>>> {
    SESSION.with(|s| s.borrow().clone())
}

pub(crate) fn is_active() -> bool {
    session().is_some()
}

pub(crate) fn initialize() {
    let origin = Instant::now();
    let Some(path) = std::env::var_os(REQUEST_ENV) else {
        return;
    };
    let result = (|| -> Result<Session, String> {
        let request: Request =
            serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        request.validate()?;
        if super::super::config_root() != Some(request.config.as_path()) {
            return Err("startup worker requires its own isolated configuration".into());
        }
        let stream = TcpStream::connect_timeout(&request.endpoint, Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        stream.set_nodelay(true).map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| e.to_string())?;
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        let (tx, commands) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            let _ = tx.send("quit".into());
        });
        Ok(Session {
            request,
            stream,
            commands,
            origin,
            cycle: 0,
            armed: true,
            running: false,
            first: false,
            loaded: false,
            input_sent: false,
            finished: false,
            target: Vec::new(),
            window: None,
            polling: false,
        })
    })();
    match result {
        Ok(mut state) => {
            state.emit("connected", None);
            SESSION.with(|s| *s.borrow_mut() = Some(Rc::new(RefCell::new(state))));
        }
        Err(error) => {
            eprintln!("Startup benchmark: {error}");
            std::process::exit(2);
        }
    }
}

impl Session {
    fn emit(&mut self, name: &str, display: Option<(f32, [f32; 2])>) {
        let event = Event {
            token: self.request.token.clone(),
            name: name.into(),
            cycle: self.cycle,
            entry_ms: self.origin.elapsed().as_secs_f64() * 1_000.,
            scale_factor: display.map(|d| d.0),
            viewport: display.map(|d| d.1),
        };
        // Small, bounded loopback writes; the parent records notification arrival separately.
        if serde_json::to_writer(&mut self.stream, &event).is_err()
            || self.stream.write_all(b"\n").is_err()
        {
            self.running = false;
        }
    }
}

pub(crate) fn launch_received(cx: &mut App) {
    let Some(state) = session() else { return };
    {
        let mut s = state.borrow_mut();
        if !s.armed {
            return;
        }
        s.cycle += 1;
        s.armed = false;
        s.running = true;
        s.first = false;
        s.loaded = false;
        s.input_sent = false;
        s.finished = false;
    }
    cx.refresh_windows();
    if !state.borrow().polling {
        state.borrow_mut().polling = true;
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(2))
                    .await;
                let state = state.clone();
                if cx.update(|cx| tick(&state, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}

pub(crate) fn attach(tabs: Entity<ExplorerTabs>, window: &mut Window, _cx: &mut App) {
    let Some(state) = session() else { return };
    state.borrow_mut().window = Some(window.window_handle());
    let before = state.clone();
    let after = state;
    let handle = window.window_handle();
    let display = (
        window.scale_factor(),
        [
            f32::from(window.viewport_size().width),
            f32::from(window.viewport_size().height),
        ],
    );
    window.observe_benchmark_frames(
        move |_, cx| {
            let s = before.borrow();
            if !s.running || s.finished || s.window != Some(handle) {
                return 0;
            }
            let snapshot = adapter::snapshot(&tabs, cx, false);
            let loaded = !snapshot.loading
                && snapshot.error.is_none()
                && snapshot.path == s.request.path
                && snapshot.entries == s.request.entries;
            s.cycle * 10
                + if loaded && s.input_sent && snapshot.selected == s.target {
                    3
                } else if loaded {
                    2
                } else {
                    1
                }
        },
        move |frame| {
            let mut s = after.borrow_mut();
            if !s.running || s.finished || frame.tag / 10 != s.cycle || !frame.drew_scene {
                return;
            }
            if !s.first {
                s.first = true;
                s.emit("first_frame", Some(display));
            }
            if frame.tag % 10 >= 2 && !s.loaded {
                s.loaded = true;
                s.emit("folder_frame", Some(display));
            }
            if frame.tag % 10 == 3 {
                s.finished = true;
                s.emit("input_frame", Some(display));
            }
        },
    );
}

fn tick(state: &Rc<RefCell<Session>>, cx: &mut App) {
    loop {
        let command = state.borrow().commands.try_recv();
        let Ok(command) = command else { break };
        match command.as_str() {
            "arm" => {
                let mut s = state.borrow_mut();
                s.armed = true;
                s.emit("armed", None);
            }
            "close" => {
                state.borrow_mut().running = false;
                for handle in cx.windows() {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
                let mut s = state.borrow_mut();
                s.window = None;
                s.emit("closed", None);
            }
            "quit" => {
                crate::startup_trace::flush();
                cx.quit();
                return;
            }
            _ => {
                state.borrow_mut().emit("invalid_command", None);
                cx.quit();
                return;
            }
        }
    }
    let handle = {
        let mut s = state.borrow_mut();
        if !s.running || !s.loaded || s.input_sent {
            return;
        }
        s.input_sent = true;
        // Alternate endpoints so a resident window must actually change selection.
        s.target = if s.request.entries == 0 {
            vec![]
        } else {
            vec![if s.cycle % 2 == 1 {
                s.request.entries - 1
            } else {
                0
            }]
        };
        s.window
    };
    if let Some(handle) = handle {
        let key = if state.borrow().cycle % 2 == 1 {
            "end"
        } else {
            "home"
        };
        let handled = handle.update(cx, |_, window, cx| {
            let handled =
                window.dispatch_keystroke(Keystroke::parse(key).expect("benchmark key"), cx);
            window.refresh();
            handled
        });
        if !matches!(handled, Ok(true)) {
            let mut s = state.borrow_mut();
            s.running = false;
            s.emit("input_not_handled", None);
        }
    }
}
