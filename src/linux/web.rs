//! WebKitGTK plumbing: one shared persistent context, per-tab webviews, popups,
//! permissions and link routing.

use crate::config::{cache_dir, data_dir};
use crate::links::{self, Target};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::path::PathBuf;
use std::rc::Rc;
use webkit2gtk::{
    CookieManagerExt, NavigationPolicyDecisionExt, PermissionRequestExt, PolicyDecisionExt,
    SettingsExt, URIRequestExt, WebContextExt, WebViewExt, WebsiteDataManagerExt,
};
use webkit2gtk::{
    CookiePersistentStorage, DeviceInfoPermissionRequest, MediaKeySystemPermissionRequest,
    MemoryPressureSettings, NavigationPolicyDecision, NavigationType,
    NotificationPermissionRequest, PermissionRequest, PointerLockPermissionRequest,
    PolicyDecisionType, Settings, UserMediaPermissionRequest, WebContext,
    WebProcessTerminationReason, WebView, WebsiteDataAccessPermissionRequest, WebsiteDataManager,
};

unsafe extern "C" {
    // WebKitGTK >= 2.40; not exposed by webkit2gtk-sys yet.
    fn webkit_website_data_manager_set_favicons_enabled(
        manager: *mut webkit2gtk::ffi::WebKitWebsiteDataManager,
        enabled: glib::ffi::gboolean,
    );
}

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
    unsafe {
        webkit_website_data_manager_set_favicons_enabled(manager.as_ptr(), glib::ffi::GTRUE);
    }
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

/// Makes each web process shed caches and run GC early instead of growing to the
/// default ~1 GB before reacting. No kill threshold: a tab is never terminated.
fn memory_pressure() -> MemoryPressureSettings {
    let mut mp = MemoryPressureSettings::new();
    mp.set_memory_limit(1536); // MiB per web process
    mp.set_conservative_threshold(0.25); // start trimming at ~384 MiB
    mp.set_strict_threshold(0.5); // aggressive at ~768 MiB
    mp.set_poll_interval(5.0);
    mp
}

pub fn new_settings(user_agent: &str) -> Settings {
    let s = Settings::new();
    s.set_user_agent(Some(user_agent));
    s.set_enable_developer_extras(true);
    s.set_enable_media_stream(true);
    s.set_enable_webrtc(true);
    s.set_enable_mediasource(true);
    s.set_enable_encrypted_media(true);
    s.set_enable_smooth_scrolling(true);
    s.set_javascript_can_open_windows_automatically(true);
    s.set_javascript_can_access_clipboard(true);
    s.set_media_playback_requires_user_gesture(false);
    // Back/forward page cache keeps whole pages alive; single-page apps don't need it.
    s.set_enable_page_cache(false);
    // WEBTABS_DEBUG=1 mirrors every page's JS console to stdout.
    s.set_enable_write_console_messages_to_stdout(std::env::var_os("WEBTABS_DEBUG").is_some());
    s
}

pub fn new_webview(ctx: &WebContext, settings: &Settings) -> WebView {
    let wv = WebView::builder()
        .web_context(ctx)
        .settings(settings)
        .build();
    setup_common(&wv);
    wv
}

/// Behaviour shared by tab webviews and popups.
fn setup_common(wv: &WebView) {
    wv.connect_permission_request(|_, req| {
        allow_or_deny(req);
        true
    });
    wv.connect_web_process_terminated(|wv, reason| {
        if reason != WebProcessTerminationReason::TerminatedByApi {
            eprintln!(
                "webtabs: web process for {:?} died ({reason:?}), reloading",
                wv.uri()
            );
            wv.reload();
        }
    });
}

/// These are the user's own apps: grant what chat/meeting apps need,
/// keep location and everything unknown denied.
fn allow_or_deny(req: &PermissionRequest) {
    let allowed = req.is::<NotificationPermissionRequest>()
        || req.is::<UserMediaPermissionRequest>()
        || req.is::<DeviceInfoPermissionRequest>()
        || req.is::<MediaKeySystemPermissionRequest>()
        || req.is::<PointerLockPermissionRequest>()
        || req.is::<WebsiteDataAccessPermissionRequest>();
    if allowed { req.allow() } else { req.deny() }
}

pub fn open_external(uri: &str) {
    if let Err(e) = gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>) {
        eprintln!("webtabs: cannot open {uri}: {e}");
    }
}

/// Routes link clicks and new windows. `tab_url` is the home URL of the owning
/// tab, used to decide what counts as "the same app".
pub type UrlFn = Rc<dyn Fn() -> String>;

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
            open_external(&uri);
            decision.ignore();
        }
        external
    });

    let url_fn = tab_url.clone();
    wv.connect_create(move |wv, _action| {
        let popup = WebView::with_related_view(wv);
        if let Some(s) = WebViewExt::settings(wv) {
            popup.set_settings(&s);
        }
        setup_common(&popup);
        setup_popup(&popup, url_fn.clone(), parent.clone());
        Some(popup.upcast())
    });
}

/// A popup is a small window for OAuth flows, Meet calls, "open in new window" etc.
/// If its first real navigation turns out to be an external link, hand it to the
/// system browser and drop the popup before it is ever shown.
fn setup_popup(popup: &WebView, tab_url: UrlFn, parent: gtk::Window) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_transient_for(Some(&parent));
    window.set_default_size(1000, 760);
    window.set_title("webtabs");
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
            open_external(&uri);
            decision.ignore();
            let win = win.clone();
            glib::idle_add_local_once(move || win.close());
            return true;
        }
        false
    });

    let win = window.clone();
    popup.connect_ready_to_show(move |_| win.show_all());
    let win = window.clone();
    popup.connect_close(move |_| win.close());
    let win = window.clone();
    popup.connect_title_notify(move |p| {
        win.set_title(
            p.title()
                .as_deref()
                .filter(|t| !t.is_empty())
                .unwrap_or("webtabs"),
        );
    });
    setup_routing(popup, tab_url, window.upcast());
}
