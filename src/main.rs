#[cfg(any(target_os = "macos", target_os = "linux"))]
mod app;

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() {
    app::run();
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    eprintln!("Universal Explorer's GPUI app currently targets macOS and Linux.");
    std::process::exit(1);
}
