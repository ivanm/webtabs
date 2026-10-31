# webtabs

Your web apps (WhatsApp, Gmail, Calendar, Slack, internal dashboards) as tabs in
one small native window. No account, no store, no Electron.

- **Linux:** Rust + GTK3 + WebKitGTK (the system webview). About 1–2 MB binary.
- **macOS 14+:** native AppKit + WKWebView, linked to the same Rust core as Linux.
- **Windows 11 (prototype):** Rust + Win32 + WebView2 (`src/windows/`), same core. About 1.5 MB.

## Build and run on Windows (prototype)

Requires stable Rust (`x86_64-pc-windows-msvc`), the Visual Studio 2022 Build
Tools' C++ x64 tools (`Microsoft.VisualStudio.Component.VC.Tools.x86.x64`) and a
Windows SDK (headers/libs and `rc.exe`, which embeds the icon and manifest). The
WebView2 Runtime ships with Windows 11; the loader is linked statically.

```powershell
cargo build --release      # target\release\webtabs.exe
target\release\webtabs.exe # --hidden starts in the tray
```

Config lives in `%APPDATA%\webtabs\tabs.toml`, WebView2 profiles in
`%LOCALAPPDATA%\webtabs\WebView2` (one `shared` profile plus one per isolated
session). The prototype has the icon tab bar with unread bubbles and tags, the
total in the title, link routing, isolated sessions, notifications (shown as
Windows toasts through the tray icon; clicking one opens the tab), tray icon
with unread state, close-to-tray and hot reload of `tabs.toml`. There is no
add/edit dialog or settings window yet: `+` offers the presets, the gear and the
tray menu hold the switches and open `tabs.toml` in Notepad. `WEBTABS_DEBUG=1`
traces routing and notifications to stderr.

## Build and run on macOS

Requires Xcode (or the Command Line Tools with the Swift compiler) and stable Rust.

```sh
make build       # target/release/WebTabs.app, native to this Mac's architecture
open target/release/WebTabs.app
make install     # optional: copy to ~/Applications/WebTabs.app
make test        # shared Rust tests + clippy (rustup component add clippy)
```

The build also detects a project-local Rust installation at `target/toolchain`.
It signs with an "Apple Development" identity or the local "WebTabs Local Signing"
one when present, otherwise ad-hoc. Ad-hoc builds change identity on every
rebuild, so macOS asks again for Keychain access to WebKit's "WebTabs WebCrypto
Master Key". Run `scripts/macos-signing-identity.sh` once to create the local
identity; `WEBTABS_SIGN_IDENTITY=-` forces ad-hoc. Distributing to other Macs
needs Developer ID signing and notarization. `WEBTABS_ARCHS="arm64 x86_64"`
builds one app for Apple Silicon and Intel; `WEBTABS_APP` sets where it goes.

## Releases

Pushing a tag builds both platforms on GitHub Actions and publishes a release
with `WebTabs-macOS.zip` (Apple Silicon + Intel) and `webtabs-linux-x86_64.tar.gz`:

```sh
git tag v0.2.0 && git push origin v0.2.0
```

Actions › Release › Run workflow builds the same files as artifacts without
publishing. The Mac app is signed with the self-signed "WebTabs Release Signing"
identity from the `MACOS_SIGNING_P12` / `MACOS_SIGNING_PASSWORD` secrets
(`scripts/macos-release-signing.sh` creates and stores them), so updates keep
Keychain access. It isn't notarized: the first time you open a
downloaded copy, macOS blocks it; allow it under System Settings › Privacy &
Security › Open Anyway. Downloading with `gh` skips that check:

```sh
gh release download --repo ivanm/webtabs --pattern WebTabs-macOS.zip --clobber &&
  ditto -x -k WebTabs-macOS.zip ~/Applications
```

The Mac app works like the Linux one described below: the same icon tab bar
(unread bubbles, icon tags, `+` and the gear), right-click tab menu, middle-click
to reload, the add/edit sheet with presets and Advanced, and the settings window
with its tab manager. Shortcuts use ⌘ instead of Ctrl (⌘T, ⌘E, ⌘,, ⌘1…9,
⌃Tab or ⇧⌘] / ⇧⌘[, ⌘R, ⌘[ / ⌘], ⇧⌘H for the tab's home page); the menu bar icon
replaces the tray. Downloads go to ~/Downloads with a notification. The Dock icon
shows the unread count (Settings › Unread count on Dock icon); macOS only shows it,
and notifications, when WebTabs is allowed them in System Settings › Notifications.

Config lives in `~/Library/Application Support/webtabs/tabs.toml`, site icons in
`~/Library/Application Support/webtabs/favicons`. WebKit keeps the browsing
profiles itself; `WEBTABS_DATA` selects a separate set of them (and of icons).
Removing the last tab of an isolated session deletes that profile.
Notifications use a page `Notification` bridge; service-worker push
notifications are not supported. Pages are inspectable from Safari's Develop
menu and with "Inspect Element".

### Shared implementation

`src/config.rs`, `badge.rs`, `links.rs`, `favicon.rs`, and `presets.rs` are the
single source for both platforms: TOML parsing and validation, URL normalization,
user agents, session naming, unread parsing, link routing and the external-link
allowlist, favicon ranking and cache paths, and presets. `src/lib.rs` exposes
them to Swift through an owned JSON/C interface; Rust allocates responses and
Swift always releases them through Rust.

`src/linux/` owns GTK/WebKitGTK integration; `macos/` owns AppKit/WKWebView and
follows the same structure: `main.swift` (window, tabs, notifications, config
sync, like `linux/mod.rs`), `Web.swift` (`web.rs`), `TabDialog.swift`
(`tab_dialog.rs`), `Settings.swift` (`settings.rs`), `Icons.swift` (`icons.rs`),
`TabBar.swift` (the bar's views) and `Core.swift` (the bridge).

A native test with its own config and profile (generated pages, no accounts)
covers the bar, tab menu, add/edit sheets, settings, removal, empty state,
isolated-profile cleanup and, run a second time with `--verify-profile`, that a
cookie survives the restart:

```sh
WEBTABS_CONFIG="$PWD/target/smoke/tabs.toml" \
WEBTABS_DATA="$PWD/target/smoke/profile" \
target/release/WebTabs.app/Contents/MacOS/WebTabs --smoke-test
```

Add `WEBTABS_SNAPSHOTS=<dir>` to save PNGs of the native windows it opens.

The following installation instructions and Ctrl-based shortcuts describe Linux.

## Install (Linux)

Needs `webkit2gtk-4.1`, `gtk3`, `libayatana-appindicator` (tray), and
`gst-plugins-good`/`gst-plugins-bad`/`gst-libav` (media, calls).

```sh
make install      # ~/.local/bin/webtabs + desktop entry + icon
make autostart    # optional: start hidden in the tray on login
```

## Tabs

The tab bar shows each site's icon with its unread count; hover a tab to see its
name. `+` opens the add window: one-click presets (WhatsApp, Slack, Telegram,
Gmail) or any URL, plus an optional icon tag (e.g. "W"), mute, **isolated login**,
and a user-agent picker under Advanced.

**Isolated login** gives a tab its own cookies and storage, for a second account on
the same service (two Slack or Gmail accounts). Several Slack *workspaces* on one
account don't need it; switch between them inside Slack. Removing an isolated tab
deletes its login data.

Use `+` to add a tab, right-click a tab to edit, move or remove it, or open
**settings with the gear** (top right). Settings has the app switches (start with
computer, notifications, tray icon, close to tray, unread count in title) and the
tab list, where you can reorder tabs and mute notifications per tab.

Everything is stored in `~/.config/webtabs/tabs.toml`, which you can also edit by
hand. The app reloads it on save and keeps running tabs (and their logins) for
URLs that didn't change.

```toml
notifications = true
tray = true
close_to_tray = true
unread_in_title = true
unread_in_dock = true          # macOS: unread count on the Dock icon
# user_agent = "..."            # global override (default: Safari)

[[tab]]
name = "WhatsApp"
url = "https://web.whatsapp.com"
notifications = false           # mute this tab (badge still shows)
label = "W"                     # 1-2 letter tag on the icon
session = "whatsapp-2"          # isolated login storage

[[tab]]
name = "Internal"
url = "https://grafana.example.com"
user_agent = "..."              # per-tab override
```

All tabs share one persistent profile in `~/.local/share/webtabs`. Site icons are
cached in `~/.local/share/webtabs/favicons`.

## Shortcuts

| Keys | Action |
|---|---|
| Ctrl+1…8 / Ctrl+9 | go to tab N / last tab |
| Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+PgUp/PgDn | next / previous tab |
| Ctrl+R, F5 / Ctrl+Shift+R | reload / reload ignoring cache |
| Alt+Left / Alt+Right / Alt+Home | back / forward / tab home page |
| Ctrl+T / Ctrl+E | add tab / edit current tab |
| Ctrl+= / Ctrl+- / Ctrl+0 | zoom in / out / reset |
| Ctrl+, | settings |
| F12, Ctrl+Shift+I | web inspector |
| Ctrl+W / Ctrl+Q | hide to tray / quit |

Middle-click a tab to reload it.

## Behaviour

- **Unread badges** come from page titles (`(3) WhatsApp`, `Inbox (12)`, `* Slack`).
  The total shows in the window title. The tray icon switches to its unread
  variant and the taskbar entry flashes.
- **Notifications** become desktop notifications, unless they're turned off in
  settings or muted for that tab. Clicking one opens the right tab
  and the page jumps to that chat or email.
- **Links:** links you click to other sites open in your default browser. Same-site
  windows and logins (Google, Microsoft, Okta, Apple, GitHub sign-in) open in a
  small in-app popup so they finish inside the app's session. A page can't open
  your browser or launch other apps by itself: that needs a real click, and only
  web, mail and phone links are handed out.
- **Permissions:** notifications and camera/mic (for Meet) are granted only to the
  tab's own site; DRM is allowed. Location is denied. Pages can't read your
  clipboard (your own copy/paste works as usual).
- A crashed tab reloads itself (at most 3 times a minute). A broken `tabs.toml`
  is ignored (with a notification) until you fix it.

## Develop

```sh
make test   # unit tests + clippy -D warnings
cargo run
```

Environment overrides (handy for testing): `WEBTABS_CONFIG=path/to/tabs.toml`,
`WEBTABS_DATA=dir`. `WEBTABS_DEBUG=1` prints every page's JS console to stdout.

On NVIDIA's proprietary driver the app sets `WEBKIT_DISABLE_DMABUF_RENDERER=1`
itself; without it WebKitGTK renders blank pages.
