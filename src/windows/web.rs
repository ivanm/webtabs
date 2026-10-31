//! WebView2 plumbing: one shared environment (browser process), a profile per
//! session, per-tab controllers, popups, permissions and link routing.

use crate::config::Config;
use crate::config::Tab;
use crate::links::{self, Target};
use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use ::windows::Win32::System::SystemInformation::GetTickCount;
use ::windows::Win32::System::Threading::GetCurrentProcessId;
use ::windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use ::windows::Win32::UI::Shell::ShellExecuteW;
use ::windows::Win32::UI::WindowsAndMessaging::*;
use ::windows::core::{HSTRING, Interface, PCWSTR, PWSTR, w};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::result::Result;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;

/// Home URL of the tab that owns a webview (or popup), read live so edits apply.
pub type UrlFn = Rc<dyn Fn() -> String>;

/// Where WebView2 keeps its profiles. Local (not roaming) AppData: browsing
/// profiles are large and machine-specific. `WEBTABS_DATA` relocates it like on Linux.
pub fn user_data_dir() -> PathBuf {
    std::env::var_os("WEBTABS_DATA")
        .map(|d| PathBuf::from(d).join("WebView2"))
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("webtabs")
                .join("WebView2")
        })
}

/// WebView2 profile name for a session: every normal tab shares one profile (so
/// SSO works across tabs); `session = "x"` gets its own isolated one.
pub fn profile_name(session: Option<&str>) -> String {
    match session {
        None => "shared".into(),
        Some(key) => format!("s-{key}"),
    }
}

/// The engine is Chromium here, so the Safari UA the WebKit ports send would be
/// a lie that gets us WebKit-only code paths. Only explicit overrides apply.
pub fn user_agent_override<'a>(cfg: &'a Config, tab: &'a Tab) -> Option<&'a str> {
    tab.user_agent.as_deref().or(cfg.user_agent.as_deref())
}

pub fn create_environment(done: impl FnOnce(Result<ICoreWebView2Environment, String>) + 'static) {
    let dir = user_data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let options = CoreWebView2EnvironmentOptions::default();
    let done = Rc::new(RefCell::new(Some(done)));
    let finish = done.clone();
    let handler =
        CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, env| {
            let out = match (result, env) {
                (Ok(()), Some(env)) => Ok(env),
                (Err(e), _) => Err(e.message()),
                (Ok(()), None) => Err("no environment".into()),
            };
            if let Some(f) = finish.borrow_mut().take() {
                f(out);
            }
            Ok(())
        }));
    let options: ICoreWebView2EnvironmentOptions = options.into();
    let result = unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &HSTRING::from(dir.as_os_str()),
            &options,
            &handler,
        )
    };
    if let Err(e) = result
        && let Some(f) = done.borrow_mut().take()
    {
        f(Err(format!(
            "{} ({:#x}). Is the Microsoft Edge WebView2 Runtime installed?",
            e.message(),
            e.code().0
        )));
    }
}

/// Creates a controller (one browser view) hosted in `parent`, in `profile`.
pub fn create_controller(
    env: &ICoreWebView2Environment,
    parent: HWND,
    profile: &str,
    done: impl FnOnce(Result<ICoreWebView2Controller, String>) + 'static,
) {
    let done = Rc::new(RefCell::new(Some(done)));
    let finish = done.clone();
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
        move |result, controller| {
            let out = match (result, controller) {
                (Ok(()), Some(c)) => Ok(c),
                (Err(e), _) => Err(e.message()),
                (Ok(()), None) => Err("no controller".into()),
            };
            if let Some(f) = finish.borrow_mut().take() {
                f(out);
            }
            Ok(())
        },
    ));
    let result = (|| -> ::windows::core::Result<()> {
        let env10: ICoreWebView2Environment10 = env.cast()?;
        let options = unsafe { env10.CreateCoreWebView2ControllerOptions()? };
        unsafe {
            options.SetProfileName(&HSTRING::from(profile))?;
            env10.CreateCoreWebView2ControllerWithOptions(parent, &options, &handler)
        }
    })();
    if let Err(e) = result
        && let Some(f) = done.borrow_mut().take()
    {
        f(Err(e.message()));
    }
}

thread_local! {
    /// WebView2's own (Edge) user agent, read from the first webview before any
    /// override, so removing an override can put it back without a restart.
    static ENGINE_UA: std::cell::OnceCell<String> = const { std::cell::OnceCell::new() };
}

fn engine_ua(s2: &ICoreWebView2Settings2) -> Option<String> {
    ENGINE_UA.with(|cell| {
        if cell.get().is_none() {
            let mut cur = PWSTR::null();
            unsafe {
                let _ = s2.UserAgent(&mut cur);
            }
            let cur = take_pwstr(cur);
            if !cur.is_empty() {
                let _ = cell.set(cur);
            }
        }
        cell.get().cloned()
    })
}

pub fn configure(wv: &ICoreWebView2, user_agent: Option<&str>) {
    unsafe {
        if let Ok(s) = wv.Settings() {
            let _ = s.SetAreDevToolsEnabled(true);
            let _ = s.SetIsStatusBarEnabled(false);
            let _ = s.SetAreDefaultScriptDialogsEnabled(true);
            if let Ok(s2) = s.cast::<ICoreWebView2Settings2>() {
                // Capture the engine default before the first override hides it.
                let _ = engine_ua(&s2);
                if let Some(ua) = user_agent {
                    let _ = s2.SetUserAgent(&HSTRING::from(ua));
                }
            }
        }
    }
}

pub fn set_user_agent(wv: &ICoreWebView2, user_agent: Option<&str>) -> bool {
    unsafe {
        let Ok(s2) = wv
            .Settings()
            .and_then(|s| s.cast::<ICoreWebView2Settings2>())
        else {
            return false;
        };
        let mut cur = PWSTR::null();
        let _ = s2.UserAgent(&mut cur);
        let cur = take_pwstr(cur);
        // No override means WebView2's own user agent (WebView2 has no "reset").
        let Some(want) = user_agent.map(String::from).or_else(|| engine_ua(&s2)) else {
            return false;
        };
        want != cur && s2.SetUserAgent(&HSTRING::from(want.as_str())).is_ok()
    }
}

pub fn source(wv: &ICoreWebView2) -> String {
    let mut p = PWSTR::null();
    unsafe {
        let _ = wv.Source(&mut p);
    }
    take_pwstr(p)
}

pub fn title(wv: &ICoreWebView2) -> String {
    let mut p = PWSTR::null();
    unsafe {
        let _ = wv.DocumentTitle(&mut p);
    }
    take_pwstr(p)
}

pub fn navigate(wv: &ICoreWebView2, url: &str) {
    unsafe {
        if let Err(e) = wv.Navigate(&HSTRING::from(url)) {
            eprintln!("webtabs: cannot load {url}: {}", e.message());
        }
    }
}

/// `https://host[:port]` for permission pre-grants.
fn origin_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let o = u.origin();
    o.is_tuple().then(|| o.ascii_serialization())
}

/// Pre-grants notifications to the tab's own origin so pages see
/// `Notification.permission === "granted"` from the first load (WhatsApp/Slack
/// otherwise show a nag banner), then calls `then`.
pub fn pregrant_notifications(wv: &ICoreWebView2, url: &str, then: impl FnOnce() + 'static) {
    let then = Rc::new(RefCell::new(Some(then)));
    let run = then.clone();
    let result = (|| -> ::windows::core::Result<()> {
        let origin = origin_of(url).ok_or_else(|| {
            ::windows::core::Error::from_hresult(::windows::Win32::Foundation::E_INVALIDARG)
        })?;
        let profile = unsafe { wv.cast::<ICoreWebView2_13>()?.Profile()? };
        let profile4: ICoreWebView2Profile4 = profile.cast()?;
        let handler = SetPermissionStateCompletedHandler::create(Box::new(move |_| {
            if let Some(f) = run.borrow_mut().take() {
                f();
            }
            Ok(())
        }));
        unsafe {
            profile4.SetPermissionState(
                COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
                &HSTRING::from(origin),
                COREWEBVIEW2_PERMISSION_STATE_ALLOW,
                &handler,
            )
        }
    })();
    if result.is_err()
        && let Some(f) = then.borrow_mut().take()
    {
        f();
    }
}

/// How recent a real click or key press must be for a page action to count as
/// the user's. Link clicks reach us within milliseconds.
const GESTURE_WINDOW_MS: u32 = 1500;

/// WebView2's `IsUserInitiated` follows Chromium's transient activation and also
/// covers navigations the host started, so on its own a page can push links out
/// of the app (seen: `window.open` 2 s after load reported as user-initiated).
/// Leaving the app also needs fresh real input while one of our windows is in front.
pub fn user_gesture(reported: bool) -> bool {
    if !reported {
        return false;
    }
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    let recent = unsafe {
        GetLastInputInfo(&mut info).as_bool()
            && GetTickCount().wrapping_sub(info.dwTime) < GESTURE_WINDOW_MS
    };
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
    }
    recent && pid == unsafe { GetCurrentProcessId() }
}

/// Hands a link to the desktop's default handler (http/https/mailto/tel only).
pub fn open_external(uri: &str) {
    debug!("open externally: {uri}");
    if !links::is_safe_external(uri) {
        eprintln!("webtabs: refusing to open {uri}");
        return;
    }
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(uri),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if r.0 as isize <= 32 {
        eprintln!("webtabs: cannot open {uri} ({})", r.0 as isize);
    }
}

/// These are the user's own apps: grant what chat/meeting apps need, but only
/// to the tab's own site (a page reached through a redirect gets nothing).
/// Location and anything unknown are always denied.
fn setup_permissions(wv: &ICoreWebView2, home: UrlFn) -> ::windows::core::Result<()> {
    let handler = PermissionRequestedEventHandler::create(Box::new(move |sender, args| {
        let (Some(wv), Some(args)) = (sender, args) else {
            return Ok(());
        };
        let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
        unsafe { args.PermissionKind(&mut kind)? };
        let own_site = links::same_site(&source(&wv), &home());
        let allow = match kind {
            COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS
            | COREWEBVIEW2_PERMISSION_KIND_CAMERA
            | COREWEBVIEW2_PERMISSION_KIND_MICROPHONE => own_site,
            COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY => true,
            _ => false,
        };
        unsafe {
            args.SetState(if allow {
                COREWEBVIEW2_PERMISSION_STATE_ALLOW
            } else {
                COREWEBVIEW2_PERMISSION_STATE_DENY
            })
        }
    }));
    let mut token = 0;
    unsafe { wv.add_PermissionRequested(&handler, &mut token) }
}

/// Behaviour shared by tab webviews and popups: permissions and link routing.
pub fn setup_common(
    env: &ICoreWebView2Environment,
    wv: &ICoreWebView2,
    home: UrlFn,
    profile: String,
    owner: HWND,
) -> ::windows::core::Result<()> {
    setup_permissions(wv, home.clone())?;

    // Main-frame navigations: only user-clicked links leave the app; redirects
    // (SSO) and script navigations stay in. Non-http schemes go through
    // LaunchingExternalUriScheme below.
    let url_fn = home.clone();
    let handler = NavigationStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut uri = PWSTR::null();
        let (mut user, mut redirected) = (Default::default(), Default::default());
        unsafe {
            args.Uri(&mut uri)?;
            args.IsUserInitiated(&mut user)?;
            args.IsRedirected(&mut redirected)?;
        }
        let uri = take_pwstr(uri);
        if uri.starts_with("http")
            && user_gesture(user.as_bool())
            && !redirected.as_bool()
            && links::route(&url_fn(), &uri) == Target::External
        {
            unsafe { args.SetCancel(true)? };
            open_external(&uri);
        }
        Ok(())
    }));
    let mut token = 0;
    unsafe { wv.add_NavigationStarting(&handler, &mut token)? };

    // mailto:, tel:, zoommtg:, slack:... WebView2 would ask "Open app?"; we hand
    // over the allowlisted ones on a real click and drop the rest.
    if let Ok(wv18) = wv.cast::<ICoreWebView2_18>() {
        let handler = LaunchingExternalUriSchemeEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            let mut user = Default::default();
            unsafe {
                args.Uri(&mut uri)?;
                args.IsUserInitiated(&mut user)?;
                args.SetCancel(true)?;
            }
            let uri = take_pwstr(uri);
            let user = user_gesture(user.as_bool());
            debug!("external scheme {uri} (user: {user})");
            if user {
                open_external(&uri);
            }
            Ok(())
        }));
        unsafe { wv18.add_LaunchingExternalUriScheme(&handler, &mut token)? };
    }

    // target=_blank and window.open: external sites go to the browser (only on a
    // click), the tab's own site and logins get an in-app popup.
    let env = env.clone();
    let handler = NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut uri = PWSTR::null();
        let mut user = Default::default();
        unsafe {
            args.Uri(&mut uri)?;
            args.IsUserInitiated(&mut user)?;
        }
        let uri = take_pwstr(uri);
        let reported = user.as_bool();
        let user = user_gesture(reported);
        debug!("new window {uri} (user: {user}, reported: {reported})");
        match links::route(&home(), &uri) {
            Target::External => {
                unsafe { args.SetHandled(true)? };
                if user {
                    open_external(&uri);
                }
            }
            Target::InApp => open_popup(&env, &args, home.clone(), profile.clone(), owner, user),
        }
        Ok(())
    }));
    unsafe { wv.add_NewWindowRequested(&handler, &mut token) }
}

// ---- popups ---------------------------------------------------------------------

const POPUP_CLASS: ::windows::core::PCWSTR = w!("WebTabsPopup");

struct Popup {
    controller: ICoreWebView2Controller,
}

thread_local! {
    static POPUPS: RefCell<HashMap<isize, Popup>> = RefCell::default();
}

pub fn register_popup_class(instance: ::windows::Win32::Foundation::HINSTANCE, icon: HICON) {
    let wc = WNDCLASSW {
        lpfnWndProc: Some(popup_proc),
        hInstance: instance,
        lpszClassName: POPUP_CLASS,
        hIcon: icon,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };
}

/// A popup is a small window for OAuth flows, Meet calls, "open in new window" etc.
/// If its first real navigation turns out to be an external link, hand it to the
/// system browser (only if a click opened it) and drop the popup unshown.
fn open_popup(
    env: &ICoreWebView2Environment,
    args: &ICoreWebView2NewWindowRequestedEventArgs,
    home: UrlFn,
    profile: String,
    owner: HWND,
    by_user: bool,
) {
    let Ok(deferral) = (unsafe { args.GetDeferral() }) else {
        return;
    };
    let (mut w, mut h) = (1000, 760);
    if let Ok(f) = unsafe { args.WindowFeatures() } {
        let mut has = Default::default();
        let (mut fw, mut fh) = (0u32, 0u32);
        unsafe {
            let _ = f.HasSize(&mut has);
            if has.as_bool() {
                let _ = f.Width(&mut fw);
                let _ = f.Height(&mut fh);
                if fw >= 200 && fh >= 150 {
                    (w, h) = (fw as i32, fh as i32);
                }
            }
        }
    }
    let scale = unsafe { ::windows::Win32::UI::HiDpi::GetDpiForWindow(owner) }.max(96) as i32;
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            POPUP_CLASS,
            w!("webtabs"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            w * scale / 96,
            h * scale / 96,
            Some(owner),
            None,
            None,
            None,
        )
    };
    let Ok(hwnd) = hwnd else {
        let _ = unsafe { deferral.Complete() };
        return;
    };
    let args = args.clone();
    let env2 = env.clone();
    let profile_name = profile.clone();
    create_controller(env, hwnd, &profile_name, move |result| {
        let done = |args: &ICoreWebView2NewWindowRequestedEventArgs| unsafe {
            let _ = args.SetHandled(true);
            let _ = deferral.Complete();
        };
        let controller = match result {
            Ok(c) => c,
            Err(e) => {
                eprintln!("webtabs: popup failed: {e}");
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return done(&args);
            }
        };
        let Ok(wv) = (unsafe { controller.CoreWebView2() }) else {
            unsafe {
                let _ = controller.Close();
                let _ = DestroyWindow(hwnd);
            }
            return done(&args);
        };
        // Same settings (user agent) as the opener.
        configure(&wv, None);
        let _ = setup_popup(&env2, &wv, hwnd, home, profile, owner, by_user);
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
            let _ = controller.SetBounds(rc);
            let _ = controller.SetIsVisible(true);
            let _ = args.SetNewWindow(&wv);
        }
        POPUPS.with(|p| {
            p.borrow_mut().insert(hwnd.0 as isize, Popup { controller });
        });
        done(&args);
    });
}

fn setup_popup(
    env: &ICoreWebView2Environment,
    wv: &ICoreWebView2,
    hwnd: HWND,
    home: UrlFn,
    profile: String,
    owner: HWND,
    by_user: bool,
) -> ::windows::core::Result<()> {
    let hwnd_raw = hwnd.0 as isize;
    let fresh = Rc::new(Cell::new(true));
    let url_fn = home.clone();
    let handler = NavigationStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        if !fresh.get() {
            return Ok(());
        }
        let mut uri = PWSTR::null();
        unsafe { args.Uri(&mut uri)? };
        let uri = take_pwstr(uri);
        if uri.is_empty() || uri == "about:blank" {
            return Ok(());
        }
        fresh.set(false);
        let hwnd = HWND(hwnd_raw as _);
        if links::route(&url_fn(), &uri) == Target::External {
            unsafe { args.SetCancel(true)? };
            if by_user {
                open_external(&uri);
            }
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        } else if uri.starts_with("http") {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            }
        }
        Ok(())
    }));
    let mut token = 0;
    unsafe { wv.add_NavigationStarting(&handler, &mut token)? };

    let handler = DocumentTitleChangedEventHandler::create(Box::new(move |sender, _| {
        if let Some(wv) = sender {
            let t = title(&wv);
            let t = if t.is_empty() {
                "webtabs".to_string()
            } else {
                t
            };
            unsafe {
                let _ = SetWindowTextW(HWND(hwnd_raw as _), &HSTRING::from(t));
            }
        }
        Ok(())
    }));
    unsafe { wv.add_DocumentTitleChanged(&handler, &mut token)? };

    let handler = WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd_raw as _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        Ok(())
    }));
    unsafe { wv.add_WindowCloseRequested(&handler, &mut token)? };

    // Drop popups whose first load never turns into a real page (e.g. blocked).
    unsafe {
        SetTimer(Some(hwnd), 1, 15_000, None);
    }
    setup_common(env, wv, home, profile, owner)
}

extern "system" fn popup_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            let controller = POPUPS.with(|p| {
                p.borrow()
                    .get(&(hwnd.0 as isize))
                    .map(|p| p.controller.clone())
            });
            if let Some(c) = controller {
                let mut rc = RECT::default();
                unsafe {
                    let _ = GetClientRect(hwnd, &mut rc);
                    let _ = c.SetBounds(rc);
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), wparam.0);
                if !IsWindowVisible(hwnd).as_bool() {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let popup = POPUPS.with(|p| p.borrow_mut().remove(&(hwnd.0 as isize)));
            if let Some(p) = popup {
                unsafe {
                    let _ = p.controller.Close();
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Moves keyboard focus into a webview.
pub fn focus(controller: &ICoreWebView2Controller) {
    unsafe {
        let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
    }
}
