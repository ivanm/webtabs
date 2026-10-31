// Release builds are GUI apps on Windows: no console window behind them.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// XDG autostart: Linux only for now (Windows' mechanism is an open decision).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod autostart;
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
mod badge;
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
mod config;
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
mod favicon;
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
mod links;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
mod presets;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::run()
}

#[cfg(target_os = "macos")]
fn main() {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().expect("executable path");
    let dir = exe.parent().expect("executable directory");
    let app = dir.join("WebTabs.app/Contents/MacOS/WebTabs");
    let app = if app.exists() {
        app
    } else {
        dir.join("../release/WebTabs.app/Contents/MacOS/WebTabs")
    };
    let error = std::process::Command::new(app)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("webtabs: {error}. Run `make build` to build the native Mac app first.");
    std::process::exit(1);
}

#[cfg(target_os = "windows")]
fn main() -> std::process::ExitCode {
    windows::run()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn main() {
    eprintln!("webtabs: this platform's backend is not implemented yet (macOS is next)");
    std::process::exit(1);
}
