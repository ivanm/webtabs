//! WebKitGTK plumbing: one shared persistent context, per-tab webviews, popups,
//! permissions and link routing.

use crate::config::{cache_dir, data_dir};
use crate::links::{self, Target};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use webkit2gtk::{
    CookieManagerExt, NavigationPolicyDecisionExt, PermissionRequestExt, PolicyDecisionExt,
    SettingsExt, URIRequestExt, WebContextExt, WebViewExt, WebsiteDataAccessPermissionRequestExt,
    WebsiteDataManagerExt,
};
use webkit2gtk::{
    CookiePersistentStorage, DeviceInfoPermissionRequest, HardwareAccelerationPolicy,
    MediaKeySystemPermissionRequest, MemoryPressureSettings, NavigationPolicyDecision,
    NavigationType, NotificationPermissionRequest, PermissionRequest, PointerLockPermissionRequest,
    PolicyDecisionType, Settings, UserMediaPermissionRequest, WebContext,
    WebProcessTerminationReason, WebView, WebsiteDataAccessPermissionRequest, WebsiteDataManager,
};

/// Storage for a session: `None` is the shared profile every normal tab uses
/// (so SSO works across tabs); `Some(key)` is an isolated tab's private one.
pub fn session_dirs(session: Option<&str>) -> (PathBuf, PathBuf) {
    match session {
        None => (data_dir().join("web"), cache_dir()),
        Some(key) => {
            let root = data_dir().join("sessions").join(key);
            (root.join("web"), root.join("cache"))
        }
    }
}

/// A persistent browsing profile (cookies, local storage, IndexedDB) so logins
/// survive restarts.
pub fn new_context(session: Option<&str>) -> WebContext {
    let (data, cache) = session_dirs(session);
    let manager = WebsiteDataManager::builder()
        .base_data_directory(data.to_string_lossy().as_ref())
        .base_cache_directory(cache.to_string_lossy().as_ref())
        .build();
    if let Some(cookies) = manager.cookie_manager() {
        cookies.set_persistent_storage(
            &data.join("cookies.sqlite").to_string_lossy(),
            CookiePersistentStorage::Sqlite,
        );
        cookies.set_accept_policy(webkit2gtk::CookieAcceptPolicy::NoThirdParty);
    }
    let ctx = WebContext::builder()
        .website_data_manager(&manager)
        .memory_pressure_settings(&memory_pressure())
        .build();
    // Web apps live on JS + network data; a browser-sized memory cache only
    // grows the footprint. DocumentBrowser keeps a moderate disk cache.
    ctx.set_cache_model(webkit2gtk::CacheModel::DocumentBrowser);
    ctx.set_spell_checking_enabled(true);
    let langs: Vec<String> = glib::language_names()
        .iter()
        .map(|l| l.split('.').next().unwrap_or("").to_string())
        .filter(|l| l.len() >= 2 && l != "C" && l != "POSIX")
        .collect();
    if !langs.is_empty() {
        ctx.set_spell_checking_languages(&langs.iter().map(String::as_str).collect::<Vec<_>>());
    }
    ctx
}

/// Lets a web process that keeps growing shed caches and GC before it gets huge.
/// Thresholds sit above a logged-in WhatsApp (~600-700 MiB) so normal tabs are
/// never in a pressure band (where WebKit would run full GCs every poll).
/// No kill threshold: a tab is never terminated.
fn memory_pressure() -> MemoryPressureSettings {
    let mut mp = MemoryPressureSettings::new();
    mp.set_memory_limit(3072); // MiB per web process
    mp.set_conservative_threshold(0.33); // start trimming at ~1 GiB
    mp.set_strict_threshold(0.5); // aggressive at ~1.5 GiB
    mp.set_poll_interval(30.0);
    mp
}

thread_local! {
    static GPU: Cell<bool> = const { Cell::new(true) };
}

/// Fixed at startup: WebKit reads its renderer choice once per process.
pub fn set_gpu_acceleration(on: bool) {
    GPU.with(|g| g.set(on));
}

pub fn new_settings(user_agent: &str) -> Settings {
    let s = Settings::new();
    if !GPU.with(|g| g.get()) {
        s.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);
    }
    s.set_user_agent(Some(user_agent));
    s.set_enable_developer_extras(true);
    s.set_enable_media_stream(true);
    s.set_enable_webrtc(true);
    s.set_enable_mediasource(true);
    s.set_enable_encrypted_media(true);
    s.set_enable_smooth_scrolling(true);
    // Needed for OAuth/Meet popups; external opens still require a user gesture.
    // (No javascript_can_access_clipboard: it lets any page read the clipboard
    // without a click. User-initiated copy/paste works without it.)
    s.set_javascript_can_open_windows_automatically(true);
    s.set_media_playback_requires_user_gesture(false);
    // Back/forward page cache keeps whole pages alive; single-page apps don't need it.
    s.set_enable_page_cache(false);
    // WEBTABS_DEBUG=1 mirrors every page's JS console to stdout.
    s.set_enable_write_console_messages_to_stdout(std::env::var_os("WEBTABS_DEBUG").is_some());
    s
}

/// Home URL of the tab that owns a webview (or popup), read live so edits apply.
pub type UrlFn = Rc<dyn Fn() -> String>;

/// Crash-reload guard: more crashes than this within the window stops reloading.
const MAX_CRASHES: usize = 3;
const CRASH_WINDOW: Duration = Duration::from_secs(60);

pub fn new_webview(ctx: &WebContext, settings: &Settings, home: UrlFn) -> WebView {
    let wv = WebView::builder()
        .web_context(ctx)
        .settings(settings)
        .build();
    setup_common(&wv, home);
    wv
}

/// Behaviour shared by tab webviews and popups.
fn setup_common(wv: &WebView, home: UrlFn) {
    wv.connect_permission_request(move |wv, req| {
        allow_or_deny(wv, req, &home());
        true
    });
    let crashes: Rc<RefCell<Vec<Instant>>> = Rc::default();
    wv.connect_web_process_terminated(move |wv, reason| {
        if reason == WebProcessTerminationReason::TerminatedByApi {
            return;
        }
        let mut recent = crashes.borrow_mut();
        recent.retain(|t| t.elapsed() < CRASH_WINDOW);
        recent.push(Instant::now());
        if recent.len() > MAX_CRASHES {
            eprintln!(
                "webtabs: {:?} keeps crashing ({reason:?}); not reloading",
                wv.uri()
            );
            return;
        }
        eprintln!(
            "webtabs: web process for {:?} died ({reason:?}), reloading",
            wv.uri()
        );
        wv.reload();
    });
}

/// These are the user's own apps: grant what chat/meeting apps need, but only
/// to the tab's own site (a page reached through a redirect gets nothing).
/// Location and anything unknown are always denied.
fn allow_or_deny(wv: &WebView, req: &PermissionRequest, home: &str) {
    let page = wv.uri().map(|u| u.to_string()).unwrap_or_default();
    let own_site = links::same_site(&page, home);
    let allowed = if let Some(r) = req.downcast_ref::<WebsiteDataAccessPermissionRequest>() {
        // Storage access for an embedded (third-party) frame on the tab's site.
        r.current_domain()
            .is_some_and(|d| links::same_site(&format!("https://{d}/"), home))
    } else if req.is::<NotificationPermissionRequest>()
        || req.is::<UserMediaPermissionRequest>()
        || req.is::<DeviceInfoPermissionRequest>()
    {
        own_site
    } else {
        req.is::<MediaKeySystemPermissionRequest>() || req.is::<PointerLockPermissionRequest>()
    };
    if allowed { req.allow() } else { req.deny() }
}

/// Hands a link to the desktop's default handler (http/https/mailto/tel only).
pub fn open_external(uri: &str) {
    if !links::is_safe_external(uri) {
        eprintln!("webtabs: refusing to open {uri}");
        return;
    }
    if let Err(e) = gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>) {
        eprintln!("webtabs: cannot open {uri}: {e}");
    }
}

pub fn setup_routing(wv: &WebView, tab_url: UrlFn, parent: gtk::Window) {
    let url_fn = tab_url.clone();
    wv.connect_decide_policy(move |_, decision, kind| {
        let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() else {
            return false;
        };
        let Some(mut action) = nav.navigation_action() else {
            return false;
        };
        let Some(uri) = action.request().and_then(|r| r.uri()) else {
            return false;
        };
        let main_frame =
            kind == PolicyDecisionType::NavigationAction && action.frame_name().is_none();
        let external = match kind {
            PolicyDecisionType::NewWindowAction => {
                links::route(&url_fn(), &uri) == Target::External
            }
            PolicyDecisionType::NavigationAction if main_frame => {
                // Only user-clicked links leave the app; redirects (SSO) stay in.
                action.navigation_type() == NavigationType::LinkClicked
                    && links::route(&url_fn(), &uri) == Target::External
            }
            _ => false,
        };
        if external {
            // A page can't push things out of the app on its own (popup spam,
            // custom-scheme launches): only a real click opens the browser.
            if action.is_user_gesture() {
                open_external(&uri);
            }
            decision.ignore();
        }
        external
    });

    let url_fn = tab_url.clone();
    wv.connect_create(move |wv, action| {
        let popup = WebView::with_related_view(wv);
        if let Some(s) = WebViewExt::settings(wv) {
            popup.set_settings(&s);
        }
        setup_common(&popup, url_fn.clone());
        setup_popup(
            &popup,
            url_fn.clone(),
            parent.clone(),
            action.is_user_gesture(),
        );
        Some(popup.upcast())
    });
}

/// A popup is a small window for OAuth flows, Meet calls, "open in new window" etc.
/// If its first real navigation turns out to be an external link, hand it to the
/// system browser (only if a click opened it) and drop the popup unshown.
fn setup_popup(popup: &WebView, tab_url: UrlFn, parent: gtk::Window, by_user: bool) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_transient_for(Some(&parent));
    window.set_default_size(1000, 760);
    window.set_title("WebTabs");
    window.add(popup);

    let url_fn = tab_url.clone();
    let win = window.clone();
    popup.connect_decide_policy(move |popup, decision, kind| {
        if kind != PolicyDecisionType::NavigationAction {
            return false;
        }
        let fresh = popup
            .uri()
            .is_none_or(|u| u.is_empty() || u == "about:blank");
        let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() else {
            return false;
        };
        let Some(uri) = nav
            .navigation_action()
            .and_then(|a| a.request())
            .and_then(|r| r.uri())
        else {
            return false;
        };
        if fresh && links::route(&url_fn(), &uri) == Target::External {
            if by_user {
                open_external(&uri);
            }
            decision.ignore();
            let win = win.clone();
            glib::idle_add_local_once(move || win.close());
            return true;
        }
        false
    });

    // Show only once real content commits: a popup whose load WebKit blocks
    // (e.g. file://) never appears, and is dropped after a grace period.
    let ready = Rc::new(Cell::new(false));
    let (win, r) = (window.clone(), ready.clone());
    popup.connect_ready_to_show(move |p| {
        r.set(true);
        if p.uri().is_some_and(|u| u.starts_with("http")) {
            win.show_all();
        }
    });
    let (win, r) = (window.clone(), ready.clone());
    popup.connect_load_changed(move |p, event| {
        if event == webkit2gtk::LoadEvent::Committed
            && r.get()
            && !win.is_visible()
            && p.uri().is_some_and(|u| u.starts_with("http"))
        {
            win.show_all();
        }
    });
    let win = window.downgrade();
    glib::timeout_add_local_once(Duration::from_secs(15), move || {
        if let Some(win) = win.upgrade()
            && !win.is_visible()
        {
            win.close();
        }
    });
    let win = window.clone();
    popup.connect_close(move |_| win.close());
    let win = window.clone();
    popup.connect_title_notify(move |p| {
        win.set_title(
            p.title()
                .as_deref()
                .filter(|t| !t.is_empty())
                .unwrap_or("WebTabs"),
        );
    });
    setup_routing(popup, tab_url, window.upcast());
}
