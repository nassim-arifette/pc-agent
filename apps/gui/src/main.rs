//! PC Agent's desktop interface: the same core as the `pcagent` CLI, in a window.
// No console window behind the app in release builds; debug builds keep it for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

// Portable: built and tested on every platform, used by the window on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
mod daily_report;
#[cfg_attr(not(windows), allow(dead_code))]
mod failure;
#[cfg_attr(not(windows), allow(dead_code))]
mod images;
#[cfg_attr(not(windows), allow(dead_code))]
mod model_routing;
#[cfg_attr(not(windows), allow(dead_code))]
mod preview;
#[cfg_attr(not(windows), allow(dead_code))]
mod protocol;
#[cfg_attr(not(windows), allow(dead_code))]
mod selection;
#[cfg_attr(not(windows), allow(dead_code))]
mod session;
#[cfg_attr(not(windows), allow(dead_code))]
mod settings;
#[cfg_attr(not(windows), allow(dead_code))]
mod shell;
#[cfg_attr(not(windows), allow(dead_code, unused_imports))]
mod view;

#[cfg(windows)]
mod actions;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod elevation;

#[cfg(windows)]
fn main() {
    // Started by the daily scheduled task: write the report without opening a window.
    let args = std::env::args_os().collect::<Vec<_>>();
    if daily_report::requested(&args) {
        std::process::exit(daily_report::run_headless());
    }
    if daily_report::remove_requested(&args) {
        std::process::exit(daily_report::remove_headless());
    }
    app::run();
}

#[cfg(not(windows))]
fn main() {
    let args = std::env::args_os().collect::<Vec<_>>();
    if daily_report::requested(&args) {
        std::process::exit(daily_report::run_headless());
    }
    if daily_report::remove_requested(&args) {
        std::process::exit(daily_report::remove_headless());
    }
    eprintln!("PC Agent's window runs on Windows. Use the pcagent CLI on this system.");
    std::process::exit(2);
}
