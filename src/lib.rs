#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod app;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod explorer;

#[cfg(all(
    feature = "benchmarks",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
))]
pub use explorer::benchmark_support;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub fn run() {
    app::run();
}
