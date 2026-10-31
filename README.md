# webtabs

Your web apps (WhatsApp, Gmail, Calendar, Slack, internal dashboards) as tabs in
one small native window. No account, no store, no Electron.

- **Linux:** Rust + GTK3 + WebKitGTK (the system webview). About 1–2 MB binary.
- **macOS 14+:** native AppKit + WKWebView, linked to the same Rust core as Linux.

## Build and run on macOS

Requires Xcode (or the Command Line Tools with the Swift compiler) and stable Rust.

```sh
make build       # target/release/WebTabs.app, native to this Mac's architecture
open target/release/WebTabs.app
make install     # optional: copy to ~/Applications/WebTabs.app
make test        # shared Rust tests + clippy (rustup component add clippy)
```

The build also detects a project-local Rust installation at `target/toolchain`.
The app is ad-hoc signed for local use; distributing it to other Macs requires
Developer ID signing and notarization. No Electron or bundled browser engine.

Use ⌘T to add a tab, ⌘E to edit, ⌘, for settings, ⌘1…9 to switch,
Control-Tab to cycle, and the Tabs menu to reorder. ⌘W closes the window
(or keeps the app in the menu bar when enabled); ⌘Q quits. Standard copy/paste,
reload, navigation and zoom shortcuts are available in the menus.

Config lives in `~/Library/Application Support/webtabs/tabs.toml` and reloads
on edits. WebKit manages persistent profiles in its own application storage;
`WEBTABS_DATA` selects a separate profile namespace on Mac rather than moving
WebKit's files. Shared tabs use one profile; isolated tabs have separate
persistent cookies/storage. Removing the last tab using an isolated session
clears that session's website data. Start at login uses macOS Login Items.

macOS supports page-created desktop notifications through a native bridge,
including mute settings and click-to-focus. Service-worker/push notifications,
site-specific authenticated calls, DRM, and SSO flows have not been validated.
Camera and microphone access use WebKit/macOS permission prompts. Site icons are
loaded into memory on Mac; Linux additionally caches them to disk. Inspector
support is enabled by `WEBTABS_DEBUG=1` (Safari's Develop menu).

### Shared implementation

`src/config.rs`, `badge.rs`, `links.rs`, `favicon.rs`, and `presets.rs` are the
single source for both platforms: TOML parsing and validation, URL normalization,
user agents, session naming, unread parsing, link routing, favicon ranking, and
presets. `src/lib.rs` exposes these functions to Swift through an owned JSON/C
interface. Rust allocates responses and Swift always releases them through Rust.

`src/linux/` owns GTK/WebKitGTK integration. `macos/main.swift` owns AppKit/WKWebView
integration: controls, webview lifetimes, OS notifications, menus, profile storage,
and login registration. Tab-view reconciliation is platform-specific because it
owns native widgets; it does not duplicate configuration or routing rules.

For a deterministic native rendering test with a separate config/profile:

```sh
WEBTABS_CONFIG="$PWD/target/smoke/tabs.toml" \
WEBTABS_DATA="$PWD/target/smoke/profile" \
target/release/WebTabs.app/Contents/MacOS/WebTabs --smoke-test
```

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
