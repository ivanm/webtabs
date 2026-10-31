//! Windows only: embeds the app icon, version info and the application manifest
//! (per-monitor DPI awareness, common controls v6) into webtabs.exe.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/windows");
        embed_resource::compile("assets/windows/webtabs.rc", embed_resource::NONE)
            .manifest_required()
            .expect("compiling assets/windows/webtabs.rc (needs the Windows SDK's rc.exe)");
    }
}
