//! Linux front end: GTK3 window with an icon tab bar, one WebKitGTK view per tab,
//! tray icon, desktop notifications, settings window and a hot-reloaded config file.

mod icons;
mod settings;
mod tab_dialog;
mod web;

use crate::badge::Badge;
use crate::config::{self, Config, Tab};
use crate::favicon;
use gtk::gdk::keys::constants as key;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use libappindicator::{AppIndicator, AppIndicatorStatus};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use webkit2gtk::{
    DownloadExt, LoadEvent, NotificationExt, SettingsExt, WebContextExt, WebInspectorExt,
    WebViewExt, WebsiteDataManagerExtManual,
};
use webkit2gtk::{WebContext, WebView};

const APP_ID: &str = "dev.vaan.webtabs";
const ICON: &str = include_str!("../../assets/webtabs.svg");
const ICON_UNREAD: &str = include_str!("../../assets/webtabs-unread.svg");
/// Tray mark (same as the macOS menu bar icon), light for dark panels.
const TRAY_ICON: &str = include_str!("../../assets/webtabs-tray.svg");
const TRAY_ICON_UNREAD: &str = include_str!("../../assets/webtabs-tray-unread.svg");
/// Desktop file the launcher badge is keyed by (see packaging/webtabs.desktop).
const LAUNCHER_APP_URI: &str = "application://webtabs.desktop";
const TAB_ICON_PX: i32 = 32;
const CSS: &str = "
.tabbar { padding: 2px 6px; border-bottom: 1px solid alpha(currentColor, 0.12); }
.tabbar button { min-height: 0; min-width: 0; }
.tabbar button.tab { padding: 4px 6px; margin: 2px 1px; border-radius: 10px;
                     border: none; background: none; box-shadow: none; }
.tabbar button.tab:hover { background: alpha(currentColor, 0.08); }
.tabbar button.tab.active { background: alpha(currentColor, 0.16); }
.tag { background: #3b82f6; color: #fff; border-radius: 5px; padding: 0 3px;
       font-size: 9px; font-weight: bold; min-height: 13px; }
.preset { padding: 10px 4px; border-radius: 12px; }
.badge { background: #e5484d; color: #fff; border-radius: 10px; padding: 0 4px;
         font-size: 10px; font-weight: bold; min-width: 10px; min-height: 16px; }
.tabbar button.bar { padding: 2px 10px; border-radius: 10px; }
.bar-glyph { font-size: 28px; opacity: 0.85; }
.caption { font-size: 0.85em; }
.empty-title { font-size: 1.4em; font-weight: bold; }
";

pub fn run() -> std::process::ExitCode {
    let start_hidden = std::env::args().any(|a| a == "--hidden");
    // WebKitGTK's DMA-BUF renderer can't allocate GBM buffers on the proprietary
    // NVIDIA driver and every page renders blank. Respect an explicit user setting.
    if std::path::Path::new("/proc/driver/nvidia").exists()
        && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
    {
        // SAFETY: still single-threaded; GTK and the web processes are not started yet.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
    glib::set_prgname(Some("webtabs"));
    glib::set_application_name("webtabs");
    let gapp = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::empty());
    let state: Rc<RefCell<Option<Rc<App>>>> = Rc::default();
    gapp.connect_activate(move |gapp| {
        if let Some(app) = state.borrow().as_ref() {
            app.show(); // second launch: just bring the window back
            return;
        }
        match App::new(gapp) {
            Ok(app) => {
                if !start_hidden {
                    app.show();
                }
                *state.borrow_mut() = Some(app);
            }
            Err(e) => {
                eprintln!("webtabs: {e}");
                gapp.quit();
            }
        }
    });
    // Our own flags (--hidden) are not GTK's business.
    if gapp.run_with_args::<&str>(&[]) == glib::ExitCode::SUCCESS {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

struct App {
    gapp: gtk::Application,
    window: gtk::ApplicationWindow,
    /// Holds one webview per tab plus the "empty" page.
    stack: gtk::Stack,
    /// The whole bar (tabs, "+", gear); hidden while a page is fullscreen.
    bar: gtk::Box,
    /// Tab buttons, in tab order; the "+" button sits right after it.
    tab_bar: gtk::Box,
    /// Tab whose page is fullscreen (0 = none).
    fullscreen: Cell<u64>,
    current: Cell<u64>,
    /// Shared profile used by every non-isolated tab.
    context: WebContext,
    /// Isolated tabs' private profiles, by session key.
    isolated: RefCell<HashMap<String, WebContext>>,
    config_path: PathBuf,
    config: RefCell<Config>,
    last_saved: RefCell<String>,
    tabs: RefCell<Vec<Rc<TabView>>>,
    tray: RefCell<Option<AppIndicator>>,
    next_id: Cell<u64>,
    reload_pending: Cell<bool>,
    last_unread: Cell<u32>,
    /// Last launcher badge sent (count, visible), so unchanged state isn't resent.
    last_launcher: Cell<Option<(u32, bool)>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    /// Set while the settings window is open, so App changes show up in it.
    settings_refresh: RefCell<Option<Rc<dyn Fn()>>>,
    settings_close_row: RefCell<Option<glib::WeakRef<gtk::ListBoxRow>>>,
    settings_window: RefCell<Option<glib::WeakRef<gtk::Dialog>>>,
    /// Isolated-session data scheduled for deletion; cancelled if the key returns.
    pending_deletes: RefCell<HashMap<String, glib::SourceId>>,
}

type TabAction = dyn Fn(&Rc<App>, &Rc<TabView>);

struct TabView {
    id: u64,
    tab: RefCell<Tab>,
    webview: WebView,
    button: gtk::Button,
    badge_label: gtk::Label,
    tag_label: gtk::Label,
    icon: gtk::Image,
    pixbuf: RefCell<Option<Pixbuf>>,
    icon_fetched: Cell<bool>,
    /// Bumped when the tab's URL changes, so a slow fetch for the old URL is ignored.
    icon_gen: Cell<u64>,
    badge: Cell<Badge>,
    loaded: Cell<bool>,
    notification: RefCell<Option<webkit2gtk::Notification>>,
}

impl TabView {
    fn load(&self) {
        if !self.loaded.replace(true) {
            self.webview.load_uri(&self.tab.borrow().url);
        }
    }

    fn set_icon(&self, p: &Pixbuf) {
        *self.pixbuf.borrow_mut() = Some(p.clone());
        icons::show(&self.icon, p, TAB_ICON_PX);
    }

    fn update_tag(&self) {
        let label = self.tab.borrow().label.clone();
        self.tag_label.set_text(label.as_deref().unwrap_or(""));
        self.tag_label.set_visible(label.is_some());
    }

    fn update_tooltip(&self) {
        let tab = self.tab.borrow();
        let unread = match self.badge.get() {
            Badge::Count(n) => format!(" \u{2014} {n} unread"),
            Badge::Dot => " \u{2014} new activity".into(),
            Badge::None => String::new(),
        };
        self.button
            .set_tooltip_text(Some(&format!("{}{unread}", tab.name)));
    }
}

impl App {
    fn new(gapp: &gtk::Application) -> Result<Rc<App>, String> {
        let config_path = config::config_path();
        let cfg = Config::load_or_create(&config_path)?;
        install_css();

        let window = gtk::ApplicationWindow::new(gapp);
        window.set_title("webtabs");
        window.set_default_size(1280, 860);
        let icon_dir = write_icons();
        if let Ok(pixbuf) = gtk::gdk_pixbuf::Pixbuf::from_file(icon_dir.join("webtabs.svg")) {
            window.set_icon(Some(&pixbuf));
        }

        // [tab][tab][tab][+] ............................ [gear]
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        bar.style_context().add_class("tabbar");
        let tab_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        // Text glyphs instead of symbolic icons: some icon themes (Papirus) draw
        // those tiny, while text always follows the theme's colour and our size.
        let bar_button = |glyph: &str, tip: &str| {
            let label = gtk::Label::new(Some(glyph));
            label.style_context().add_class("bar-glyph");
            let b = gtk::Button::new();
            b.add(&label);
            b.set_relief(gtk::ReliefStyle::None);
            b.set_valign(gtk::Align::Center);
            b.set_tooltip_text(Some(tip));
            b.style_context().add_class("bar");
            hand_cursor(&b);
            b
        };
        let add = bar_button("+", "Add tab (Ctrl+T)");
        let gear = bar_button("\u{2699}", "Settings (Ctrl+,)");
        let tabs_scroll =
            gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        tabs_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        tabs_scroll.set_propagate_natural_width(true);
        tabs_scroll.add(&tab_bar);
        bar.pack_start(&tabs_scroll, false, true, 0);
        bar.pack_start(&add, false, false, 4);
        bar.pack_end(&gear, false, false, 0);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        let (empty, empty_add) = empty_state();
        stack.add_named(&empty, "empty");
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.pack_start(&bar, false, false, 0);
        root.pack_start(&stack, true, true, 0);
        window.add(&root);

        let app = Rc::new(App {
            gapp: gapp.clone(),
            window,
            stack,
            bar,
            tab_bar,
            fullscreen: Cell::new(0),
            current: Cell::new(0),
            context: web::new_context(None),
            isolated: RefCell::default(),
            config_path,
            last_saved: RefCell::new(cfg.to_toml()),
            config: RefCell::new(cfg.clone()),
            tabs: RefCell::default(),
            tray: RefCell::default(),
            next_id: Cell::new(1),
            reload_pending: Cell::new(false),
            last_unread: Cell::new(0),
            last_launcher: Cell::default(),
            monitor: RefCell::default(),
            settings_refresh: RefCell::default(),
            settings_close_row: RefCell::default(),
            settings_window: RefCell::default(),
            pending_deletes: RefCell::default(),
        });

        let weak = Rc::downgrade(&app);
        add.connect_clicked(move |_| with(&weak, |a| a.add_tab_dialog()));
        let weak = Rc::downgrade(&app);
        empty_add.connect_clicked(move |_| with(&weak, |a| a.add_tab_dialog()));
        let weak = Rc::downgrade(&app);
        gear.connect_clicked(move |_| with(&weak, settings::open));
        app.setup_context(&app.context);
        app.setup_window();
        app.setup_actions();
        app.setup_tray(&icon_dir);
        app.watch_config();

        let tabs: Vec<_> = cfg.tabs.iter().map(|t| app.create_tab(t)).collect();
        *app.tabs.borrow_mut() = tabs;
        app.purge_stale_sessions();
        app.window.show_all();
        app.window.hide();
        app.update_empty_state();
        app.apply_options();
        app.load_tabs_progressively();
        Ok(app)
    }

    // ---- window & chrome ---------------------------------------------------

    fn show(&self) {
        self.window.show();
        self.window.present();
    }

    fn can_hide_to_tray(&self) -> bool {
        let cfg = self.config.borrow();
        cfg.tray && cfg.close_to_tray && self.tray.borrow().is_some()
    }

    fn toggle_visible(&self) {
        if self.window.is_visible() && self.window.is_active() {
            self.window.hide();
        } else {
            self.show();
        }
    }

    fn setup_window(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.window.connect_delete_event(move |win, _| {
            let Some(app) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if app.can_hide_to_tray() {
                win.hide();
            } else {
                app.gapp.quit();
            }
            glib::Propagation::Stop
        });
        self.window.connect_focus_in_event(|win, _| {
            win.set_urgency_hint(false);
            glib::Propagation::Proceed
        });

        let weak = Rc::downgrade(self);
        self.window.connect_key_press_event(move |_, ev| {
            let handled = weak.upgrade().is_some_and(|a| a.handle_key(ev));
            if handled {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }

    fn handle_key(self: &Rc<Self>, ev: &gdk::EventKey) -> bool {
        let mods = ev.state() & gtk::accelerator_get_default_mod_mask();
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = mods.contains(gdk::ModifierType::MOD1_MASK);
        let k = ev.keyval();
        let n = self.tabs.borrow().len();
        let cur = self.current_index().unwrap_or(0);
        let wv = self.current_tab().map(|t| t.webview.clone());

        if ctrl
            && !alt
            && let Some(d) = k
                .to_unicode()
                .and_then(|c| c.to_digit(10))
                .filter(|d| (1..=9).contains(d))
        {
            if n > 0 {
                // Ctrl+9 is always the last tab, like browsers.
                let idx = if d == 9 {
                    n - 1
                } else {
                    (d as usize - 1).min(n - 1)
                };
                self.select_index(idx);
            }
            return true;
        }
        match (ctrl, shift, alt) {
            (true, false, false) if k == key::Tab || k == key::Page_Down => {
                if n > 0 {
                    self.select_index((cur + 1) % n);
                }
            }
            (true, true, false) if k == key::ISO_Left_Tab || k == key::Tab => {
                if n > 0 {
                    self.select_index((cur + n - 1) % n);
                }
            }
            (true, false, false) if k == key::Page_Up => {
                if n > 0 {
                    self.select_index((cur + n - 1) % n);
                }
            }
            (true, false, false) if k == key::r => wv.iter().for_each(|w| w.reload()),
            (false, false, false) if k == key::F5 => wv.iter().for_each(|w| w.reload()),
            (true, true, false) if k == key::R => wv.iter().for_each(|w| w.reload_bypass_cache()),
            (true, false, false) if k == key::t => self.add_tab_dialog(),
            (true, false, false) if k == key::e => {
                if let Some(tv) = self.current_tab() {
                    self.edit_tab_dialog(&tv);
                }
            }
            (true, false, false) if k == key::comma => settings::open(self),
            (true, false, false) if k == key::w => {
                // Never hide into a tray that isn't there.
                if self.can_hide_to_tray() {
                    self.window.hide();
                }
            }
            (true, false, false) if k == key::q => self.gapp.quit(),
            (false, false, true) if k == key::Left => wv.iter().for_each(|w| w.go_back()),
            (false, false, true) if k == key::Right => wv.iter().for_each(|w| w.go_forward()),
            (false, false, true) if k == key::Home => {
                if let Some(tv) = self.current_tab() {
                    tv.webview.load_uri(&tv.tab.borrow().url);
                }
            }
            (true, _, false) if k == key::plus || k == key::equal || k == key::KP_Add => wv
                .iter()
                .for_each(|w| w.set_zoom_level((w.zoom_level() * 1.1).min(4.0))),
            (true, false, false) if k == key::minus || k == key::KP_Subtract => wv
                .iter()
                .for_each(|w| w.set_zoom_level((w.zoom_level() / 1.1).max(0.3))),
            (true, false, false) if k == key::_0 || k == key::KP_0 => {
                wv.iter().for_each(|w| w.set_zoom_level(1.0))
            }
            (false, false, false) if k == key::F12 => wv
                .iter()
                .filter_map(|w| w.inspector())
                .for_each(|i| i.show()),
            (true, true, false) if k == key::I => wv
                .iter()
                .filter_map(|w| w.inspector())
                .for_each(|i| i.show()),
            _ => return false,
        }
        true
    }

    fn setup_actions(self: &Rc<Self>) {
        // Target of desktop-notification clicks: "app.focus-tab(<tab id>)".
        let action = gio::SimpleAction::new("focus-tab", Some(glib::VariantTy::UINT64));
        let weak = Rc::downgrade(self);
        action.connect_activate(move |_, param| {
            let (Some(app), Some(id)) = (weak.upgrade(), param.and_then(|p| p.get::<u64>())) else {
                return;
            };
            app.show();
            let tab = app.tabs.borrow().iter().find(|t| t.id == id).cloned();
            if let Some(tv) = tab {
                app.select(&tv);
                if let Some(n) = tv.notification.borrow_mut().take() {
                    n.clicked(); // lets the page jump to the chat/email
                }
            }
        });
        self.gapp.add_action(&action);
    }

    fn setup_tray(self: &Rc<Self>, icon_dir: &std::path::Path) {
        let mut tray =
            AppIndicator::with_path("webtabs", "webtabs-tray", &icon_dir.to_string_lossy());
        tray.set_title("webtabs");
        tray.set_attention_icon_full("webtabs-tray-unread", "Unread messages");
        tray.set_status(AppIndicatorStatus::Active);

        let menu = gtk::Menu::new();
        let toggle = gtk::MenuItem::with_label("Show / Hide");
        let config = gtk::MenuItem::with_label("Settings…");
        let quit = gtk::MenuItem::with_label("Quit");
        let weak = Rc::downgrade(self);
        toggle.connect_activate(move |_| with(&weak, |a| a.toggle_visible()));
        let weak = Rc::downgrade(self);
        config.connect_activate(move |_| {
            with(&weak, |a| {
                a.show();
                settings::open(a);
            })
        });
        let weak = Rc::downgrade(self);
        quit.connect_activate(move |_| with(&weak, |a| a.gapp.quit()));
        menu.append(&toggle);
        menu.append(&config);
        menu.append(&gtk::SeparatorMenuItem::new());
        menu.append(&quit);
        menu.show_all();
        let mut menu = menu;
        tray.set_menu(&mut menu);
        *self.tray.borrow_mut() = Some(tray);
    }

    fn update_empty_state(&self) {
        let first = self.tabs.borrow().first().cloned();
        match first {
            None => {
                self.current.set(0);
                self.stack.set_visible_child_name("empty");
            }
            Some(first) if self.current_tab().is_none() => self.select(&first),
            Some(_) => {}
        }
    }

    // ---- options -------------------------------------------------------------

    /// Changes app-wide options from the settings window and persists them.
    fn update_options(&self, f: impl FnOnce(&mut Config)) {
        f(&mut self.config.borrow_mut());
        self.apply_options();
        self.save();
    }

    fn apply_options(&self) {
        let tray_on = self.config.borrow().tray;
        if let Some(row) = self
            .settings_close_row
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
        {
            row.set_sensitive(tray_on);
        }
        self.refresh_unread();
    }

    fn error(&self, title: &str, detail: &str) {
        let d = gtk::MessageDialog::new(
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Error,
            gtk::ButtonsType::Close,
            title,
        );
        d.set_secondary_text(Some(detail));
        d.run();
        d.close();
    }

    fn refresh_settings(&self) {
        let refresh = self.settings_refresh.borrow().clone();
        if let Some(f) = refresh {
            f();
        }
    }

    // ---- tabs ----------------------------------------------------------------

    fn create_tab(self: &Rc<Self>, tab: &Tab) -> Rc<TabView> {
        let cfg = self.config.borrow();
        let settings = web::new_settings(cfg.user_agent_for(tab));
        drop(cfg);
        // The webview needs its tab's home URL (permissions, routing) before the
        // TabView exists; fill the slot right after creating it.
        let slot: Rc<RefCell<Weak<TabView>>> = Rc::default();
        let home: web::UrlFn = {
            let slot = slot.clone();
            Rc::new(move || {
                slot.borrow()
                    .upgrade()
                    .map(|t| t.tab.borrow().url.clone())
                    .unwrap_or_default()
            })
        };
        let webview = web::new_webview(&self.context_for(tab), &settings, home.clone());

        // Icon with the unread bubble pinned to its top-right corner.
        let icon = gtk::Image::from_icon_name(Some("applications-internet"), gtk::IconSize::Dnd);
        icon.set_pixel_size(TAB_ICON_PX);
        icon.set_margin_top(5);
        icon.set_margin_end(7);
        icon.set_margin_start(3);
        let badge_label = gtk::Label::new(None);
        badge_label.style_context().add_class("badge");
        badge_label.set_halign(gtk::Align::End);
        badge_label.set_valign(gtk::Align::Start);
        badge_label.set_no_show_all(true);
        // Optional tag (e.g. "W") bottom-left, to tell same-service tabs apart.
        let tag_label = gtk::Label::new(None);
        tag_label.style_context().add_class("tag");
        tag_label.set_halign(gtk::Align::Start);
        tag_label.set_valign(gtk::Align::End);
        tag_label.set_no_show_all(true);
        let overlay = gtk::Overlay::new();
        overlay.add(&icon);
        overlay.add_overlay(&badge_label);
        overlay.add_overlay(&tag_label);
        let button = gtk::Button::new();
        button.style_context().add_class("tab");
        button.set_relief(gtk::ReliefStyle::None);
        button.set_focus_on_click(false);
        hand_cursor(&button);
        button.add(&overlay);
        button.show_all();

        let tv = Rc::new(TabView {
            id: self.next_id.replace(self.next_id.get() + 1),
            tab: RefCell::new(tab.clone()),
            webview: webview.clone(),
            button: button.clone(),
            badge_label,
            tag_label,
            icon,
            pixbuf: RefCell::default(),
            icon_fetched: Cell::new(false),
            icon_gen: Cell::new(0),
            badge: Cell::new(Badge::None),
            loaded: Cell::new(false),
            notification: RefCell::default(),
        });
        tv.update_tooltip();
        tv.update_tag();
        if let Some(p) = icons::load_cached(&tab.url) {
            // Cached icons are reused; they're refetched only when the URL changes.
            tv.icon_fetched.set(true);
            tv.set_icon(&p);
        }
        // Re-render crisp when the window moves to a screen with another scale.
        let tab_w = Rc::downgrade(&tv);
        tv.icon.connect_scale_factor_notify(move |_| {
            if let Some(tv) = tab_w.upgrade() {
                let p = tv.pixbuf.borrow().clone();
                if let Some(p) = p {
                    tv.set_icon(&p);
                }
            }
        });

        *slot.borrow_mut() = Rc::downgrade(&tv);
        web::setup_routing(&webview, home, self.window.clone().upcast());

        let (app_w, tab_w) = (Rc::downgrade(self), Rc::downgrade(&tv));
        webview.connect_title_notify(move |wv| {
            let (Some(app), Some(tv)) = (app_w.upgrade(), tab_w.upgrade()) else {
                return;
            };
            let badge = Badge::from_title(wv.title().as_deref().unwrap_or(""));
            if tv.badge.replace(badge) != badge {
                tv.badge_label.set_text(&badge.label());
                tv.badge_label.set_visible(badge.is_unread());
                tv.update_tooltip();
                app.refresh_unread();
            }
        });
        let (app_w, tab_w) = (Rc::downgrade(self), Rc::downgrade(&tv));
        webview.connect_load_changed(move |wv, event| {
            if event != LoadEvent::Finished {
                return;
            }
            let (Some(app), Some(tv)) = (app_w.upgrade(), tab_w.upgrade()) else {
                return;
            };
            // Wait until we're on the tab's own site: the first load is often a
            // login redirect (accounts.google.com) whose icon isn't the app's.
            let on_site = wv
                .uri()
                .is_some_and(|u| crate::links::same_site(&u, &tv.tab.borrow().url));
            if on_site && !tv.icon_fetched.replace(true) {
                app.fetch_icon(&tv, wv);
            }
        });
        // Fullscreen video: the default handler fullscreens the window; hide the
        // bar so the page really fills the screen.
        let (app_w, tab_w) = (Rc::downgrade(self), Rc::downgrade(&tv));
        webview.connect_enter_fullscreen(move |_| {
            if let (Some(app), Some(tv)) = (app_w.upgrade(), tab_w.upgrade()) {
                app.fullscreen.set(tv.id);
                app.bar.hide();
            }
            false
        });
        let app_w = Rc::downgrade(self);
        webview.connect_leave_fullscreen(move |_| {
            if let Some(app) = app_w.upgrade() {
                app.fullscreen.set(0);
                app.bar.show();
            }
            false
        });
        let (app_w, tab_w) = (Rc::downgrade(self), Rc::downgrade(&tv));
        webview.connect_show_notification(move |_, n| {
            if let (Some(app), Some(tv)) = (app_w.upgrade(), tab_w.upgrade()) {
                app.notify(&tv, n);
            }
            true
        });

        let (app_w, tab_w) = (Rc::downgrade(self), Rc::downgrade(&tv));
        button.connect_clicked(move |_| {
            if let (Some(app), Some(tv)) = (app_w.upgrade(), tab_w.upgrade()) {
                app.select(&tv);
            }
        });
        let (app_w, tab_w) = (Rc::downgrade(self), Rc::downgrade(&tv));
        button.connect_button_press_event(move |_, ev| {
            let (Some(app), Some(tv)) = (app_w.upgrade(), tab_w.upgrade()) else {
                return glib::Propagation::Proceed;
            };
            match ev.button() {
                3 => {
                    app.tab_menu(&tv).popup_at_pointer(Some(ev));
                    glib::Propagation::Stop
                }
                2 => {
                    tv.webview.reload();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });

        webview.show();
        self.stack.add(&webview);
        self.tab_bar.pack_start(&button, false, false, 0);
        tv
    }

    /// Reads the page's declared icons, downloads the best one and caches it.
    fn fetch_icon(&self, tv: &Rc<TabView>, wv: &WebView) {
        let page_url = wv
            .uri()
            .map(|u| u.to_string())
            .unwrap_or_else(|| tv.tab.borrow().url.clone());
        let home = tv.tab.borrow().url.clone();
        let generation = tv.icon_gen.get();
        let tab_w = Rc::downgrade(tv);
        wv.evaluate_javascript(
            favicon::COLLECT_LINKS_JS,
            None,
            None,
            None::<&gio::Cancellable>,
            move |res| {
                use javascriptcore::ValueExt;
                let json = res.map(|v| v.to_str().to_string()).unwrap_or_default();
                icons::fetch(favicon::candidates(&page_url, &json), move |p| {
                    let (Some(tv), Some(p)) = (tab_w.upgrade(), p) else {
                        return;
                    };
                    if tv.icon_gen.get() != generation {
                        return; // the tab's URL changed while this was downloading
                    }
                    icons::store(&home, &p);
                    tv.set_icon(&p);
                });
            },
        );
    }

    // ---- selection & ordering ------------------------------------------------

    fn select(&self, tv: &Rc<TabView>) {
        if self.fullscreen.get() != tv.id {
            self.exit_fullscreen();
        }
        self.current.set(tv.id);
        self.stack.set_visible_child(&tv.webview);
        for t in self.tabs.borrow().iter() {
            let sc = t.button.style_context();
            if t.id == tv.id {
                sc.add_class("active");
            } else {
                sc.remove_class("active");
            }
        }
        tv.load();
        let wv = tv.webview.clone();
        glib::idle_add_local_once(move || wv.grab_focus());
    }

    fn select_index(&self, i: usize) {
        let tv = self.tabs.borrow().get(i).cloned();
        if let Some(tv) = tv {
            self.select(&tv);
        }
    }

    fn current_index(&self) -> Option<usize> {
        self.tabs
            .borrow()
            .iter()
            .position(|t| t.id == self.current.get())
    }

    /// Makes the bar's button order match `self.tabs`.
    fn sync_bar(&self) {
        for (i, tv) in self.tabs.borrow().iter().enumerate() {
            self.tab_bar.reorder_child(&tv.button, i as i32);
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
        self.sync_bar();
        self.save();
        self.refresh_settings();
    }

    fn set_tab_notifications(&self, tv: &Rc<TabView>, on: bool) {
        tv.tab.borrow_mut().notifications = on;
        self.save();
    }

    /// Show the first tab immediately, then wake the rest up one by one so the
    /// window is usable before every web process has started.
    fn load_tabs_progressively(self: &Rc<Self>) {
        if let Some(tv) = self.current_tab() {
            tv.load();
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let next = app.tabs.borrow().iter().find(|t| !t.loaded.get()).cloned();
            match next {
                Some(tv) => {
                    tv.load();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            }
        });
    }

    fn remove_tab(&self, tv: &Rc<TabView>) {
        let idx = self.tabs.borrow().iter().position(|t| t.id == tv.id);
        self.detach(tv);
        self.tabs.borrow_mut().retain(|t| t.id != tv.id);
        if self.current.get() == tv.id {
            self.current.set(0);
            let n = self.tabs.borrow().len();
            if let (Some(i), true) = (idx, n > 0) {
                self.select_index(i.min(n - 1));
            }
        }
        self.update_empty_state();
        self.refresh_unread();
    }

    /// Leaves page fullscreen (e.g. switching tabs with a shortcut). The page gets
    /// the normal exit; the bar and window are restored here in case it never answers.
    fn exit_fullscreen(&self) {
        let id = self.fullscreen.replace(0);
        if id == 0 {
            return;
        }
        if let Some(tv) = self.tabs.borrow().iter().find(|t| t.id == id) {
            tv.webview.evaluate_javascript(
                "document.fullscreenElement && document.exitFullscreen()",
                None,
                None,
                None::<&gio::Cancellable>,
                |_| {},
            );
        }
        self.window.unfullscreen();
        self.bar.show();
    }

    /// Drops a tab's widgets and web process.
    fn detach(&self, tv: &TabView) {
        if self.fullscreen.get() == tv.id {
            self.exit_fullscreen();
        }
        self.stack.remove(&tv.webview);
        self.tab_bar.remove(&tv.button);
        tv.webview.try_close();
    }

    fn apply_tab(&self, tv: &TabView, new: &Tab) {
        let old = tv.tab.replace(new.clone());
        tv.update_tooltip();
        tv.update_tag();
        if old.url != new.url {
            tv.icon_fetched.set(false);
            tv.icon_gen.set(tv.icon_gen.get() + 1);
        }
        let cfg = self.config.borrow();
        let ua = cfg.user_agent_for(new);
        if let Some(s) = WebViewExt::settings(&tv.webview)
            && s.user_agent().as_deref() != Some(ua)
        {
            s.set_user_agent(Some(ua));
            if tv.loaded.get() {
                tv.webview.reload();
            }
        }
        if old.url != new.url && tv.loaded.get() {
            tv.webview.load_uri(&new.url);
        }
    }

    fn current_tab(&self) -> Option<Rc<TabView>> {
        self.tabs
            .borrow()
            .iter()
            .find(|t| t.id == self.current.get())
            .cloned()
    }

    fn tab_menu(self: &Rc<Self>, tv: &Rc<TabView>) -> gtk::Menu {
        let menu = gtk::Menu::new();
        let item = |label: &str, f: Box<TabAction>| {
            let mi = gtk::MenuItem::with_label(label);
            let (aw, tw) = (Rc::downgrade(self), Rc::downgrade(tv));
            mi.connect_activate(move |_| {
                if let (Some(a), Some(t)) = (aw.upgrade(), tw.upgrade()) {
                    f(&a, &t);
                }
            });
            menu.append(&mi);
        };
        item("Reload", Box::new(|_, t| t.webview.reload()));
        item(
            "Go to home page",
            Box::new(|_, t| t.webview.load_uri(&t.tab.borrow().url)),
        );
        item("Move left", Box::new(|a, t| a.move_tab(t, -1)));
        item("Move right", Box::new(|a, t| a.move_tab(t, 1)));
        item("Edit…", Box::new(|a, t| a.edit_tab_dialog(t)));
        item("Remove", Box::new(|a, t| a.confirm_remove(t)));
        menu.append(&gtk::SeparatorMenuItem::new());
        item("Add tab…", Box::new(|a, _| a.add_tab_dialog()));
        item("Settings…", Box::new(|a, _| settings::open(a)));
        menu.set_attach_widget(Some(&self.window));
        // Built per right-click: destroy it once closed (after the item's
        // activate handler has run) or every click leaks a menu.
        menu.connect_deactivate(|m| {
            let m = m.clone();
            glib::idle_add_local_once(move || unsafe { m.destroy() });
        });
        menu.show_all();
        menu
    }

    fn all_tabs(&self) -> Vec<Tab> {
        self.tabs
            .borrow()
            .iter()
            .map(|t| t.tab.borrow().clone())
            .collect()
    }

    fn add_tab_dialog(self: &Rc<Self>) {
        let existing = self.all_tabs();
        let cx = tab_dialog::Context {
            parent: self.window.upcast_ref(),
            editing: None,
            existing: &existing,
        };
        if let Some(tab) = tab_dialog::run(cx) {
            let tv = self.create_tab(&tab);
            self.tabs.borrow_mut().push(tv.clone());
            self.select(&tv);
            self.update_empty_state();
            self.save();
            self.refresh_settings();
        }
    }

    fn edit_tab_dialog(self: &Rc<Self>, tv: &Rc<TabView>) {
        let current = tv.tab.borrow().clone();
        let existing = self.all_tabs();
        let cx = tab_dialog::Context {
            parent: self.window.upcast_ref(),
            editing: Some(&current),
            existing: &existing,
        };
        if let Some(tab) = tab_dialog::run(cx) {
            // The dialog runs a nested main loop: a tabs.toml reload may have
            // removed this tab meanwhile. Then there's nothing left to edit.
            if !self.tabs.borrow().iter().any(|t| t.id == tv.id) {
                return;
            }
            if tab.session != current.session {
                // A webview can't change profiles; give the tab a fresh one.
                self.replace_tab(tv, &tab);
                if let Some(key) = &current.session {
                    self.drop_session_if_unused(key);
                }
            } else {
                self.apply_tab(tv, &tab);
            }
            self.save();
            self.refresh_settings();
        }
    }

    /// Swaps a tab for a freshly built one in the same position.
    fn replace_tab(self: &Rc<Self>, old: &Rc<TabView>, tab: &Tab) {
        let was_current = self.current.get() == old.id;
        let fresh = self.create_tab(tab);
        let placed = {
            let mut tabs = self.tabs.borrow_mut();
            match tabs.iter().position(|t| t.id == old.id) {
                Some(i) => {
                    tabs[i] = fresh.clone();
                    true
                }
                None => false,
            }
        };
        if !placed {
            self.detach(&fresh); // old tab is gone; don't leave an orphan behind
            return;
        }
        self.detach(old);
        self.sync_bar();
        if was_current {
            self.select(&fresh);
        } else {
            fresh.load();
        }
        self.refresh_unread();
    }

    fn confirm_remove(self: &Rc<Self>, tv: &Rc<TabView>) {
        let (name, session) = {
            let t = tv.tab.borrow();
            (t.name.clone(), t.session.clone())
        };
        let dialog = gtk::MessageDialog::new(
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Question,
            gtk::ButtonsType::None,
            &format!("Remove \"{name}\"?"),
        );
        if session.is_some() {
            dialog.set_secondary_text(Some("Its isolated login data is deleted too."));
        }
        dialog.add_buttons(&[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Remove", gtk::ResponseType::Accept),
        ]);
        let ok = dialog.run() == gtk::ResponseType::Accept;
        dialog.close();
        if ok {
            self.remove_tab(tv);
            if let Some(key) = &session {
                self.drop_session_if_unused(key);
            }
            self.save();
            self.refresh_settings();
        }
    }

    // ---- unread & notifications ---------------------------------------------

    fn refresh_unread(&self) {
        let tabs = self.tabs.borrow();
        let total: u32 = tabs.iter().map(|t| t.badge.get().count()).sum();
        let any = tabs.iter().any(|t| t.badge.get().is_unread());
        drop(tabs);
        let cfg = self.config.borrow();
        let (in_title, tray_on, in_launcher) = (cfg.unread_in_title, cfg.tray, cfg.unread_in_dock);
        drop(cfg);
        self.update_launcher(total, in_launcher && total > 0);
        self.window.set_title(&if total > 0 && in_title {
            format!("webtabs ({total})")
        } else {
            "webtabs".into()
        });
        if let Some(tray) = self.tray.borrow_mut().as_mut() {
            tray.set_status(if !tray_on {
                AppIndicatorStatus::Passive
            } else if any {
                AppIndicatorStatus::Attention
            } else {
                AppIndicatorStatus::Active
            });
        }
        if total > self.last_unread.get() && !self.window.is_active() {
            self.window.set_urgency_hint(true);
        }
        self.last_unread.set(total);
    }

    /// Unread count on the dock/launcher icon via the `com.canonical.Unity.LauncherEntry`
    /// signal (KDE, Dash-to-Dock, Plank; panels without support ignore it).
    fn update_launcher(&self, count: u32, visible: bool) {
        let state = (if visible { count } else { 0 }, visible);
        if self.last_launcher.replace(Some(state)) == Some(state) {
            return;
        }
        let Some(bus) = self.gapp.dbus_connection() else {
            return;
        };
        let props = glib::VariantDict::new(None);
        props.insert("count", i64::from(state.0));
        props.insert("count-visible", visible);
        // (sa{sv}): build the tuple from the parts so the dict isn't boxed as `v`.
        let args = glib::Variant::tuple_from_iter([LAUNCHER_APP_URI.to_variant(), props.end()]);
        if let Err(e) = bus.emit_signal(
            None,
            "/dev/vaan/webtabs",
            "com.canonical.Unity.LauncherEntry",
            "Update",
            Some(&args),
        ) {
            eprintln!("webtabs: launcher badge: {e}");
        }
    }

    fn notify(&self, tv: &Rc<TabView>, n: &webkit2gtk::Notification) {
        if !self.config.borrow().notifications || !tv.tab.borrow().notifications {
            return;
        }
        let is_current = self.current_tab().is_some_and(|c| c.id == tv.id);
        if self.window.is_active() && is_current {
            return; // you're looking at it
        }
        let app_name = tv.tab.borrow().name.clone();
        let title = n
            .title()
            .map(|t| t.to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or(app_name.clone());
        let gn = gio::Notification::new(&title);
        let body = n.body().map(|b| b.to_string()).unwrap_or_default();
        gn.set_body(Some(&if body.is_empty() {
            app_name
        } else {
            format!("{body}\n— {app_name}")
        }));
        gn.set_default_action_and_target_value("app.focus-tab", Some(&tv.id.to_variant()));
        let id = format!("tab-{}", tv.id);
        self.gapp.send_notification(Some(&id), &gn);
        // All of a tab's notifications share one desktop id, so only withdraw
        // when the page closes the one currently shown (not an older one).
        let (gapp, tab_w) = (self.gapp.clone(), Rc::downgrade(tv));
        n.connect_closed(move |closed| {
            let Some(tv) = tab_w.upgrade() else { return };
            let latest = tv.notification.borrow().as_ref() == Some(closed);
            if latest {
                tv.notification.borrow_mut().take();
                gapp.withdraw_notification(&id);
            }
        });
        *tv.notification.borrow_mut() = Some(n.clone());
    }

    /// The browsing profile a tab runs in, creating isolated ones on demand.
    fn context_for(self: &Rc<Self>, tab: &Tab) -> WebContext {
        let Some(key) = tab.session.as_deref() else {
            return self.context.clone();
        };
        // The key is back (re-added, or a config edit in progress): keep its data.
        if let Some(pending) = self.pending_deletes.borrow_mut().remove(key) {
            pending.remove();
        }
        if let Some(ctx) = self.isolated.borrow().get(key) {
            return ctx.clone();
        }
        let ctx = web::new_context(Some(key));
        self.setup_context(&ctx);
        self.isolated
            .borrow_mut()
            .insert(key.to_string(), ctx.clone());
        ctx
    }

    /// Deletes an isolated profile's data once no tab uses it anymore.
    fn session_in_use(&self, key: &str) -> bool {
        self.tabs
            .borrow()
            .iter()
            .any(|t| t.tab.borrow().session.as_deref() == Some(key))
    }

    /// Deletes an isolated profile's data once no tab uses it anymore. Cookies and
    /// storage are cleared right away through WebKit; the folder goes a moment
    /// later. A stale folder (app quit first) is purged at the next start.
    fn drop_session_if_unused(self: &Rc<Self>, key: &str) {
        if self.session_in_use(key) {
            return;
        }
        if let Some(ctx) = self.isolated.borrow_mut().remove(key)
            && let Some(manager) = ctx.website_data_manager()
        {
            manager.clear(
                webkit2gtk::WebsiteDataTypes::ALL,
                glib::TimeSpan::from_seconds(0),
                None::<&gio::Cancellable>,
                |r| {
                    if let Err(e) = r {
                        eprintln!("webtabs: clearing session data failed: {e}");
                    }
                },
            );
        }
        let weak = Rc::downgrade(self);
        let key_owned = key.to_string();
        // Give the web/network processes a moment to let go of their files.
        let source = glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
            let Some(app) = weak.upgrade() else { return };
            app.pending_deletes.borrow_mut().remove(&key_owned);
            if !app.session_in_use(&key_owned) {
                delete_session_dir(&key_owned);
            }
        });
        if let Some(old) = self
            .pending_deletes
            .borrow_mut()
            .insert(key.to_string(), source)
        {
            old.remove();
        }
    }

    /// Removes session folders no tab refers to (e.g. the app quit right after
    /// an isolated tab was removed).
    fn purge_stale_sessions(&self) {
        let root = config::data_dir().join("sessions");
        let Ok(entries) = std::fs::read_dir(&root) else {
            return;
        };
        for entry in entries.flatten() {
            let key = entry.file_name().to_string_lossy().into_owned();
            if !self.session_in_use(&key) {
                delete_session_dir(&key);
            }
        }
    }

    fn setup_context(self: &Rc<Self>, context: &WebContext) {
        // Pre-grant notifications so `Notification.permission === "granted"` from
        // the first load (WhatsApp/Slack otherwise show a nag banner).
        let weak = Rc::downgrade(self);
        context.connect_initialize_notification_permissions(move |ctx| {
            let Some(app) = weak.upgrade() else { return };
            let origins: Vec<_> = app
                .config
                .borrow()
                .tabs
                .iter()
                .map(|t| webkit2gtk::SecurityOrigin::for_uri(&t.url))
                .collect();
            ctx.initialize_notification_permissions(&origins.iter().collect::<Vec<_>>(), &[]);
        });
        let weak = Rc::downgrade(self);
        context.connect_download_started(move |_, download| {
            let weak = weak.clone();
            download.connect_finished(move |d| {
                let Some(app) = weak.upgrade() else { return };
                let Some(dest) = d.destination() else { return };
                let path = glib::filename_from_uri(&dest)
                    .map(|(p, _)| p)
                    .unwrap_or_else(|_| dest.as_str().into());
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let gn = gio::Notification::new("Download complete");
                gn.set_body(Some(&name));
                app.gapp.send_notification(None, &gn);
            });
        });
    }

    // ---- config file sync ------------------------------------------------------

    fn config_uri(&self) -> String {
        gio::File::for_path(&self.config_path).uri().to_string()
    }

    fn save(&self) {
        let mut cfg = self.config.borrow().clone();
        cfg.tabs = self
            .tabs
            .borrow()
            .iter()
            .map(|t| t.tab.borrow().clone())
            .collect();
        *self.last_saved.borrow_mut() = cfg.to_toml();
        if let Err(e) = cfg.save(&self.config_path) {
            eprintln!("webtabs: cannot save {}: {e}", self.config_path.display());
        }
        *self.config.borrow_mut() = cfg;
    }

    fn watch_config(self: &Rc<Self>) {
        let file = gio::File::for_path(&self.config_path);
        let monitor =
            match file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>) {
                Ok(m) => m,
                Err(e) => return eprintln!("webtabs: not watching config: {e}"),
            };
        let weak = Rc::downgrade(self);
        monitor.connect_changed(move |_, _, _, event| {
            if matches!(
                event,
                gio::FileMonitorEvent::Deleted | gio::FileMonitorEvent::PreUnmount
            ) {
                return;
            }
            let Some(app) = weak.upgrade() else { return };
            // Editors fire several events per save; coalesce them.
            if app.reload_pending.replace(true) {
                return;
            }
            let weak = weak.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                with(&weak, |a| {
                    a.reload_pending.set(false);
                    a.reload_config();
                })
            });
        });
        *self.monitor.borrow_mut() = Some(monitor);
    }

    fn reload_config(self: &Rc<Self>) {
        let Ok(text) = std::fs::read_to_string(&self.config_path) else {
            return;
        };
        if text == *self.last_saved.borrow() {
            return; // our own write
        }
        let new = match Config::parse(&text) {
            Ok(c) => c,
            Err(e) => {
                let gn = gio::Notification::new("webtabs: tabs.toml not applied");
                gn.set_body(Some(&e));
                self.gapp.send_notification(Some("config-error"), &gn);
                return;
            }
        };
        self.gapp.withdraw_notification("config-error");
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
        for tv in &old {
            self.detach(tv);
            if self.current.get() == tv.id {
                self.current.set(0);
            }
        }
        *self.tabs.borrow_mut() = next;
        for key in old.iter().filter_map(|t| t.tab.borrow().session.clone()) {
            self.drop_session_if_unused(&key);
        }
        self.sync_bar();
        self.update_empty_state();
        self.apply_options();
        self.load_tabs_progressively();
        self.refresh_settings();
    }
}

fn delete_session_dir(key: &str) {
    let (data, _) = web::session_dirs(Some(key));
    let Some(root) = data.parent() else { return };
    if let Err(e) = std::fs::remove_dir_all(root)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("webtabs: cannot delete {}: {e}", root.display());
    }
}

fn with(weak: &Weak<App>, f: impl FnOnce(&Rc<App>)) {
    if let Some(app) = weak.upgrade() {
        f(&app);
    }
}

// ---- helpers ----------------------------------------------------------------------

/// Pointing-hand cursor while hovering `w`, like a link. GTK3 buttons have no
/// window of their own, so swap the toplevel's cursor on enter/leave.
fn hand_cursor(w: &impl IsA<gtk::Widget>) {
    w.add_events(gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
    w.connect_enter_notify_event(|w, _| {
        if let Some(win) = w.window() {
            let cursor = gdk::Cursor::from_name(&win.display(), "pointer");
            win.set_cursor(cursor.as_ref());
        }
        glib::Propagation::Proceed
    });
    w.connect_leave_notify_event(|w, _| {
        if let Some(win) = w.window() {
            win.set_cursor(None);
        }
        glib::Propagation::Proceed
    });
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    // A styling mistake must never keep the app from starting.
    if let Err(e) = provider.load_from_data(CSS.as_bytes()) {
        eprintln!("webtabs: css: {e}");
    }
    if let Some(screen) = gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Icons are embedded in the binary and written out for the tray/window,
/// so the app works the same installed or straight from `cargo run`.
fn write_icons() -> PathBuf {
    let dir = config::data_dir().join("icons");
    let _ = std::fs::create_dir_all(&dir);
    for (name, svg) in [
        ("webtabs.svg", ICON),
        ("webtabs-unread.svg", ICON_UNREAD),
        ("webtabs-tray.svg", TRAY_ICON),
        ("webtabs-tray-unread.svg", TRAY_ICON_UNREAD),
    ] {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(svg) {
            let _ = std::fs::write(&path, svg);
        }
    }
    dir
}

fn empty_state() -> (gtk::Widget, gtk::Button) {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.set_valign(gtk::Align::Center);
    b.set_halign(gtk::Align::Center);
    let title = gtk::Label::new(Some("No tabs yet"));
    title.style_context().add_class("empty-title");
    let hint = gtk::Label::new(Some("Press Ctrl+T, or open settings with the gear"));
    hint.style_context().add_class("dim-label");
    let btn = gtk::Button::with_label("Add a tab");
    btn.set_halign(gtk::Align::Center);
    b.pack_start(&title, false, false, 0);
    b.pack_start(&hint, false, false, 0);
    b.pack_start(&btn, false, false, 0);
    b.show_all();
    (b.upcast(), btn)
}
