#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod app;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod debug_options;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod explorer;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod http_client;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod loaders;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod settings;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub use settings::{
    AppSettings, ContextMenuSettings, CustomContextMenuItem, DriveHideKind, ExplorerSettings,
    FileColumnKind, FileColumnSettings, SidebarSettings, StartLocation, TabSettings, ViewSettings,
};

#[cfg(all(
    feature = "benchmarks",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
))]
pub use explorer::benchmark_support;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub fn run() {
    app::run();
}
