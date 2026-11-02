# Developing WebTabs

WebTabs is one Rust core shared by three native front ends:

| Platform | Front end | Web engine |
|---|---|---|
| Linux | Rust + GTK3 (`src/linux/`) | WebKitGTK 4.1 (the system webview) |
| macOS 14+ | Swift + AppKit (`macos/`), linked to the Rust core | WKWebView |
| Windows 10 22H2+ / 11 | Rust + Win32 (`src/windows/`) | WebView2 (Edge) |

- [Shared core](#shared-core)
- [Linux](#linux)
- [macOS](#macos)
- [Windows](#windows)
- [Releases](#releases)
- [Environment variables](#environment-variables)

## Shared core

`src/config.rs`, `badge.rs`, `links.rs`, `favicon.rs` and `presets.rs` are the
single source of every product rule: TOML parsing and validation, URL
normalization, user agents, session naming, unread parsing, link routing and the
external-link allowlist, favicon ranking and cache paths, and the presets.
`src/autostart.rs` is the Linux XDG autostart entry; Windows uses the Run key
(`src/windows/autostart.rs`).

`src/lib.rs` exposes the core to Swift through an owned JSON/C interface: Rust
allocates responses and Swift always releases them through Rust.

Each front end follows the same layout:

| Linux | macOS | Windows | Role |
|---|---|---|---|
| `linux/mod.rs` | `main.swift` | `windows/mod.rs` | window, tabs, tray, notifications, config sync |
| `linux/web.rs` | `Web.swift` | `windows/web.rs` | webview setup, link routing, permissions |
| `linux/settings.rs` | `Settings.swift` | `windows/panel.rs` + `panel.html` | settings window |
| `linux/tab_dialog.rs` | `TabDialog.swift` | `windows/panel.rs` + `panel.html` | add/edit tab form |
| `linux/icons.rs` | `Icons.swift` | `windows/icons.rs` | site icons |
| — | `TabBar.swift` | `windows/paint.rs` | tab bar drawing |
| — | `Core.swift` | — | bridge to the Rust core |

On Windows the settings window and tab form are a local page in WebView2 that
only renders state and posts clicks; validation and saving stay in Rust.

```sh
make test   # unit tests + clippy -D warnings
cargo run
```

## Linux

Needs Rust stable and the development packages for `webkit2gtk-4.1`, `gtk3` and
`libayatana-appindicator`.

```sh
cargo build --release   # target/release/webtabs
make install            # ~/.local/bin/webtabs + desktop entry + icon
make autostart          # optional: start hidden in the tray on login
make package            # target/dist: tarball, .deb (where dpkg-deb exists), SHA256SUMS-linux
make package-arch       # target/dist: Arch package from the current commit (needs makepkg)
```

`packaging/aur/PKGBUILD` builds from the tagged source (`makepkg -si`);
`scripts/aur-update.sh <version>` bumps it, its checksum and `.SRCINFO`.

**NVIDIA:** WebKitGTK's default DMA-BUF renderer shows blank pages on NVIDIA's
driver, so the app sets `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1` itself. That keeps
pages GPU-rendered and only copies finished frames. `WEBKIT_DISABLE_DMABUF_RENDERER=1`
also works but paints on the CPU (it is what Settings › GPU acceleration off uses).
Explicit `WEBKIT_*` variables always win.

## macOS

Requires Xcode (or the Command Line Tools with the Swift compiler) and stable Rust.

```sh
make build       # target/release/WebTabs.app, native to this Mac's architecture
open target/release/WebTabs.app
make install     # optional: copy to ~/Applications/WebTabs.app
make test        # shared Rust tests + clippy (rustup component add clippy)
```

The build also detects a project-local Rust installation at `target/toolchain`.
`WEBTABS_ARCHS="arm64 x86_64"` builds one app for Apple Silicon and Intel;
`WEBTABS_APP` sets where it goes.

**Signing.** The build signs with an "Apple Development" identity or the local
"WebTabs Local Signing" one when present, otherwise ad hoc. Ad hoc builds change
identity on every rebuild, so macOS asks again for Keychain access to WebKit's
"WebTabs WebCrypto Master Key". Run `scripts/macos-signing-identity.sh` once to
create the local identity; `WEBTABS_SIGN_IDENTITY=-` forces ad hoc. Distributing
to other Macs without warnings needs Developer ID signing and notarization.

**Smoke test.** A native test with its own config and profile (generated pages, no
accounts) covers the bar, tab menu, add/edit sheets, settings, removal, empty
state, dropped files, isolated-profile cleanup and, run a second time with
`--verify-profile`, that a cookie survives the restart:

```sh
WEBTABS_CONFIG="$PWD/target/smoke/tabs.toml" \
WEBTABS_DATA="$PWD/target/smoke/profile" \
target/release/WebTabs.app/Contents/MacOS/WebTabs --smoke-test
```

Add `WEBTABS_SNAPSHOTS=<dir>` to save PNGs of the native windows it opens.

**README screenshot.** `scripts/macos-screenshot.sh` (after `make build`) renders
`docs/screenshot-macos.png` from the mock pages in `docs/screenshot/`: fictional
chats, unread counts in their titles, the services' real icons, and a throwaway
profile. It runs the app with `WEBTABS_SCREENSHOT=<file>`, which saves the
1280×800 window (title bar, tab bar, the first tab's page;
`WEBTABS_SCREENSHOT_TAB=<n>` picks another) and quits. AppKit draws the window and
WebKit snapshots the page, so no Screen Recording permission is needed; macOS
won't focus a background launch, so the title-bar buttons are painted in their
active colours.

Notifications use a page `Notification` bridge; service-worker push
notifications are not supported. Pages are inspectable from Safari's Develop menu
and with "Inspect Element".

## Windows

Requires stable Rust (`x86_64-pc-windows-msvc`), the Visual Studio 2022 Build
Tools' C++ x64 tools (`Microsoft.VisualStudio.Component.VC.Tools.x86.x64`) and a
Windows SDK (headers/libs and `rc.exe`, which embeds the icon, manifest and
version). The WebView2 Runtime ships with Windows 11; the loader is linked
statically.

```powershell
cargo build --release      # target\release\webtabs.exe
target\release\webtabs.exe # --hidden starts in the tray
```

Packages are built with WiX 5 (`dotnet tool install --global wix --version 5.0.2`):

```powershell
pwsh scripts\package-windows.ps1     # target\dist: .msi, .zip, SHA256SUMS-windows
pwsh scripts\check-windows-msi.ps1   # installs, checks and uninstalls the MSI
```

The MSI is per-user (`%LOCALAPPDATA%\Programs\WebTabs`, Start menu shortcut, no
admin prompt) and refuses to install without the WebView2 Runtime. Never change
its `UpgradeCode` (`packaging/windows/webtabs.wxs`). WebView2 profiles live in
`%LOCALAPPDATA%\webtabs\WebView2`: one `shared` profile plus one per isolated
session. "Start with computer" writes a `webtabs` value under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

## Releases

Pushing a `v*` tag builds every platform on GitHub Actions
(`.github/workflows/release.yml`) and publishes a release:

| Platform | Files |
|---|---|
| macOS | `WebTabs-macOS.zip` (Apple Silicon + Intel) |
| Linux | `webtabs-<version>-linux-x86_64.tar.gz`, `webtabs_<version>_amd64.deb`, `SHA256SUMS-linux` |
| Arch | `webtabs-<version>-1-x86_64.pkg.tar.zst`, `SHA256SUMS-arch` |
| Windows | `WebTabs-<version>-windows-x64.msi`, `webtabs-<version>-windows-x64.zip`, `SHA256SUMS-windows` |

```sh
# bump Cargo.toml (+ Cargo.lock) and packaging/aur/PKGBUILD + .SRCINFO, commit, then:
git tag -a v0.2.3 -m "WebTabs 0.2.3" && git push origin main v0.2.3
```

Each job checks what it built: the `.deb` and Arch package are installed in a
clean system and their libraries resolved, and the MSI is installed, inspected
and uninstalled on a Windows runner. Actions › Release › Run workflow builds the
same files as artifacts without publishing.

The Mac app is signed with the self-signed "WebTabs Release Signing" identity
from the `MACOS_SIGNING_P12` / `MACOS_SIGNING_PASSWORD` secrets
(`scripts/macos-release-signing.sh` creates and stores them), so updates keep
Keychain access. It isn't notarized, and the Windows files aren't code-signed.

## Environment variables

| Variable | Effect |
|---|---|
| `WEBTABS_CONFIG` | path to `tabs.toml` |
| `WEBTABS_DATA` | data directory (browsing profiles, icons) |
| `WEBTABS_SCREENSHOT`, `WEBTABS_SCREENSHOT_TAB` | macOS: save the window as a PNG and quit (see [README screenshot](#macos)) |
| `WEBTABS_DEBUG=1` | Linux: page JS console to stdout. Windows: routing and notification trace to stderr |
| `WEBKIT_DMABUF_RENDERER_FORCE_SHM`, `WEBKIT_DISABLE_DMABUF_RENDERER` | Linux renderer overrides (see [NVIDIA](#linux)) |
| `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` | Windows: extra WebView2 flags, e.g. `--disable-gpu` |
