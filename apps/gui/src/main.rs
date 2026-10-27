//! PC Agent's desktop interface: the same core as the `pcagent` CLI, in a window.
// No console window behind the app in release builds; debug builds keep it for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

// Portable view models also compile on targets without a desktop webview.
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
mod failure;
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
mod images;
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
mod model_routing;
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
mod preview;
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
mod protocol;
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
mod selection;
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
mod session;
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
mod settings;
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
mod shell;
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code, unused_imports)
)]
mod view;

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
mod actions;
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
mod app;
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
mod elevation;

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn main() {
    app::run();
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn main() {
    eprintln!(
        "PC Agent's desktop requires Windows, Linux or macOS. Use the pcagent CLI on this system."
    );
    std::process::exit(2);
}
