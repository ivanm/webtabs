//! Windows front end: a Win32 window with an icon tab bar, one WebView2 per tab,
//! tray icon with notifications, close-to-tray and a hot-reloaded config file.
//! Mirrors `src/linux/`; all product rules (config, unread parsing, link
//! routing, favicon ranking, presets) come from the shared core modules.

/// `WEBTABS_DEBUG=1` traces app events to stderr (release builds have no
/// console; redirect it to a file).
macro_rules! debug {
    ($($arg:tt)*) => {
        if std::env::var_os("WEBTABS_DEBUG").is_some() {
            eprintln!("webtabs: {}", format!($($arg)*));
        }
    };
}

mod icons;
mod paint;
mod web;

use crate::badge::Badge;
use crate::config::{self, Config, Tab};
use crate::{favicon, links, presets};
use ::windows::Win32::Foundation::*;
use ::windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow, ScreenToClient, UpdateWindow,
};
use ::windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, IStream};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::Threading::CreateMutexW;
use ::windows::Win32::UI::Controls::*;
use ::windows::Win32::UI::HiDpi::*;
use ::windows::Win32::UI::Input::KeyboardAndMouse::*;
use ::windows::Win32::UI::Shell::*;
use ::windows::Win32::UI::WindowsAndMessaging::*;
use ::windows::core::{HSTRING, Interface, PCWSTR, PWSTR, w};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::result::Result;
use std::time::{Duration, Instant, SystemTime};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;

const CLASS: PCWSTR = w!("WebTabsWindow");
const WM_APP_TRAY: u32 = WM_APP + 1;
/// Posted by a second instance: bring the window back.
const WM_APP_SHOW: u32 = WM_APP + 2;
/// A background icon download finished (wparam = tab id, lparam = Box<Fetched>).
const WM_APP_ICON: u32 = WM_APP + 3;
/// A webview shortcut to run outside WebView2's event (wparam = vk, lparam = modifiers).
const WM_APP_KEY: u32 = WM_APP + 4;
const TIMER_LOAD: usize = 1;
const TIMER_CONFIG: usize = 2;
const TRAY_ID: u32 = 1;
/// NIN_KEYSELECT (NIN_SELECT | NINF_KEY), missing from the bindings.
const NIN_KEYSELECT_V4: u32 = WM_USER + 1;

/// Crash-reload guard: more crashes than this within the window stops reloading.
const MAX_CRASHES: usize = 3;
const CRASH_WINDOW: Duration = Duration::from_secs(60);

// Menu command ids (TrackPopupMenu with TPM_RETURNCMD).
const CMD_SHOW: u32 = 1;
const CMD_EDIT_CONFIG: u32 = 2;
const CMD_NOTIFICATIONS: u32 = 3;
const CMD_CLOSE_TO_TRAY: u32 = 4;
const CMD_UNREAD_TITLE: u32 = 5;
const CMD_TRAY: u32 = 6;
const CMD_QUIT: u32 = 7;
const CMD_RELOAD: u32 = 20;
const CMD_HOME: u32 = 21;
const CMD_LEFT: u32 = 22;
const CMD_RIGHT: u32 = 23;
const CMD_REMOVE: u32 = 24;
const CMD_DEVTOOLS: u32 = 25;
const CMD_TAB_NOTIFY: u32 = 26;
const CMD_CUSTOM_TAB: u32 = 99;
const CMD_PRESET: u32 = 100;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

fn app() -> Option<Rc<App>> {
    APP.with(|a| a.borrow().clone())
}

fn with_tab(id: u64, f: impl FnOnce(&Rc<App>, &Rc<TabView>)) {
    if let Some(app) = app()
        && let Some(tv) = app.tab(id)
    {
        f(&app, &tv);
    }
}

pub fn run() -> std::process::ExitCode {
    let start_hidden = std::env::args().any(|a| a == "--hidden");
    unsafe {
        // The manifest already asks for this; harmless when it did.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    // One instance per user: a second launch just brings the window back.
    let _mutex = unsafe { CreateMutexW(None, false, w!("Local\\dev.vaan.webtabs")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if let Ok(hwnd) = unsafe { FindWindowW(CLASS, PCWSTR::null()) } {
            unsafe {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = PostMessageW(Some(hwnd), WM_APP_SHOW, WPARAM(0), LPARAM(0));
            }
        }
        return std::process::ExitCode::SUCCESS;
    }
    icons::startup_gdiplus();
    unsafe {
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        });
    }
    match App::new() {
        Ok(app) => {
            APP.with(|a| *a.borrow_mut() = Some(app.clone()));
            app.start(start_hidden);
        }
        Err(e) => {
            error_box("webtabs could not start", &e);
            return std::process::ExitCode::FAILURE;
        }
    }
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    APP.with(|a| a.borrow_mut().take());
    std::process::ExitCode::SUCCESS
}

fn error_box(title: &str, detail: &str) {
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(detail),
            &HSTRING::from(title),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Load {
    Idle,
    Creating,
    Ready,
}

pub struct TabView {
    id: u64,
    tab: RefCell<Tab>,
    controller: RefCell<Option<ICoreWebView2Controller>>,
    webview: RefCell<Option<ICoreWebView2>>,
    state: Cell<Load>,
    badge: Cell<Badge>,
    icon: RefCell<Option<icons::Image>>,
    icon_fetched: Cell<bool>,
    /// Bumped when the tab's URL changes, so a slow fetch for the old URL is ignored.
    icon_gen: Cell<u64>,
    notification: RefCell<Option<ICoreWebView2Notification>>,
    crashes: RefCell<Vec<Instant>>,
}

impl TabView {
    fn webview(&self) -> Option<ICoreWebView2> {
        self.webview.borrow().clone()
    }

    fn controller(&self) -> Option<ICoreWebView2Controller> {
        self.controller.borrow().clone()
    }

    fn tooltip(&self) -> String {
        let unread = match self.badge.get() {
            Badge::Count(n) => format!(" \u{2014} {n} unread"),
            Badge::Dot => " \u{2014} new activity".into(),
            Badge::None => String::new(),
        };
        format!("{}{unread}", self.tab.borrow().name)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Tab(usize),
    Add,
    Gear,
}

/// App shortcuts. Reload, zoom, back/forward, find and DevTools are WebView2's own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Tab(u8),
    Next,
    Prev,
    AddTab,
    Settings,
    Hide,
    Quit,
    Home,
}

const MOD_CTRL: usize = 1;
const MOD_SHIFT: usize = 2;
const MOD_ALT: usize = 4;

fn modifiers() -> usize {
    let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
    (if down(VK_CONTROL) { MOD_CTRL } else { 0 })
        | (if down(VK_SHIFT) { MOD_SHIFT } else { 0 })
        | (if down(VK_MENU) { MOD_ALT } else { 0 })
}

/// Shortcuts shared by the tab bar and every webview.
pub fn key_for(vk: u32, mods: usize) -> Option<Key> {
    let vk = VIRTUAL_KEY(vk as u16);
    let ch = |c: u8| vk.0 == u16::from(c);
    let (ctrl, shift, alt) = (
        mods & MOD_CTRL != 0,
        mods & MOD_SHIFT != 0,
        mods & MOD_ALT != 0,
    );
    if ctrl && !alt && (u16::from(b'1')..=u16::from(b'9')).contains(&vk.0) {
        return Some(Key::Tab((vk.0 - u16::from(b'0')) as u8));
    }
    Some(match (ctrl, shift, alt) {
        (true, false, false) if vk == VK_TAB || vk == VK_NEXT => Key::Next,
        (true, true, false) if vk == VK_TAB => Key::Prev,
        (true, false, false) if vk == VK_PRIOR => Key::Prev,
        (true, false, false) if ch(b'T') => Key::AddTab,
        (true, false, false) if vk == VK_OEM_COMMA => Key::Settings,
        (true, false, false) if ch(b'W') => Key::Hide,
        (true, false, false) if ch(b'Q') => Key::Quit,
        (false, false, true) if vk == VK_HOME => Key::Home,
        _ => return None,
    })
}

pub struct App {
    hwnd: HWND,
    tooltip: HWND,
    env: RefCell<Option<ICoreWebView2Environment>>,
    config_path: PathBuf,
    config: RefCell<Config>,
    last_saved: RefCell<String>,
    config_stamp: Cell<Option<SystemTime>>,
    tabs: RefCell<Vec<Rc<TabView>>>,
    current: Cell<u64>,
    next_id: Cell<u64>,
    hover: Cell<Option<Hit>>,
    tracking_mouse: Cell<bool>,
    tray_added: Cell<bool>,
    tray_icon: HICON,
    tray_icon_unread: HICON,
    balloon_icon: HICON,
    last_unread: Cell<u32>,
    /// Tab whose notification the current tray balloon shows.
    last_notified: Cell<u64>,
    taskbar_created: u32,
    tooltip_count: Cell<usize>,
}

impl App {
    fn new() -> Result<Rc<App>, String> {
        let config_path = config::config_path();
        let cfg = Config::load_or_create(&config_path)?;
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }
            .map_err(|e| e.message())?
            .into();
        let load_icon = |id: u16, px: i32| -> HICON {
            unsafe {
                LoadImageW(
                    Some(instance),
                    PCWSTR(id as usize as *const u16),
                    IMAGE_ICON,
                    px,
                    px,
                    LR_DEFAULTCOLOR,
                )
            }
            .map(|h| HICON(h.0))
            .unwrap_or_default()
        };
        let small = unsafe { GetSystemMetrics(SM_CXSMICON) };
        let big = unsafe { GetSystemMetrics(SM_CXICON) };
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hIcon: load_icon(1, big),
            hIconSm: load_icon(1, small),
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&wc) } == 0 {
            return Err("cannot register the window class".into());
        }
        web::register_popup_class(instance, load_icon(1, big));

        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS,
                w!("webtabs"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1280,
                860,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .map_err(|e| e.message())?;
        // CW_USEDEFAULT sizes are in physical pixels; scale to the monitor.
        // Fit the monitor's work area (small screens, big scale factors).
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi);
        }
        let work = mi.rcWork;
        let w = (1280 * dpi / 96).min(work.right - work.left);
        let h = (860 * dpi / 96).min(work.bottom - work.top);
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                work.left + (work.right - work.left - w) / 2,
                work.top + (work.bottom - work.top - h) / 2,
                w,
                h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        let tooltip = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                PCWSTR::null(),
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                Some(hwnd),
                None,
                Some(instance),
                None,
            )
        }
        .unwrap_or_default();

        let app = Rc::new(App {
            hwnd,
            tooltip,
            env: RefCell::default(),
            config_stamp: Cell::new(modified(&config_path)),
            config_path,
            last_saved: RefCell::new(cfg.to_toml()),
            config: RefCell::new(cfg.clone()),
            tabs: RefCell::default(),
            current: Cell::new(0),
            next_id: Cell::new(1),
            hover: Cell::new(None),
            tracking_mouse: Cell::new(false),
            tray_added: Cell::new(false),
            tray_icon: load_icon(1, small),
            tray_icon_unread: load_icon(2, small),
            balloon_icon: load_icon(1, big),
            last_unread: Cell::new(0),
            last_notified: Cell::new(0),
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            tooltip_count: Cell::new(0),
        });
        let tabs: Vec<_> = cfg.tabs.iter().map(|t| app.create_tab(t)).collect();
        *app.tabs.borrow_mut() = tabs;
        if let Some(first) = app.tabs.borrow().first() {
            app.current.set(first.id);
        }
        Ok(app)
    }

    /// Runs once `APP` is set, so every message from here on reaches `handle`.
    fn start(&self, start_hidden: bool) {
        self.apply_options();
        self.sync_tooltips();
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_CONFIG, 1000, None);
            if !start_hidden {
                let _ = ShowWindow(self.hwnd, SW_SHOWNORMAL);
                let _ = UpdateWindow(self.hwnd);
            }
        }
        web::create_environment(move |result| {
            let Some(app) = app() else { return };
            match result {
                Ok(env) => {
                    *app.env.borrow_mut() = Some(env);
                    app.load_tabs_progressively();
                }
                Err(e) => {
                    error_box("webtabs: WebView2 is not available", &e);
                    app.quit();
                }
            }
        });
    }

    // ---- window & chrome ---------------------------------------------------

    fn dpi(&self) -> i32 {
        unsafe { GetDpiForWindow(self.hwnd) }.max(96) as i32
    }

    /// Device pixels for `v` DIPs.
    fn px(&self, v: i32) -> i32 {
        v * self.dpi() / 96
    }

    fn client(&self) -> RECT {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        rc
    }

    fn bar_height(&self) -> i32 {
        self.px(paint::BAR_H)
    }

    fn content_rect(&self) -> RECT {
        let mut rc = self.client();
        rc.top = self.bar_height().min(rc.bottom);
        rc
    }

    fn cell_rect(&self, hit: Hit) -> RECT {
        let px = |v| self.px(v);
        let (y, size) = (px(paint::CELL_Y), px(paint::CELL));
        let x = match hit {
            Hit::Tab(i) => px(paint::PAD_X) + i as i32 * px(paint::CELL + paint::GAP),
            Hit::Add => {
                px(paint::PAD_X)
                    + self.tabs.borrow().len() as i32 * px(paint::CELL + paint::GAP)
                    + px(4)
            }
            Hit::Gear => self.client().right - px(paint::PAD_X) - size,
        };
        RECT {
            left: x,
            top: y,
            right: x + size,
            bottom: y + size,
        }
    }

    fn hit_test(&self, x: i32, y: i32) -> Option<Hit> {
        if y >= self.bar_height() {
            return None;
        }
        let n = self.tabs.borrow().len();
        let inside = |r: RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        (0..n)
            .map(Hit::Tab)
            .chain([Hit::Add, Hit::Gear])
            .find(|h| inside(self.cell_rect(*h)))
    }

    fn invalidate_bar(&self) {
        let mut rc = self.client();
        rc.bottom = self.bar_height();
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), Some(&rc), false);
        }
    }

    fn show(&self) {
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            } else {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
        self.layout();
        if let Some(c) = self.current_tab().and_then(|t| t.controller()) {
            web::focus(&c);
        }
    }

    fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    fn is_active(&self) -> bool {
        unsafe { GetForegroundWindow() == self.hwnd }
    }

    fn can_hide_to_tray(&self) -> bool {
        let cfg = self.config.borrow();
        cfg.tray && cfg.close_to_tray && self.tray_added.get()
    }

    fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn toggle_visible(&self) {
        if self.is_visible() && self.is_active() {
            self.hide();
        } else {
            self.show();
        }
    }

    fn quit(&self) {
        self.remove_tray();
        for tv in self.all_views() {
            if let Some(c) = tv.controller.borrow_mut().take() {
                unsafe {
                    let _ = c.Close();
                }
            }
        }
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    /// Positions every webview under the tab bar; only the current one is visible.
    fn layout(&self) {
        let minimized = unsafe { IsIconic(self.hwnd).as_bool() };
        let rc = self.content_rect();
        let current = self.current.get();
        for tv in self.all_views() {
            if let Some(c) = tv.controller() {
                unsafe {
                    if !minimized {
                        let _ = c.SetBounds(rc);
                    }
                    let _ = c.SetIsVisible(!minimized && tv.id == current);
                }
            }
        }
    }

    fn sync_tooltips(&self) {
        let mut info = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            hwnd: self.hwnd,
            ..Default::default()
        };
        unsafe {
            for i in 0..self.tooltip_count.get() {
                info.uId = i;
                SendMessageW(
                    self.tooltip,
                    TTM_DELTOOLW,
                    Some(WPARAM(0)),
                    Some(LPARAM(&info as *const _ as isize)),
                );
            }
        }
        let mut tools: Vec<(Hit, String)> = self
            .all_views()
            .iter()
            .enumerate()
            .map(|(i, t)| (Hit::Tab(i), t.tooltip()))
            .collect();
        tools.push((Hit::Add, "Add tab (Ctrl+T)".into()));
        tools.push((Hit::Gear, "Settings (Ctrl+,)".into()));
        for (i, (hit, text)) in tools.iter().enumerate() {
            let mut text: Vec<u16> = wide(text);
            text.push(0);
            info.uId = i;
            info.uFlags = TTF_SUBCLASS;
            info.rect = self.cell_rect(*hit);
            info.lpszText = PWSTR(text.as_mut_ptr());
            unsafe {
                SendMessageW(
                    self.tooltip,
                    TTM_ADDTOOLW,
                    Some(WPARAM(0)),
                    Some(LPARAM(&info as *const _ as isize)),
                );
            }
        }
        self.tooltip_count.set(tools.len());
    }

    fn set_hover(&self, hover: Option<Hit>) {
        if self.hover.replace(hover) != hover {
            self.invalidate_bar();
        }
    }

    // ---- keyboard --------------------------------------------------------------

    /// Runs a shortcut. Webview key presses arrive here through WM_APP_KEY, posted
    /// from WebView2's AcceleratorKeyPressed event (menus must not open inside it).
    fn run_key(&self, key: Key) {
        let n = self.tabs.borrow().len();
        let cur = self.current_index().unwrap_or(0);
        match key {
            Key::Tab(_) | Key::Next | Key::Prev if n == 0 => {}
            // Ctrl+9 is always the last tab, like browsers.
            Key::Tab(9) => self.select_index(n - 1),
            Key::Tab(d) => self.select_index((d as usize).saturating_sub(1).min(n - 1)),
            Key::Next => self.select_index((cur + 1) % n),
            Key::Prev => self.select_index((cur + n - 1) % n),
            Key::AddTab => self.add_tab_menu(None),
            Key::Settings => self.settings_menu(None, false),
            Key::Hide => {
                // Never hide into a tray that isn't there.
                if self.can_hide_to_tray() {
                    self.hide();
                }
            }
            Key::Quit => self.quit(),
            Key::Home => {
                if let Some(tv) = self.current_tab() {
                    self.go_home(&tv);
                }
            }
        }
    }

    // ---- tabs ----------------------------------------------------------------

    fn all_views(&self) -> Vec<Rc<TabView>> {
        self.tabs.borrow().clone()
    }

    fn tab(&self, id: u64) -> Option<Rc<TabView>> {
        self.tabs.borrow().iter().find(|t| t.id == id).cloned()
    }

    fn current_tab(&self) -> Option<Rc<TabView>> {
        self.tab(self.current.get())
    }

    fn current_index(&self) -> Option<usize> {
        self.tabs
            .borrow()
            .iter()
            .position(|t| t.id == self.current.get())
    }

    fn create_tab(&self, tab: &Tab) -> Rc<TabView> {
        let tv = Rc::new(TabView {
            id: self.next_id.replace(self.next_id.get() + 1),
            tab: RefCell::new(tab.clone()),
            controller: RefCell::default(),
            webview: RefCell::default(),
            state: Cell::new(Load::Idle),
            badge: Cell::new(Badge::None),
            icon: RefCell::default(),
            icon_fetched: Cell::new(false),
            icon_gen: Cell::new(0),
            notification: RefCell::default(),
            crashes: RefCell::default(),
        });
        if let Some((img, _)) = icons::load_cached(&tab.url) {
            // Cached icons are reused; they're refetched only when the URL changes.
            tv.icon_fetched.set(true);
            *tv.icon.borrow_mut() = Some(img);
        }
        tv
    }

    /// Starts the tab's webview (once). The controller is created asynchronously.
    fn load(&self, tv: &Rc<TabView>) {
        if tv.state.get() != Load::Idle {
            return;
        }
        let Some(env) = self.env.borrow().clone() else {
            return; // loads once the environment exists
        };
        tv.state.set(Load::Creating);
        let profile = web::profile_name(tv.tab.borrow().session.as_deref());
        let id = tv.id;
        web::create_controller(&env, self.hwnd, &profile, move |result| {
            let Some(app) = app() else { return };
            let Some(tv) = app.tab(id) else {
                // The tab was removed while its view was being created.
                if let Ok(c) = result {
                    unsafe {
                        let _ = c.Close();
                    }
                }
                return;
            };
            match result {
                Ok(c) => app.attach(&tv, c),
                Err(e) => {
                    eprintln!(
                        "webtabs: cannot create a view for {}: {e}",
                        tv.tab.borrow().url
                    );
                    tv.state.set(Load::Ready); // don't retry in a loop
                }
            }
        });
    }

    fn attach(&self, tv: &Rc<TabView>, controller: ICoreWebView2Controller) {
        let Ok(wv) = (unsafe { controller.CoreWebView2() }) else {
            tv.state.set(Load::Ready);
            return;
        };
        let (url, ua, profile) = {
            let cfg = self.config.borrow();
            let tab = tv.tab.borrow();
            (
                tab.url.clone(),
                web::user_agent_override(&cfg, &tab).map(String::from),
                web::profile_name(tab.session.as_deref()),
            )
        };
        web::configure(&wv, ua.as_deref());
        let id = tv.id;
        let home: web::UrlFn = Rc::new(move || {
            app()
                .and_then(|a| a.tab(id))
                .map(|t| t.tab.borrow().url.clone())
                .unwrap_or_default()
        });
        if let Some(env) = self.env.borrow().clone()
            && let Err(e) = web::setup_common(&env, &wv, home, profile, self.hwnd)
        {
            eprintln!("webtabs: webview setup: {}", e.message());
        }
        if let Err(e) = self.setup_tab_events(tv, &controller, &wv) {
            eprintln!("webtabs: webview events: {}", e.message());
        }
        *tv.controller.borrow_mut() = Some(controller);
        *tv.webview.borrow_mut() = Some(wv.clone());
        tv.state.set(Load::Ready);
        self.layout();
        if tv.id == self.current.get()
            && self.is_active()
            && let Some(c) = tv.controller()
        {
            web::focus(&c);
        }
        let navigate_to = url.clone();
        let target = wv.clone();
        web::pregrant_notifications(&wv, &url, move || web::navigate(&target, &navigate_to));
    }

    fn setup_tab_events(
        &self,
        tv: &Rc<TabView>,
        controller: &ICoreWebView2Controller,
        wv: &ICoreWebView2,
    ) -> ::windows::core::Result<()> {
        let id = tv.id;
        let mut token = 0i64;
        unsafe {
            controller.add_AcceleratorKeyPressed(
                &AcceleratorKeyPressedEventHandler::create(Box::new(|_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                    let mut vk = 0u32;
                    args.KeyEventKind(&mut kind)?;
                    args.VirtualKey(&mut vk)?;
                    let mods = modifiers();
                    if (kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                        || kind == COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN)
                        && key_for(vk, mods).is_some()
                    {
                        args.SetHandled(true)?;
                        if let Some(app) = app() {
                            let _ = PostMessageW(
                                Some(app.hwnd),
                                WM_APP_KEY,
                                WPARAM(vk as usize),
                                LPARAM(mods as isize),
                            );
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            wv.add_DocumentTitleChanged(
                &DocumentTitleChangedEventHandler::create(Box::new(move |sender, _| {
                    if let Some(wv) = sender {
                        let title = web::title(&wv);
                        with_tab(id, |app, tv| app.on_title(tv, &title));
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            wv.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |sender, _| {
                    let Some(wv) = sender else { return Ok(()) };
                    with_tab(id, |app, tv| {
                        // Wait until we're on the tab's own site: the first load is often a
                        // login redirect (accounts.google.com) whose icon isn't the app's.
                        let page = web::source(&wv);
                        let on_site = links::same_site(&page, &tv.tab.borrow().url);
                        if on_site && !tv.icon_fetched.replace(true) {
                            app.fetch_icon(tv, &wv, page);
                        }
                    });
                    Ok(())
                })),
                &mut token,
            )?;
            wv.add_ProcessFailed(
                &ProcessFailedEventHandler::create(Box::new(move |sender, args| {
                    let (Some(wv), Some(args)) = (sender, args) else {
                        return Ok(());
                    };
                    let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                    args.ProcessFailedKind(&mut kind)?;
                    if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
                        || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
                    {
                        with_tab(id, |_, tv| {
                            let mut recent = tv.crashes.borrow_mut();
                            recent.retain(|t| t.elapsed() < CRASH_WINDOW);
                            recent.push(Instant::now());
                            let url = tv.tab.borrow().url.clone();
                            if recent.len() > MAX_CRASHES {
                                eprintln!("webtabs: {url} keeps crashing; not reloading");
                            } else {
                                eprintln!("webtabs: web process for {url} died, reloading");
                                let _ = wv.Reload();
                            }
                        });
                    } else {
                        eprintln!("webtabs: webview process failed ({kind:?})");
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            match wv.cast::<ICoreWebView2_24>() {
                Err(e) => eprintln!(
                    "webtabs: no notification support in this WebView2: {}",
                    e.message()
                ),
                Ok(wv24) => {
                    wv24.add_NotificationReceived(
                        &NotificationReceivedEventHandler::create(Box::new(move |_, args| {
                            let Some(args) = args else { return Ok(()) };
                            args.SetHandled(true)?;
                            let n = args.Notification()?;
                            with_tab(id, |app, tv| app.notify(tv, &n));
                            Ok(())
                        })),
                        &mut token,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn select(&self, tv: &Rc<TabView>) {
        self.current.set(tv.id);
        self.layout();
        self.load(tv);
        self.invalidate_bar();
        if let Some(c) = tv.controller() {
            web::focus(&c);
        }
    }

    fn select_index(&self, i: usize) {
        let tv = self.tabs.borrow().get(i).cloned();
        if let Some(tv) = tv {
            self.select(&tv);
        }
    }

    fn go_home(&self, tv: &TabView) {
        if let Some(wv) = tv.webview() {
            web::navigate(&wv, &tv.tab.borrow().url);
        }
    }

    /// Show the first tab immediately, then wake the rest up one by one so the
    /// window is usable before every web process has started.
    fn load_tabs_progressively(&self) {
        if let Some(tv) = self.current_tab() {
            self.load(&tv);
        }
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_LOAD, 400, None);
        }
    }

    fn on_load_timer(&self) {
        let views = self.all_views();
        if views.iter().any(|t| t.state.get() == Load::Creating) {
            return; // one at a time
        }
        match views.iter().find(|t| t.state.get() == Load::Idle) {
            Some(tv) => self.load(tv),
            None => unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_LOAD);
            },
        }
    }

    fn move_tab(&self, tv: &Rc<TabView>, delta: isize) {
        {
            let mut tabs = self.tabs.borrow_mut();
            let Some(i) = tabs.iter().position(|t| t.id == tv.id) else {
                return;
            };
            let j = i as isize + delta;
            if j < 0 || j as usize >= tabs.len() {
                return;
            }
            tabs.swap(i, j as usize);
        }
        self.after_tabs_changed();
        self.save();
    }

    fn after_tabs_changed(&self) {
        self.sync_tooltips();
        self.invalidate_bar();
        self.refresh_unread();
        let rc = self.content_rect();
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), Some(&rc), true);
        }
    }

    fn remove_tab(&self, tv: &Rc<TabView>) {
        let idx = self.tabs.borrow().iter().position(|t| t.id == tv.id);
        let session = tv.tab.borrow().session.clone();
        self.tabs.borrow_mut().retain(|t| t.id != tv.id);
        self.detach(tv, session.as_deref());
        if self.current.get() == tv.id {
            self.current.set(0);
            let n = self.tabs.borrow().len();
            if let (Some(i), true) = (idx, n > 0) {
                self.select_index(i.min(n - 1));
            }
        }
        self.after_tabs_changed();
    }

    /// Closes a tab's webview. An isolated session nobody uses anymore is deleted
    /// with it (WebView2 removes the profile's data once its views are gone).
    fn detach(&self, tv: &TabView, session: Option<&str>) {
        let unused = session.is_some_and(|key| !self.session_in_use(key));
        if let Some(wv) = tv.webview.borrow_mut().take()
            && unused
            && let Ok(profile) = unsafe { wv.cast::<ICoreWebView2_13>().and_then(|w| w.Profile()) }
            && let Ok(profile8) = profile.cast::<ICoreWebView2Profile8>()
        {
            unsafe {
                let _ = profile8.Delete();
            }
        }
        if let Some(c) = tv.controller.borrow_mut().take() {
            unsafe {
                let _ = c.Close();
            }
        }
    }

    fn session_in_use(&self, key: &str) -> bool {
        self.tabs
            .borrow()
            .iter()
            .any(|t| t.tab.borrow().session.as_deref() == Some(key))
    }

    fn apply_tab(&self, tv: &TabView, new: &Tab) {
        let old = tv.tab.replace(new.clone());
        if old.url != new.url {
            tv.icon_fetched.set(false);
            tv.icon_gen.set(tv.icon_gen.get() + 1);
        }
        if let Some(wv) = tv.webview() {
            let cfg = self.config.borrow();
            let ua = web::user_agent_override(&cfg, new).map(String::from);
            drop(cfg);
            let ua_changed = web::set_user_agent(&wv, ua.as_deref());
            if old.url != new.url {
                web::navigate(&wv, &new.url);
            } else if ua_changed {
                unsafe {
                    let _ = wv.Reload();
                }
            }
        }
    }

    fn add_tab(&self, tab: Tab) {
        let tv = self.create_tab(&tab);
        self.tabs.borrow_mut().push(tv.clone());
        self.select(&tv);
        self.after_tabs_changed();
        self.save();
    }

    // ---- icons -----------------------------------------------------------------

    /// Reads the page's declared icons and downloads the best one on a worker thread.
    fn fetch_icon(&self, tv: &Rc<TabView>, wv: &ICoreWebView2, page_url: String) {
        let (hwnd, id, generation) = (self.hwnd, tv.id, tv.icon_gen.get());
        let handler = ExecuteScriptCompletedHandler::create(Box::new(move |_, json| {
            // ExecuteScript returns the JSON encoding of the result: a JSON string here.
            let links: String = serde_json::from_str(&json).unwrap_or_default();
            icons::fetch(
                favicon::candidates(&page_url, &links),
                hwnd,
                WM_APP_ICON,
                id,
                generation,
            );
            Ok(())
        }));
        unsafe {
            let _ = wv.ExecuteScript(&HSTRING::from(favicon::COLLECT_LINKS_JS), &handler);
        }
    }

    fn on_icon_fetched(&self, id: u64, fetched: icons::Fetched) {
        let Some(tv) = self.tab(id) else { return };
        if tv.icon_gen.get() != fetched.generation {
            return; // the tab's URL changed while this was downloading
        }
        if let Some(img) = icons::Image::decode(&fetched.bytes) {
            icons::store(&tv.tab.borrow().url, &fetched.bytes);
            *tv.icon.borrow_mut() = Some(img);
            self.invalidate_bar();
            return;
        }
        // Nothing declared decodes (SVG-only sites): use WebView2's own favicon.
        let Some(wv) = tv.webview() else { return };
        let Ok(wv15) = wv.cast::<ICoreWebView2_15>() else {
            return;
        };
        let mut uri = PWSTR::null();
        unsafe {
            let _ = wv15.FaviconUri(&mut uri);
        }
        if take_pwstr(uri).is_empty() {
            return; // WebView2 would hand back its generic globe; keep the letter icon
        }
        let generation = fetched.generation;
        let handler = GetFaviconCompletedHandler::create(Box::new(move |_, stream| {
            let Some(bytes) = stream.as_ref().and_then(read_stream) else {
                return Ok(());
            };
            with_tab(id, |app, tv| {
                if tv.icon_gen.get() == generation
                    && let Some(img) = icons::Image::decode(&bytes)
                {
                    icons::store(&tv.tab.borrow().url, &bytes);
                    *tv.icon.borrow_mut() = Some(img);
                    app.invalidate_bar();
                }
            });
            Ok(())
        }));
        unsafe {
            let _ = wv15.GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &handler);
        }
    }

    // ---- unread & notifications ---------------------------------------------

    fn on_title(&self, tv: &Rc<TabView>, title: &str) {
        let badge = Badge::from_title(title);
        if tv.badge.replace(badge) != badge {
            self.invalidate_bar();
            self.sync_tooltips();
            self.refresh_unread();
        }
    }

    fn refresh_unread(&self) {
        let views = self.all_views();
        let total: u32 = views.iter().map(|t| t.badge.get().count()).sum();
        let any = views.iter().any(|t| t.badge.get().is_unread());
        let in_title = self.config.borrow().unread_in_title;
        let title = if total > 0 && in_title {
            format!("webtabs ({total})")
        } else {
            "webtabs".into()
        };
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(title));
        }
        self.update_tray(any, total);
        if total > self.last_unread.get() && !self.is_active() {
            // Taskbar attention, like GTK's urgency hint.
            let info = FLASHWINFO {
                cbSize: size_of::<FLASHWINFO>() as u32,
                hwnd: self.hwnd,
                dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
                uCount: 0,
                dwTimeout: 0,
            };
            unsafe {
                let _ = FlashWindowEx(&info);
            }
        }
        self.last_unread.set(total);
    }

    fn notify(&self, tv: &Rc<TabView>, n: &ICoreWebView2Notification) {
        debug!("notification from {}", tv.tab.borrow().name);
        if !self.config.borrow().notifications || !tv.tab.borrow().notifications {
            return;
        }
        let is_current = self.current.get() == tv.id;
        if self.is_active() && is_current {
            debug!("not shown: the tab is in front");
            return; // you're looking at it
        }
        let read = |f: &dyn Fn(&mut PWSTR) -> ::windows::core::Result<()>| {
            let mut p = PWSTR::null();
            let _ = f(&mut p);
            take_pwstr(p)
        };
        let title = read(&|p| unsafe { n.Title(p) });
        let body = read(&|p| unsafe { n.Body(p) });
        let app_name = tv.tab.borrow().name.clone();
        let title = if title.is_empty() {
            app_name.clone()
        } else {
            title
        };
        let body = if body.is_empty() {
            app_name
        } else {
            format!("{body}\n\u{2014} {app_name}")
        };
        if self.show_balloon(&title, &body) {
            unsafe {
                let _ = n.ReportShown();
            }
            self.last_notified.set(tv.id);
            *tv.notification.borrow_mut() = Some(n.clone());
            // The page closing its notification withdraws ours if it's still showing.
            let id = tv.id;
            let handler =
                NotificationCloseRequestedEventHandler::create(Box::new(move |sender, _| {
                    with_tab(id, |app, tv| {
                        let latest = tv.notification.borrow().as_ref() == sender.as_ref();
                        if latest {
                            tv.notification.borrow_mut().take();
                            if app.last_notified.get() == id {
                                app.show_balloon("", "");
                            }
                        }
                    });
                    Ok(())
                }));
            let mut token = 0;
            unsafe {
                let _ = n.add_CloseRequested(&handler, &mut token);
            }
        }
    }

    fn on_balloon_click(&self) {
        self.show();
        let Some(tv) = self.tab(self.last_notified.get()) else {
            return;
        };
        self.select(&tv);
        if let Some(n) = tv.notification.borrow_mut().take() {
            unsafe {
                let _ = n.ReportClicked(); // lets the page jump to the chat/email
            }
        }
    }

    // ---- tray --------------------------------------------------------------------

    fn tray_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            ..Default::default()
        }
    }

    fn add_tray(&self) {
        if self.tray_added.get() {
            return;
        }
        let mut nid = self.tray_data();
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = self.tray_icon;
        fill(&mut nid.szTip, "webtabs");
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
                self.tray_added.set(true);
            }
        }
    }

    fn remove_tray(&self) {
        if self.tray_added.replace(false) {
            let nid = self.tray_data();
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
        }
    }

    fn update_tray(&self, any_unread: bool, total: u32) {
        if !self.tray_added.get() {
            return;
        }
        let mut nid = self.tray_data();
        nid.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        nid.hIcon = if any_unread {
            self.tray_icon_unread
        } else {
            self.tray_icon
        };
        let tip = match total {
            0 if any_unread => "webtabs \u{2014} new activity".to_string(),
            0 => "webtabs".to_string(),
            n => format!("webtabs \u{2014} {n} unread"),
        };
        fill(&mut nid.szTip, &tip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// A tray balloon: Windows 10/11 shows it as a toast attributed to the app and
    /// keeps it in the notification center. Empty text removes the current one.
    fn show_balloon(&self, title: &str, body: &str) -> bool {
        if !self.tray_added.get() {
            return false;
        }
        let mut nid = self.tray_data();
        nid.uFlags = NIF_INFO;
        fill(&mut nid.szInfoTitle, title);
        fill(&mut nid.szInfo, body);
        nid.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON;
        nid.hBalloonIcon = self.balloon_icon;
        let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() };
        debug!("balloon {title:?}: {ok}");
        ok
    }

    fn on_tray(&self, lparam: LPARAM, wparam: WPARAM) {
        let event = (lparam.0 as u32) & 0xffff;
        match event {
            NIN_SELECT | NIN_KEYSELECT_V4 => self.toggle_visible(),
            NIN_BALLOONUSERCLICK => self.on_balloon_click(),
            WM_CONTEXTMENU => {
                // NOTIFYICON_VERSION_4: wparam holds the anchor point.
                let x = (wparam.0 & 0xffff) as i16 as i32;
                let y = ((wparam.0 >> 16) & 0xffff) as i16 as i32;
                self.settings_menu(Some(POINT { x, y }), true);
            }
            _ => {}
        }
    }

    // ---- menus (the prototype's settings and add-tab UI) -------------------------

    fn popup_menu(&self, menu: HMENU, at: Option<POINT>) -> u32 {
        let pt = at.unwrap_or_else(|| {
            let mut p = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut p);
            }
            p
        });
        unsafe {
            // Required so the menu closes when clicking elsewhere (tray menus).
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                pt.x,
                pt.y,
                None,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            cmd.0 as u32
        }
    }

    fn below(&self, hit: Hit) -> POINT {
        let rc = self.cell_rect(hit);
        let mut pt = POINT {
            x: rc.left,
            y: rc.bottom,
        };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
        }
        pt
    }

    /// Gear / tray menu: show, edit tabs.toml, app-wide switches, quit.
    fn settings_menu(&self, at: Option<POINT>, from_tray: bool) {
        let cfg = self.config.borrow().clone();
        let at = at.or_else(|| (!from_tray).then(|| self.below(Hit::Gear)));
        let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
            return;
        };
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        unsafe {
            if from_tray {
                let label = if self.is_visible() {
                    "Hide webtabs"
                } else {
                    "Show webtabs"
                };
                let _ = AppendMenuW(menu, MF_STRING, CMD_SHOW as usize, &HSTRING::from(label));
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            }
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CMD_EDIT_CONFIG as usize,
                w!("Edit tabs.toml\u{2026}"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING | check(cfg.notifications),
                CMD_NOTIFICATIONS as usize,
                w!("Notifications"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING | check(cfg.unread_in_title),
                CMD_UNREAD_TITLE as usize,
                w!("Unread count in window title"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING | check(cfg.tray),
                CMD_TRAY as usize,
                w!("Tray icon"),
            );
            let close_flags = MF_STRING
                | check(cfg.close_to_tray)
                | if cfg.tray { MF_ENABLED } else { MF_GRAYED };
            let _ = AppendMenuW(
                menu,
                close_flags,
                CMD_CLOSE_TO_TRAY as usize,
                w!("Closing the window keeps webtabs in the tray"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT as usize, w!("Quit\tCtrl+Q"));
        }
        match self.popup_menu(menu, at) {
            CMD_SHOW => self.toggle_visible(),
            CMD_EDIT_CONFIG => self.edit_config(),
            CMD_NOTIFICATIONS => self.update_options(|c| c.notifications = !c.notifications),
            CMD_UNREAD_TITLE => self.update_options(|c| c.unread_in_title = !c.unread_in_title),
            CMD_TRAY => self.update_options(|c| c.tray = !c.tray),
            CMD_CLOSE_TO_TRAY => self.update_options(|c| c.close_to_tray = !c.close_to_tray),
            CMD_QUIT => self.quit(),
            _ => {}
        }
    }

    /// "+": the shared presets, or hand-editing tabs.toml for anything else.
    fn add_tab_menu(&self, at: Option<POINT>) {
        let at = at.unwrap_or_else(|| self.below(Hit::Add));
        let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
            return;
        };
        unsafe {
            for (i, p) in presets::PRESETS.iter().enumerate() {
                let _ = AppendMenuW(
                    menu,
                    MF_STRING,
                    (CMD_PRESET as usize) + i,
                    &HSTRING::from(p.name),
                );
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CMD_CUSTOM_TAB as usize,
                w!("Other site (edit tabs.toml)\u{2026}"),
            );
        }
        match self.popup_menu(menu, Some(at)) {
            CMD_CUSTOM_TAB => self.edit_config(),
            cmd if cmd >= CMD_PRESET => {
                if let Some(p) = presets::PRESETS.get((cmd - CMD_PRESET) as usize) {
                    let mut tab = Tab::new(p.name, p.url, None);
                    // A second account of the same service needs its own logins.
                    let existing: Vec<Tab> = self
                        .all_views()
                        .iter()
                        .map(|t| t.tab.borrow().clone())
                        .collect();
                    // Same rule as the Linux dialog's hint: a tab on the same site shares logins.
                    let site_of = |u: &str| {
                        url::Url::parse(u)
                            .ok()
                            .and_then(|u| u.host_str().map(links::site))
                    };
                    let site = site_of(&tab.url);
                    if site.is_some()
                        && existing
                            .iter()
                            .any(|t| t.session.is_none() && site_of(&t.url) == site)
                    {
                        let taken: Vec<&str> = existing
                            .iter()
                            .filter_map(|t| t.session.as_deref())
                            .collect();
                        tab.session = Some(config::new_session_key(&tab.name, &taken));
                    }
                    self.add_tab(tab);
                }
            }
            _ => {}
        }
    }

    fn tab_menu(&self, tv: &Rc<TabView>) {
        let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
            return;
        };
        let notify = tv.tab.borrow().notifications;
        unsafe {
            let _ = AppendMenuW(menu, MF_STRING, CMD_RELOAD as usize, w!("Reload\tCtrl+R"));
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CMD_HOME as usize,
                w!("Go to home page\tAlt+Home"),
            );
            let _ = AppendMenuW(menu, MF_STRING, CMD_LEFT as usize, w!("Move left"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_RIGHT as usize, w!("Move right"));
            let flags = MF_STRING | if notify { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(
                menu,
                flags,
                CMD_TAB_NOTIFY as usize,
                w!("Notifications from this tab"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CMD_DEVTOOLS as usize,
                w!("Inspect (DevTools)\tF12"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CMD_EDIT_CONFIG as usize,
                w!("Edit tabs.toml\u{2026}"),
            );
            let _ = AppendMenuW(menu, MF_STRING, CMD_REMOVE as usize, w!("Remove"));
        }
        match self.popup_menu(menu, None) {
            CMD_RELOAD => {
                if let Some(wv) = tv.webview() {
                    unsafe {
                        let _ = wv.Reload();
                    }
                }
            }
            CMD_HOME => self.go_home(tv),
            CMD_LEFT => self.move_tab(tv, -1),
            CMD_RIGHT => self.move_tab(tv, 1),
            CMD_TAB_NOTIFY => {
                tv.tab.borrow_mut().notifications = !notify;
                self.save();
            }
            CMD_DEVTOOLS => {
                if let Some(wv) = tv.webview() {
                    unsafe {
                        let _ = wv.OpenDevToolsWindow();
                    }
                }
            }
            CMD_EDIT_CONFIG => self.edit_config(),
            CMD_REMOVE => self.confirm_remove(tv),
            _ => {}
        }
    }

    fn confirm_remove(&self, tv: &Rc<TabView>) {
        let (name, session) = {
            let t = tv.tab.borrow();
            (t.name.clone(), t.session.clone())
        };
        let detail = if session.is_some() {
            "\n\nIts isolated login data is deleted too."
        } else {
            ""
        };
        let answer = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                &HSTRING::from(format!("Remove \"{name}\"?{detail}")),
                w!("webtabs"),
                MB_OKCANCEL | MB_ICONQUESTION,
            )
        };
        if answer == IDOK {
            self.remove_tab(tv);
            self.save();
        }
    }

    fn edit_config(&self) {
        let path = HSTRING::from(self.config_path.as_os_str());
        unsafe {
            ShellExecuteW(
                Some(self.hwnd),
                w!("open"),
                w!("notepad.exe"),
                &path,
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
    }

    // ---- options & config file sync ------------------------------------------

    fn update_options(&self, f: impl FnOnce(&mut Config)) {
        f(&mut self.config.borrow_mut());
        self.apply_options();
        self.save();
    }

    fn apply_options(&self) {
        if self.config.borrow().tray {
            self.add_tray();
        } else {
            self.remove_tray();
            if !self.is_visible() {
                self.show(); // never strand a hidden window without its tray icon
            }
        }
        self.refresh_unread();
    }

    fn save(&self) {
        let mut cfg = self.config.borrow().clone();
        cfg.tabs = self
            .all_views()
            .iter()
            .map(|t| t.tab.borrow().clone())
            .collect();
        *self.last_saved.borrow_mut() = cfg.to_toml();
        if let Err(e) = cfg.save(&self.config_path) {
            eprintln!("webtabs: cannot save {}: {e}", self.config_path.display());
        }
        self.config_stamp.set(modified(&self.config_path));
        *self.config.borrow_mut() = cfg;
    }

    /// Polled every second: Notepad (and most editors) replace the file on save.
    fn check_config(&self) {
        let stamp = modified(&self.config_path);
        if stamp == self.config_stamp.get() {
            return;
        }
        self.config_stamp.set(stamp);
        self.reload_config();
    }

    fn reload_config(&self) {
        let Ok(text) = std::fs::read_to_string(&self.config_path) else {
            return;
        };
        if text == *self.last_saved.borrow() {
            return; // our own write
        }
        let new = match Config::parse(&text) {
            Ok(c) => c,
            Err(e) => {
                self.show_balloon("webtabs: tabs.toml not applied", &e);
                return;
            }
        };
        *self.last_saved.borrow_mut() = text;
        *self.config.borrow_mut() = new.clone();

        // Reconcile: keep live webviews (and their logins/state) for URLs that survive.
        let mut old = self.tabs.take();
        let mut next = Vec::with_capacity(new.tabs.len());
        for tab in &new.tabs {
            let same = |t: &Rc<TabView>| {
                let t = t.tab.borrow();
                t.url == tab.url && t.session == tab.session
            };
            match old.iter().position(same) {
                Some(i) => {
                    let tv = old.remove(i);
                    self.apply_tab(&tv, tab);
                    next.push(tv);
                }
                None => next.push(self.create_tab(tab)),
            }
        }
        *self.tabs.borrow_mut() = next;
        for tv in &old {
            let session = tv.tab.borrow().session.clone();
            self.detach(tv, session.as_deref());
        }
        if self.current_tab().is_none() {
            let first = self.tabs.borrow().first().cloned();
            match first {
                Some(tv) => self.select(&tv),
                None => self.current.set(0),
            }
        }
        self.apply_options();
        self.layout();
        self.after_tabs_changed();
        if self.env.borrow().is_some() {
            self.load_tabs_progressively();
        }
    }

    // ---- window procedure ---------------------------------------------------------

    fn handle(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_PAINT => paint::paint(self),
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_SIZE => {
                self.layout();
                self.sync_tooltips();
                self.invalidate_bar();
            }
            WM_MOVE | WM_MOVING => {
                for tv in self.all_views() {
                    if let Some(c) = tv.controller() {
                        unsafe {
                            let _ = c.NotifyParentWindowPositionChanged();
                        }
                    }
                }
            }
            WM_DPICHANGED => {
                let rc = unsafe { &*(lparam.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        self.hwnd,
                        None,
                        rc.left,
                        rc.top,
                        rc.right - rc.left,
                        rc.bottom - rc.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            WM_ACTIVATE => {
                if (wparam.0 & 0xffff) as u32 != WA_INACTIVE {
                    let info = FLASHWINFO {
                        cbSize: size_of::<FLASHWINFO>() as u32,
                        hwnd: self.hwnd,
                        dwFlags: FLASHW_STOP,
                        ..Default::default()
                    };
                    unsafe {
                        let _ = FlashWindowEx(&info);
                    }
                }
            }
            WM_SETFOCUS => {
                if let Some(c) = self.current_tab().and_then(|t| t.controller()) {
                    web::focus(&c);
                }
            }
            WM_SETCURSOR => {
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                    let _ = ScreenToClient(self.hwnd, &mut pt);
                }
                if (lparam.0 & 0xffff) as u32 == HTCLIENT && self.hit_test(pt.x, pt.y).is_some() {
                    unsafe {
                        SetCursor(LoadCursorW(None, IDC_HAND).ok());
                    }
                    return Some(LRESULT(1));
                }
                return None;
            }
            WM_MOUSEMOVE => {
                if !self.tracking_mouse.replace(true) {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe {
                        let _ = TrackMouseEvent(&mut tme);
                    }
                }
                let (x, y) = point(lparam);
                self.set_hover(self.hit_test(x, y));
            }
            WM_MOUSELEAVE => {
                self.tracking_mouse.set(false);
                self.set_hover(None);
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lparam);
                match self.hit_test(x, y) {
                    Some(Hit::Tab(i)) => self.select_index(i),
                    Some(Hit::Add) => self.add_tab_menu(None),
                    Some(Hit::Gear) => self.settings_menu(None, false),
                    None => {}
                }
            }
            WM_RBUTTONUP => {
                let (x, y) = point(lparam);
                match self.hit_test(x, y) {
                    Some(Hit::Tab(i)) => {
                        let tv = self.tabs.borrow().get(i).cloned();
                        if let Some(tv) = tv {
                            self.tab_menu(&tv);
                        }
                    }
                    _ => self.settings_menu(None, false),
                }
            }
            WM_MBUTTONUP => {
                let (x, y) = point(lparam);
                if let Some(Hit::Tab(i)) = self.hit_test(x, y) {
                    let wv = self.tabs.borrow().get(i).and_then(|t| t.webview());
                    if let Some(wv) = wv {
                        unsafe {
                            let _ = wv.Reload();
                        }
                    }
                }
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => self.run_key(key_for(wparam.0 as u32, modifiers())?),
            WM_APP_KEY => {
                if let Some(key) = key_for(wparam.0 as u32, lparam.0 as usize) {
                    self.run_key(key);
                }
            }
            WM_TIMER => match wparam.0 {
                TIMER_LOAD => self.on_load_timer(),
                TIMER_CONFIG => self.check_config(),
                _ => {}
            },
            WM_APP_TRAY => self.on_tray(lparam, wparam),
            WM_APP_SHOW => self.show(),
            WM_APP_ICON => {
                let fetched = unsafe { Box::from_raw(lparam.0 as *mut icons::Fetched) };
                self.on_icon_fetched(wparam.0 as u64, *fetched);
            }
            WM_CLOSE => {
                if self.can_hide_to_tray() {
                    self.hide();
                } else {
                    self.quit();
                }
            }
            WM_DESTROY => {
                self.remove_tray();
                unsafe { PostQuitMessage(0) };
            }
            m if m == self.taskbar_created && m != 0 => {
                // Explorer restarted: the tray icon is gone, put it back.
                self.tray_added.set(false);
                self.apply_options();
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A message for a window being destroyed may arrive while the app is borrowed
    // during startup/shutdown; fall back to the default handling then.
    if let Some(app) = app().filter(|a| a.hwnd == hwnd)
        && let Some(r) = app.handle(msg, wparam, lparam)
    {
        return r;
    }
    if msg == WM_CLOSE {
        // Before the app exists (startup error) there's nothing to keep alive.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xffff) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
    (x, y)
}

fn modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Copies `s` into a fixed, NUL-terminated UTF-16 buffer, truncating.
fn fill(buf: &mut [u16], s: &str) {
    let n = buf.len() - 1;
    let mut i = 0;
    for u in s.encode_utf16().take(n) {
        buf[i] = u;
        i += 1;
    }
    buf[i] = 0;
}

fn read_stream(stream: &IStream) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let mut read = 0u32;
        let hr = unsafe {
            stream.Read(
                buf.as_mut_ptr() as *mut _,
                buf.len() as u32,
                Some(&mut read),
            )
        };
        if hr.is_err() {
            return None;
        }
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buf[..read as usize]);
        if out.len() > 1024 * 1024 {
            return None;
        }
    }
    (!out.is_empty()).then_some(out)
}
