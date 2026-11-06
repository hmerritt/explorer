//! Optional, buffered startup diagnostics. No files are touched unless explicitly requested.
use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::Instant,
};

use serde::{Deserialize, Serialize};

static TRACE: OnceLock<Option<Trace>> = OnceLock::new();

struct Trace {
    origin: Instant,
    path: PathBuf,
    stages: Mutex<Vec<Stage>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Stage {
    pub name: String,
    pub start_ms: f64,
    pub duration_ms: f64,
}

pub(crate) fn initialize() {
    TRACE.get_or_init(|| {
        std::env::var_os("EXPLORER_STARTUP_TRACE").map(|path| Trace {
            origin: Instant::now(),
            path: PathBuf::from(path),
            stages: Mutex::new(Vec::new()),
        })
    });
}

pub(crate) struct Span {
    state: Option<(&'static Trace, &'static str, Instant)>,
}

impl Span {
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            state: TRACE
                .get()
                .and_then(Option::as_ref)
                .map(|trace| (trace, name, Instant::now())),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((trace, name, start)) = self.state {
            let mut stages = trace.stages.lock().unwrap_or_else(|e| e.into_inner());
            // An opt-in diagnostic session may remain resident for a long time.
            if stages.len() < 4_096 {
                stages.push(Stage {
                    name: name.into(),
                    start_ms: start.duration_since(trace.origin).as_secs_f64() * 1_000.,
                    duration_ms: start.elapsed().as_secs_f64() * 1_000.,
                });
            }
        }
    }
}

pub(crate) fn flush() {
    if let Some(trace) = TRACE.get().and_then(Option::as_ref) {
        let stages = trace
            .stages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let result = serde_json::to_vec_pretty(&stages)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&trace.path, bytes));
        if let Err(error) = result {
            eprintln!("Unable to write startup trace: {error}");
        }
    }
}
