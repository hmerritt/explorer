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
#[cfg(any(target_os = "windows", test))]
mod updater;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod window_chrome;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod window_state;
#[cfg(any(target_os = "windows", test))]
mod windows_file_associations;
#[cfg(any(target_os = "windows", test))]
mod windows_squirrel;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub use settings::{
    AppSettings, ContextMenuSettings, CustomContextMenuItem, ExplorerSettings, FileColumnKind,
    FileColumnSettings, NewWindowBehaviour, SftpSettings, SidebarGroupKind, SidebarHiddenItem,
    SidebarSettings, TabSettings, UpdaterSettings, ViewSettings,
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
    let args = std::env::args_os().collect::<Vec<_>>();
    #[cfg(target_os = "windows")]
    let (args, first_run) = match windows_squirrel::handle_startup(args) {
        Ok(windows_squirrel::SquirrelStartup::Continue { args, first_run }) => (args, first_run),
        Ok(windows_squirrel::SquirrelStartup::Exit) => return,
        Err(error) => {
            eprintln!("failed to handle Explorer installer lifecycle: {error}");
            std::process::exit(1);
        }
    };
    #[cfg(not(target_os = "windows"))]
    let (args, first_run) = (args, false);

    #[cfg(target_os = "windows")]
    match windows_file_associations::handle_file_association_command(args.clone()) {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("failed to update Explorer file associations: {error}");
            std::process::exit(1);
        }
    }

    app::run(args, first_run);
}
