#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod autostart;
mod badge;
mod config;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod favicon;
mod links;
#[cfg(target_os = "linux")]
mod linux;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod presets;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("webtabs: this platform's backend is not implemented yet (macOS is next)");
    std::process::exit(1);
}
