//! Windows only: embeds the app icon, version info and the application manifest
//! (per-monitor DPI awareness, common controls v6) into webtabs.exe.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/windows");
        let dir = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("assets/windows");
        // Version info from Cargo.toml, so the file properties match the release.
        let v = |k: &str| std::env::var(k).unwrap();
        let commas = format!(
            "{},{},{},0",
            v("CARGO_PKG_VERSION_MAJOR"),
            v("CARGO_PKG_VERSION_MINOR"),
            v("CARGO_PKG_VERSION_PATCH")
        );
        let rc = std::fs::read_to_string(dir.join("webtabs.rc"))
            .expect("reading assets/windows/webtabs.rc")
            .replace("@DIR@", &dir.to_string_lossy().replace('\\', "/"))
            .replace("@VER_COMMAS@", &commas)
            .replace("@VER@", &v("CARGO_PKG_VERSION"));
        let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("webtabs.rc");
        std::fs::write(&out, rc).expect("writing the generated webtabs.rc");
        embed_resource::compile(&out, embed_resource::NONE)
            .manifest_required()
            .expect("compiling webtabs.rc (needs the Windows SDK's rc.exe)");
    }
}
