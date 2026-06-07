#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod app;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod explorer;
#[allow(dead_code)]
mod ngram;

#[cfg(feature = "benchmarks")]
#[doc(hidden)]
pub mod benchmark_support {
    pub use crate::explorer::recursive_search::benchmark_support::*;
}

#[cfg(feature = "benchmarks")]
#[doc(hidden)]
pub mod ngram_benchmark_support {
    pub use crate::ngram::{NgramIndex, NgramIndexBuilder, NgramSearchSession};
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub fn run() {
    app::run();
}
