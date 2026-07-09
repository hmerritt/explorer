#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod app;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod debug_options;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod explorer;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod http_client;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[path = "image/mod.rs"]
mod image_viewer;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod loaders;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod settings;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod window_chrome;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod window_state;
#[cfg(any(target_os = "windows", test))]
mod windows_file_associations;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub use settings::{
    AppSettings, ContextMenuSettings, CustomContextMenuItem, DriveHideKind, ExplorerSettings,
    FileColumnKind, FileColumnSettings, NewWindowBehaviour, SidebarGroupKind, SidebarSettings,
    TabSettings, ViewSettings,
};

#[cfg(all(
    feature = "benchmarks",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
))]
pub mod benchmark_support {
    pub use crate::explorer::benchmark_support::*;
    pub use crate::image_viewer::benchmark_support::*;
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub fn run() {
    #[cfg(target_os = "windows")]
    match windows_file_associations::handle_file_association_command(std::env::args_os()) {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("failed to update Explorer file associations: {error}");
            std::process::exit(1);
        }
    }

    app::run();
}
