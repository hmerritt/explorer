#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn main() {
    explorer::run();
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn main() {
    eprintln!("Explorer's GPUI app currently targets Windows, macOS, and Linux.");
    std::process::exit(1);
}
