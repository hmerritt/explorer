#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod app;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod debug_options;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod epub_reader;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod explorer;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod http_client;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[path = "image/mod.rs"]
mod image_viewer;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod loaders;
mod os_paths;
#[cfg(feature = "benchmarks")]
pub mod performance;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod settings;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod startup_trace;
#[cfg(any(target_os = "windows", test))]
mod update_check;
#[cfg(test)]
mod updater;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod window_chrome;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod window_state;
#[cfg(any(target_os = "windows", test))]
mod windows_file_associations;
#[cfg(any(target_os = "windows", test))]
mod windows_squirrel;
#[cfg(target_os = "windows")]
mod windows_tray;

// GPUI is vendored outside the workspace. Include its native move regression
// here so the standard application test run also exercises the backend helper.
#[cfg(all(test, target_os = "windows"))]
#[path = "../vendor/gpui/src/platform/windows/window_move.rs"]
mod gpui_window_move_regression;

// Keep the vendored adapter-selection regression in the normal cross-platform test run.
#[cfg(test)]
#[path = "../vendor/gpui/src/platform/windows/device_selection.rs"]
mod gpui_device_selection_regression;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub use settings::{
    AppSettings, ContextMenuSettings, CustomContextMenuItem, ExplorerSettings, FileColumnKind,
    FileColumnSettings, NewWindowBehaviour, SftpSettings, SidebarGroupKind, SidebarHiddenItem,
    SidebarOrderItem, SidebarOrderSettings, SidebarSettings, TabSettings, UpdaterSettings,
    ViewSettings,
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
    // A normal build cannot honor the isolated benchmark configuration. Reject
    // worker launches before routing to an existing instance or touching settings.
    #[cfg(not(feature = "benchmarks"))]
    if std::env::var_os("EXPLORER_STARTUP_BENCH_REQUEST").is_some() {
        eprintln!("Startup benchmark workers require a build with --features benchmarks");
        std::process::exit(2);
    }
    startup_trace::initialize();
    #[cfg(feature = "benchmarks")]
    performance::startup::initialize();
    let args = std::env::args_os().collect::<Vec<_>>();
    #[cfg(target_os = "windows")]
    let (args, first_run) = match {
        let _span = startup_trace::Span::new("installer_lifecycle");
        windows_squirrel::handle_startup(args)
    } {
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
    match {
        let _span = startup_trace::Span::new("file_association_arguments");
        windows_file_associations::handle_file_association_command(args.clone())
    } {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("failed to update Explorer file associations: {error}");
            std::process::exit(1);
        }
    }

    app::run(args, first_run);
    startup_trace::flush();
}
