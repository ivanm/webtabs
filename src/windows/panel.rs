//! Settings window and add/edit tab form: the Windows counterparts of
//! `src/linux/settings.rs` and `src/linux/tab_dialog.rs`. One owned, modal
//! window hosts `panel.html` in WebView2; the page only renders state and
//! forwards clicks as JSON messages, every rule (validation, isolated-login
//! hints, session keys, saving) runs here through the same `App` methods the
//! tab bar uses. Changes apply immediately and are written to tabs.toml.

use super::{App, TabView, app, autostart, chrome, icons, paint, web};
use crate::appearance::{BarMetrics, Tint, tint_label, tint_names};
use crate::config::{self, ICON_SIZES, Tab, USER_AGENT_PRESETS};
use crate::presets::PRESETS;
use crate::{favicon, links};
use ::windows::Win32::Foundation::*;
use ::windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, FillRect, GetMonitorInfoW, HDC, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::HiDpi::GetDpiForWindow;
use ::windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use ::windows::Win32::UI::WindowsAndMessaging::*;
use ::windows::core::{HSTRING, Interface, PCWSTR, PWSTR, w};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;

const CLASS: PCWSTR = w!("WebTabsPanel");
const PAGE: &str = include_str!("panel.html");
/// A message from the page (lparam = `Box<String>` of JSON), handled outside
/// WebView2's event so it may open message boxes.
const WM_APP_PAGE: u32 = WM_APP + 20;
/// App state changed: resend it to the page (coalesced).
const WM_APP_REFRESH: u32 = WM_APP + 21;
/// A preset's icon finished downloading (wparam = preset index, lparam = `Box<Fetched>`).
const WM_APP_PRESET_ICON: u32 = WM_APP + 22;
/// Download generation for preset icons; keeps their temp files apart from tabs'.
const PRESET_GEN: u64 = u64::MAX;
/// Saves tabs.toml once the icon size slider or colour picker settles.
const TIMER_SAVE: usize = 1;
const SAVE_DELAY_MS: u32 = 500;
/// The app icon for About (sent once).
const APP_ICON: &[u8] = include_bytes!("../../assets/webtabs.png");

/// Page background (panel.html's --win), shown before the page paints.
fn background(dark: bool) -> (u8, u8, u8) {
    if dark {
        (0x1d, 0x1d, 0x1f)
    } else {
        (0xff, 0xff, 0xff)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    Settings,
    /// The add-tab form (the "+" button, Ctrl+T).
    Add,
    /// The edit form for a tab id (a tab's Edit…).
    Edit(u64),
}

struct Panel {
    hwnd: HWND,
    owner: HWND,
    controller: RefCell<Option<ICoreWebView2Controller>>,
    webview: RefCell<Option<ICoreWebView2>>,
    ready: Cell<bool>,
    /// What to show once the page has loaded.
    pending: Cell<Option<Open>>,
    refresh_posted: Cell<bool>,
    /// An appearance change waits to be written to tabs.toml.
    save_pending: Cell<bool>,
    /// The About icon went out with an earlier state.
    icon_sent: Cell<bool>,
    dark: Cell<bool>,
}

thread_local! {
    static PANEL: RefCell<Option<Rc<Panel>>> = const { RefCell::new(None) };
}

fn panel() -> Option<Rc<Panel>> {
    PANEL.with(|p| p.borrow().clone())
}

/// The open settings window, to own message boxes raised from it.
pub fn hwnd() -> Option<HWND> {
    panel().map(|p| p.hwnd)
}

/// Opens (or brings back) the window showing `what`.
pub fn open(app: &App, what: Open) {
    if let Some(p) = panel() {
        unsafe {
            if IsIconic(p.hwnd).as_bool() {
                let _ = ShowWindow(p.hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(p.hwnd);
        }
        p.show(what);
        return;
    }
    let Some(env) = app.env.borrow().clone() else {
        return; // WebView2 is still starting; nothing to host the page in yet
    };
    let Some(hwnd) = create_window(app.hwnd, what) else {
        return;
    };
    let panel = Rc::new(Panel {
        hwnd,
        owner: app.hwnd,
        controller: RefCell::default(),
        webview: RefCell::default(),
        ready: Cell::new(false),
        pending: Cell::new(Some(what)),
        refresh_posted: Cell::new(false),
        save_pending: Cell::new(false),
        icon_sent: Cell::new(false),
        dark: Cell::new(app.dark.get()),
    });
    chrome::set_dark_frame(hwnd, app.dark.get());
    PANEL.with(|p| *p.borrow_mut() = Some(panel.clone()));
    unsafe {
        // Modal, like the Linux dialogs: the tab bar waits until this closes.
        let _ = EnableWindow(app.hwnd, false);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
    }
    web::create_controller(&env, hwnd, &web::profile_name(None), move |result| {
        let Some(p) = panel_for(hwnd) else {
            if let Ok(c) = result {
                unsafe {
                    let _ = c.Close();
                }
            }
            return;
        };
        match result {
            Ok(c) => p.attach(c),
            Err(e) => {
                eprintln!("webtabs: settings window: {e}");
                p.close();
            }
        }
    });
}

/// The system theme changed: follow it in the window frame and background.
pub fn appearance_changed() {
    let (Some(p), Some(app)) = (panel(), app()) else {
        return;
    };
    let dark = app.dark.get();
    if p.dark.replace(dark) != dark {
        chrome::set_dark_frame(p.hwnd, dark);
        p.set_background();
    }
}

/// Resends the app state to an open window (after any change, from anywhere).
pub fn refresh() {
    if let Some(p) = panel()
        && p.ready.get()
        && !p.refresh_posted.replace(true)
    {
        unsafe {
            let _ = PostMessageW(Some(p.hwnd), WM_APP_REFRESH, WPARAM(0), LPARAM(0));
        }
    }
}

fn panel_for(hwnd: HWND) -> Option<Rc<Panel>> {
    panel().filter(|p| p.hwnd == hwnd)
}

fn create_window(owner: HWND, what: Open) -> Option<HWND> {
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.ok()?.into();
    thread_local! { static REGISTERED: Cell<bool> = const { Cell::new(false) }; }
    if !REGISTERED.with(|r| r.replace(true)) {
        let icon = |index| HICON(unsafe { GetClassLongPtrW(owner, index) } as *mut _);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hIcon: icon(GCLP_HICON),
            hIconSm: icon(GCLP_HICONSM),
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&wc) } == 0 {
            REGISTERED.with(|r| r.set(false));
            return None;
        }
    }
    // Sizes in DIPs, like the GTK dialogs; scaled to the owner's monitor and
    // centred over the main window, clamped to the work area.
    let (w, h) = match what {
        Open::Settings => (760, 620),
        Open::Add => (560, 620),
        Open::Edit(_) => (560, 600),
    };
    let dpi = unsafe { GetDpiForWindow(owner) }.max(96) as i32;
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let mut parent = RECT::default();
    unsafe {
        let _ = GetMonitorInfoW(MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST), &mut mi);
        let _ = GetWindowRect(owner, &mut parent);
    }
    let work = mi.rcWork;
    let w = (w * dpi / 96).min(work.right - work.left);
    let h = (h * dpi / 96).min(work.bottom - work.top);
    let x = ((parent.left + parent.right - w) / 2).clamp(work.left, work.right - w);
    let y = ((parent.top + parent.bottom - h) / 2).clamp(work.top, work.bottom - h);
    unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            CLASS,
            w!("WebTabs Settings"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_CLIPCHILDREN,
            x,
            y,
            w,
            h,
            Some(owner),
            None,
            Some(instance),
            None,
        )
    }
    .ok()
}

fn rgb((r, g, b): (u8, u8, u8)) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

impl Panel {
    fn attach(&self, controller: ICoreWebView2Controller) {
        let Ok(wv) = (unsafe { controller.CoreWebView2() }) else {
            self.close();
            return;
        };
        *self.controller.borrow_mut() = Some(controller.clone());
        self.set_background();
        unsafe {
            if let Ok(s) = wv.Settings() {
                let debug = std::env::var_os("WEBTABS_DEBUG").is_some();
                let _ = s.SetAreDevToolsEnabled(debug);
                let _ = s.SetIsStatusBarEnabled(false);
                let _ = s.SetIsZoomControlEnabled(false);
                let _ = s.SetAreDefaultScriptDialogsEnabled(false);
                if let Ok(s3) = s.cast::<ICoreWebView2Settings3>() {
                    // No reload/print/find on a settings page.
                    let _ = s3.SetAreBrowserAcceleratorKeysEnabled(debug);
                }
            }
        }
        if let Err(e) = self.setup_events(&wv) {
            eprintln!("webtabs: settings window events: {}", e.message());
        }
        *self.controller.borrow_mut() = Some(controller.clone());
        *self.webview.borrow_mut() = Some(wv.clone());
        self.layout();
        unsafe {
            let _ = controller.SetIsVisible(true);
            let _ = wv.NavigateToString(&HSTRING::from(PAGE));
        }
        web::focus(&controller);
    }

    /// The window and webview background for the theme (seen before the page
    /// paints, and while resizing).
    fn set_background(&self) {
        let (r, g, b) = background(self.dark.get());
        if let Some(c2) = self
            .controller
            .borrow()
            .as_ref()
            .and_then(|c| c.cast::<ICoreWebView2Controller2>().ok())
        {
            unsafe {
                let _ = c2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                    A: 255,
                    R: r,
                    G: g,
                    B: b,
                });
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
    }

    fn paint_background(&self, hdc: HDC) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
            let brush = CreateSolidBrush(rgb(background(self.dark.get())));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush.into());
        }
    }

    fn setup_events(&self, wv: &ICoreWebView2) -> ::windows::core::Result<()> {
        let hwnd = self.hwnd;
        let mut token = 0i64;
        unsafe {
            wv.add_WebMessageReceived(
                &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut json = PWSTR::null();
                    args.WebMessageAsJson(&mut json)?;
                    let boxed = Box::into_raw(Box::new(take_pwstr(json)));
                    if PostMessageW(Some(hwnd), WM_APP_PAGE, WPARAM(0), LPARAM(boxed as isize))
                        .is_err()
                    {
                        drop(Box::from_raw(boxed));
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            // The page never opens windows or navigates (a file dropped on it would).
            wv.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetHandled(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            wv.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(|_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let uri = take_pwstr(uri);
                    // NavigateToString loads as about:blank / data:.
                    if !(uri.starts_with("about:") || uri.starts_with("data:")) {
                        args.SetCancel(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        Ok(())
    }

    fn layout(&self) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        if let Some(c) = self.controller.borrow().as_ref() {
            unsafe {
                let _ = c.SetBounds(rc);
            }
        }
    }

    fn post(&self, msg: Value) {
        if let Some(wv) = self.webview.borrow().as_ref() {
            unsafe {
                let _ = wv.PostWebMessageAsJson(&HSTRING::from(msg.to_string()));
            }
        }
    }

    fn show(&self, what: Open) {
        if !self.ready.get() {
            self.pending.set(Some(what));
            return;
        }
        // The window is "only" the form when it was opened for one and the
        // settings list isn't showing; Cancel/Save then close it.
        match what {
            Open::Settings => self.post(json!({"cmd": "settings"})),
            Open::Add => self.post(json!({"cmd": "form", "id": null, "only": false})),
            Open::Edit(id) => self.post(json!({"cmd": "form", "id": id, "only": false})),
        }
    }

    fn close(&self) {
        unsafe {
            // Re-enable the owner first so Windows activates it, not another app.
            let _ = EnableWindow(self.owner, true);
            let _ = DestroyWindow(self.hwnd);
        }
    }

    /// Writes an appearance change the slider or picker left pending.
    fn flush_save(&self, app: &App) {
        if self.save_pending.replace(false) {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_SAVE);
            }
            app.save();
        }
    }

    fn on_destroy(&self) {
        if let Some(app) = app() {
            self.flush_save(&app);
        }
        if let Some(c) = self.controller.borrow_mut().take() {
            unsafe {
                let _ = c.Close();
            }
        }
        self.webview.borrow_mut().take();
        unsafe {
            let _ = EnableWindow(self.owner, true);
        }
        PANEL.with(|p| p.borrow_mut().take());
    }

    fn send_state(&self, app: &App) {
        self.refresh_posted.set(false);
        let cfg = app.config.borrow().clone();
        let tabs: Vec<Value> = app
            .all_views()
            .iter()
            .map(|tv| {
                let t = tv.tab.borrow();
                json!({
                    "id": tv.id,
                    "name": t.name,
                    "url": t.url,
                    "notifications": t.notifications,
                    "label": t.label,
                    "isolated": t.session.is_some(),
                    "user_agent": t.user_agent,
                    "icon": cached_icon(&t.url),
                })
            })
            .collect();
        let presets: Vec<Value> = PRESETS
            .iter()
            .map(|p| json!({"name": p.name, "url": p.url, "icon": cached_icon(p.url)}))
            .collect();
        let hex = |c: u32| format!("#{c:06x}");
        // The preview draws exactly what the bar will: shared colours and geometry.
        let tints: Vec<Value> = tint_names()
            .iter()
            .filter_map(|name| {
                let t = Tint::parse(Some(name))?;
                Some(json!({
                    "name": name,
                    "label": tint_label(Some(name), ""),
                    "light": hex(t.color(false)),
                    "dark": hex(t.color(true)),
                }))
            })
            .collect();
        let metrics: serde_json::Map<String, Value> = ICON_SIZES
            .map(|size| {
                let m = BarMetrics::new(size);
                (
                    size.to_string(),
                    json!({"icon": m.icon, "height": m.height, "tile": m.tile,
                           "tile_y": m.tile_y, "radius": m.radius}),
                )
            })
            .collect();
        let app_icon = (!self.icon_sent.replace(true))
            .then(|| format!("data:image/png;base64,{}", base64(APP_ICON)));
        self.post(json!({
            "cmd": "state",
            "options": {
                "notifications": cfg.notifications,
                "tray": cfg.tray,
                "close_to_tray": cfg.close_to_tray,
                "unread_in_title": cfg.unread_in_title,
                "unread_in_dock": cfg.unread_in_dock,
                "gpu_acceleration": cfg.gpu_acceleration,
                "icon_size": cfg.icon_size,
                "tint": cfg.tint,
            },
            "autostart": autostart::is_enabled(),
            "tabs": tabs,
            "presets": presets,
            "ua_presets": USER_AGENT_PRESETS,
            "tints": tints,
            "bar_light": hex(paint::BAR_LIGHT),
            "bar_dark": hex(paint::BAR_DARK),
            "metrics": metrics,
            "icon_min": ICON_SIZES.start(),
            "icon_max": ICON_SIZES.end(),
            "version": env!("CARGO_PKG_VERSION"),
            "app_icon": app_icon,
        }));
    }

    /// Presets without a cached icon get one fetched, like the Linux dialog.
    fn fetch_preset_icons(&self) {
        for (i, p) in PRESETS.iter().enumerate() {
            if cached_icon(p.url).is_none() {
                icons::fetch(
                    favicon::candidates(p.url, "[]"),
                    self.hwnd,
                    WM_APP_PRESET_ICON,
                    i as u64,
                    PRESET_GEN,
                );
            }
        }
    }

    fn on_page(&self, app: &App, m: &Value) {
        let text = |k: &str| m[k].as_str().unwrap_or("").to_string();
        let flag = |k: &str| m[k].as_bool().unwrap_or(false);
        let tab = || m["id"].as_u64().and_then(|id| app.tab(id));
        match m["cmd"].as_str().unwrap_or("") {
            "ready" => {
                self.ready.set(true);
                self.send_state(app);
                match self.pending.take() {
                    Some(Open::Add) => self.post(json!({"cmd": "form", "id": null, "only": true})),
                    Some(Open::Edit(id)) => {
                        self.post(json!({"cmd": "form", "id": id, "only": true}))
                    }
                    _ => self.post(json!({"cmd": "settings"})),
                }
                self.fetch_preset_icons();
            }
            "view" => unsafe {
                let _ = SetWindowTextW(self.hwnd, &HSTRING::from(text("title")));
            },
            "set" => {
                let on = flag("value");
                let key = text("key");
                app.update_options(|c| match key.as_str() {
                    "notifications" => c.notifications = on,
                    "tray" => c.tray = on,
                    "close_to_tray" => c.close_to_tray = on,
                    "unread_in_title" => c.unread_in_title = on,
                    "unread_in_dock" => c.unread_in_dock = on,
                    "gpu_acceleration" => c.gpu_acceleration = on,
                    _ => {}
                });
            }
            "autostart" => {
                if let Err(e) = autostart::set(flag("on")) {
                    error(self.hwnd, "Could not change autostart", &e);
                }
                self.send_state(app); // shows what actually stuck
            }
            "tab_notify" => {
                if let Some(tv) = tab() {
                    app.set_tab_notifications(&tv, flag("on"));
                }
            }
            "move" => {
                if let Some(tv) = tab() {
                    app.move_tab(&tv, m["delta"].as_i64().unwrap_or(0) as isize);
                }
            }
            "remove" => {
                if let Some(tv) = tab() {
                    app.confirm_remove(&tv);
                }
            }
            "hint" => {
                let editing = tab().map(|tv| tv.tab.borrow().url.clone());
                let text = isolation_hint(app, &text("url"), editing.as_deref(), flag("isolated"));
                self.post(json!({"cmd": "hint", "text": text}));
            }
            "save_tab" => self.save_tab(app, m, tab()),
            // The slider and the colour picker: applied now, saved once they settle.
            "appearance" => {
                let size = m["icon_size"].as_u64();
                let tint = m
                    .get("tint")
                    .map(|t| t.as_str().and_then(config::normalize_tint));
                app.preview_options(|c| {
                    if let Some(size) = size {
                        c.icon_size = (size as u32).clamp(*ICON_SIZES.start(), *ICON_SIZES.end());
                    }
                    if let Some(tint) = tint {
                        c.tint = tint;
                    }
                });
                self.save_pending.set(true);
                unsafe {
                    SetTimer(Some(self.hwnd), TIMER_SAVE, SAVE_DELAY_MS, None);
                }
            }
            "reorder" => {
                if let Some(tv) = tab() {
                    app.move_tab_to(&tv, m["to"].as_u64().unwrap_or(0) as usize);
                }
            }
            "open_site" => web::open_external("https://github.com/ivanm/webtabs"),
            "open_vaan" => web::open_external("https://vaan.dev"),
            "open_license" => web::open_external("https://www.gnu.org/licenses/gpl-3.0.html"),
            "open_toml" => app.edit_config(),
            "close" => self.close(),
            other => eprintln!("webtabs: settings window: unknown message {other:?}"),
        }
    }

    /// Builds the tab exactly like Linux's dialog, then adds or applies it.
    fn save_tab(&self, app: &App, m: &Value, editing: Option<Rc<TabView>>) {
        let text = |k: &str| m[k].as_str().unwrap_or("");
        if m["id"].is_u64() && editing.is_none() {
            // Removed (e.g. tabs.toml edited) while the form was open.
            self.post(json!({"cmd": "tab_saved"}));
            return;
        }
        let current = editing.as_ref().map(|tv| tv.tab.borrow().clone());
        let user_agent = match text("ua") {
            "custom" => Some(text("ua_custom").to_string()),
            "default" | "" => None,
            preset => USER_AGENT_PRESETS
                .iter()
                .find(|(n, _)| *n == preset)
                .map(|(_, v)| v.to_string()),
        };
        let mut t =
            Tab::new(text("name"), text("url"), user_agent.as_deref()).with_label(text("label"));
        t.notifications = !m["mute"].as_bool().unwrap_or(false);
        t.session = if m["isolated"].as_bool().unwrap_or(false) {
            // Keep the existing storage when editing an already isolated tab.
            current
                .as_ref()
                .and_then(|c| c.session.clone())
                .or_else(|| {
                    let existing = app.all_tabs();
                    let taken: Vec<&str> = existing
                        .iter()
                        .filter_map(|t| t.session.as_deref())
                        .collect();
                    Some(config::new_session_key(&t.name, &taken))
                })
        } else {
            None
        };
        if !t.is_valid() {
            self.post(json!({"cmd": "tab_invalid"}));
            return;
        }
        match editing {
            Some(tv) => app.edit_tab(&tv, &t),
            None => app.add_tab(t),
        }
        self.post(json!({"cmd": "tab_saved"}));
    }
}

/// "Shares its login with your … tab": a second tab on the same site without
/// its own session would be signed in as the first one.
fn isolation_hint(app: &App, url: &str, editing_url: Option<&str>, isolated: bool) -> String {
    if isolated {
        return String::new();
    }
    let site_of = |u: &str| {
        url::Url::parse(u)
            .ok()
            .and_then(|u| u.host_str().map(links::site))
    };
    let Some(site) = site_of(&config::normalize_url(url)) else {
        return String::new();
    };
    app.all_tabs()
        .iter()
        .find(|t| {
            Some(t.url.as_str()) != editing_url
                && t.session.is_none()
                && site_of(&t.url).as_ref() == Some(&site)
        })
        .map(|t| {
            format!(
                "This would share its login with your \u{201c}{}\u{201d} tab. Turn on Own login in Options to use a different account.",
                t.name
            )
        })
        .unwrap_or_default()
}

fn error(owner: HWND, title: &str, detail: &str) {
    unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(detail),
            &HSTRING::from(title),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// A site's cached icon as a data: URL for the page.
fn cached_icon(page_url: &str) -> Option<String> {
    let bytes = std::fs::read(favicon::cache_path(page_url)?).ok()?;
    icons::Image::decode(&bytes)?;
    Some(data_url(&bytes))
}

fn data_url(bytes: &[u8]) -> String {
    let mime = match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xff, 0xd8, ..] => "image/jpeg",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'B', b'M', ..] => "image/bmp",
        [0, 0, 1, 0, ..] => "image/x-icon",
        _ => "application/octet-stream",
    };
    format!("data:{mime};base64,{}", base64(bytes))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(p) = panel_for(hwnd) else {
        if msg == WM_APP_PAGE {
            drop(unsafe { Box::from_raw(lparam.0 as *mut String) });
        } else if msg == WM_APP_PRESET_ICON {
            drop(unsafe { Box::from_raw(lparam.0 as *mut icons::Fetched) });
        }
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_SIZE => p.layout(),
        WM_ERASEBKGND => {
            p.paint_background(HDC(wparam.0 as *mut _));
            return LRESULT(1);
        }
        WM_TIMER if wparam.0 == TIMER_SAVE => {
            if let Some(app) = app() {
                p.flush_save(&app);
            }
        }
        WM_GETMINMAXINFO => {
            let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            info.ptMinTrackSize = POINT {
                x: 420 * dpi / 96,
                y: 360 * dpi / 96,
            };
        }
        WM_DPICHANGED => {
            let rc = unsafe { &*(lparam.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rc.left,
                    rc.top,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        WM_MOVE => {
            if let Some(c) = p.controller.borrow().as_ref() {
                unsafe {
                    let _ = c.NotifyParentWindowPositionChanged();
                }
            }
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xffff) as u32 != WA_INACTIVE
                && let Some(c) = p.controller.borrow().as_ref()
            {
                web::focus(c);
            }
        }
        WM_APP_PAGE => {
            let json = unsafe { Box::from_raw(lparam.0 as *mut String) };
            match (serde_json::from_str::<Value>(&json), app()) {
                (Ok(m), Some(app)) => p.on_page(&app, &m),
                (Err(e), _) => eprintln!("webtabs: settings window: bad message: {e}"),
                _ => {}
            }
        }
        WM_APP_REFRESH => {
            if let Some(app) = app() {
                p.send_state(&app);
            }
        }
        WM_APP_PRESET_ICON => {
            let fetched = unsafe { Box::from_raw(lparam.0 as *mut icons::Fetched) };
            if let Some(preset) = PRESETS.get(wparam.0)
                && !fetched.bytes.is_empty()
            {
                icons::store(preset.url, &fetched.bytes);
                p.post(json!({"cmd": "preset_icon", "index": wparam.0, "src": data_url(&fetched.bytes)}));
            }
        }
        WM_CLOSE => p.close(),
        WM_DESTROY => p.on_destroy(),
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn data_url_sniffs_image_types() {
        assert!(data_url(b"\x89PNG\r\n").starts_with("data:image/png;base64,"));
        assert!(data_url(&[0, 0, 1, 0, 1]).starts_with("data:image/x-icon;base64,"));
    }
}
