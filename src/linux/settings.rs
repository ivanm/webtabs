//! Settings window (gear button): app-wide switches on top, tab manager below.
//! Every change applies immediately and is written to tabs.toml.

use super::{App, TabView, icons, web};
use crate::autostart;
use crate::config::Config;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;

pub fn open(app: &Rc<App>) {
    // One settings window at a time (gear, Ctrl+, and the tray can all open it).
    let open_one = app
        .settings_window
        .borrow()
        .as_ref()
        .and_then(|w| w.upgrade());
    if let Some(dialog) = open_one {
        dialog.present();
        return;
    }
    let dialog = gtk::Dialog::with_buttons(
        Some("webtabs settings"),
        Some(&app.window),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[("Close", gtk::ResponseType::Close)],
    );
    dialog.set_default_size(560, 760);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.set_border_width(18);

    page.pack_start(&heading("General"), false, false, 0);
    let general = gtk::ListBox::new();
    general.set_selection_mode(gtk::SelectionMode::None);
    general.style_context().add_class("frame");

    let entry = autostart::entry_path();
    switch_row(
        &general,
        "Start with computer",
        "Opens hidden in the tray when you log in",
        autostart::is_enabled(&entry),
        {
            let app = Rc::downgrade(app);
            move |on| {
                let exe = std::env::current_exe().unwrap_or_else(|_| "webtabs".into());
                match autostart::set(&entry, on, &exe) {
                    Ok(()) => true,
                    Err(e) => {
                        if let Some(app) = app.upgrade() {
                            app.error("Could not change autostart", &e.to_string());
                        }
                        false
                    }
                }
            }
        },
    );
    let opt = |general: &gtk::ListBox,
               title: &str,
               sub: &str,
               get: fn(&Config) -> bool,
               set: fn(&mut Config, bool)| {
        let current = get(&app.config.borrow());
        let app = Rc::downgrade(app);
        switch_row(general, title, sub, current, move |on| {
            if let Some(app) = app.upgrade() {
                app.update_options(|c| set(c, on));
            }
            true
        })
    };
    opt(
        &general,
        "Show notifications",
        "Desktop notifications from your tabs",
        |c| c.notifications,
        |c, v| c.notifications = v,
    );
    opt(
        &general,
        "Show icon in tray",
        "Unread indicator and quick menu in the panel",
        |c| c.tray,
        |c, v| c.tray = v,
    );
    let close = opt(
        &general,
        "Close to tray",
        "Closing the window keeps webtabs running",
        |c| c.close_to_tray,
        |c, v| c.close_to_tray = v,
    );
    opt(
        &general,
        "Unread count in window title",
        "e.g. \u{201c}webtabs (5)\u{201d}",
        |c| c.unread_in_title,
        |c, v| c.unread_in_title = v,
    );
    // Closing to a tray that isn't there would make the app unreachable.
    close.set_sensitive(app.config.borrow().tray);
    app.settings_close_row.replace(Some(close.downgrade()));
    page.pack_start(&general, false, false, 0);

    let tabs_heading = heading("Tabs");
    tabs_heading.set_margin_top(14);
    page.pack_start(&tabs_heading, false, false, 0);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.style_context().add_class("frame");
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_min_content_height(260);
    scroll.add(&list);
    page.pack_start(&scroll, true, true, 0);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let add = gtk::Button::with_label("Add tab");
    add.set_image(Some(&gtk::Image::from_icon_name(
        Some("list-add-symbolic"),
        gtk::IconSize::Button,
    )));
    add.set_always_show_image(true);
    let toml = gtk::Button::with_label("Open tabs.toml");
    actions.pack_start(&add, false, false, 0);
    actions.pack_end(&toml, false, false, 0);
    page.pack_start(&actions, false, false, 4);

    dialog.content_area().pack_start(&page, true, true, 0);

    // The list is rebuilt from App state after every change; App calls this back
    // too when tabs.toml is edited by hand while the window is open.
    let refresh: Rc<dyn Fn()> = {
        let (app, list) = (Rc::downgrade(app), list.clone());
        Rc::new(move || {
            if let Some(app) = app.upgrade() {
                fill(&app, &list);
            }
        })
    };
    app.settings_refresh.replace(Some(refresh.clone()));
    refresh();

    let weak = Rc::downgrade(app);
    add.connect_clicked(move |_| {
        if let Some(app) = weak.upgrade() {
            app.add_tab_dialog();
        }
    });
    let weak = Rc::downgrade(app);
    toml.connect_clicked(move |_| {
        if let Some(app) = weak.upgrade() {
            web::open_external(&app.config_uri());
        }
    });

    app.settings_window.replace(Some(dialog.downgrade()));
    dialog.show_all();
    dialog.run();
    app.settings_window.replace(None);
    app.settings_refresh.replace(None);
    app.settings_close_row.replace(None);
    dialog.close();
}

fn fill(app: &Rc<App>, list: &gtk::ListBox) {
    list.foreach(|c| list.remove(c));
    let tabs = app.tabs.borrow().clone();
    if tabs.is_empty() {
        let empty = gtk::Label::new(Some("No tabs yet"));
        empty.style_context().add_class("dim-label");
        empty.set_margin_top(18);
        empty.set_margin_bottom(18);
        list.add(&empty);
    }
    for (i, tv) in tabs.iter().enumerate() {
        list.add(&tab_row(app, tv, i, tabs.len()));
    }
    list.show_all();
}

fn tab_row(app: &Rc<App>, tv: &Rc<TabView>, index: usize, count: usize) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_border_width(8);

    let icon =
        gtk::Image::from_icon_name(Some("applications-internet"), gtk::IconSize::LargeToolbar);
    icon.set_pixel_size(24);
    if let Some(p) = tv.pixbuf.borrow().as_ref() {
        let (icon, p) = (icon.clone(), p.clone());
        // Needs to be realized to know the HiDPI scale.
        icon.connect_realize(move |i| icons::show(i, &p, 24));
    }
    row.pack_start(&icon, false, false, 0);

    let tab = tv.tab.borrow().clone();
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let name = gtk::Label::new(Some(&tab.name));
    name.set_halign(gtk::Align::Start);
    let url = gtk::Label::new(Some(&tab.url));
    url.set_halign(gtk::Align::Start);
    url.set_ellipsize(gtk::pango::EllipsizeMode::End);
    url.style_context().add_class("dim-label");
    url.style_context().add_class("caption");
    text.pack_start(&name, false, false, 0);
    text.pack_start(&url, false, false, 0);
    row.pack_start(&text, true, true, 0);

    let bell = gtk::ToggleButton::new();
    bell.set_relief(gtk::ReliefStyle::None);
    bell.set_active(tab.notifications);
    let set_bell = |b: &gtk::ToggleButton| {
        let on = b.is_active();
        b.set_image(Some(&gtk::Image::from_icon_name(
            Some(if on {
                "preferences-system-notifications-symbolic"
            } else {
                "notifications-disabled-symbolic"
            }),
            gtk::IconSize::Button,
        )));
        b.set_tooltip_text(Some(if on {
            "Notifications on (click to mute)"
        } else {
            "Muted (click to unmute)"
        }));
    };
    set_bell(&bell);
    let (aw, tw) = (Rc::downgrade(app), Rc::downgrade(tv));
    bell.connect_toggled(move |b| {
        set_bell(b);
        if let (Some(a), Some(t)) = (aw.upgrade(), tw.upgrade()) {
            a.set_tab_notifications(&t, b.is_active());
        }
    });
    row.pack_start(&bell, false, false, 0);

    let button = |icon: &str, tip: &str, sensitive: bool, f: fn(&Rc<App>, &Rc<TabView>)| {
        let b = gtk::Button::from_icon_name(Some(icon), gtk::IconSize::Button);
        b.set_relief(gtk::ReliefStyle::None);
        b.set_tooltip_text(Some(tip));
        b.set_sensitive(sensitive);
        let (aw, tw) = (Rc::downgrade(app), Rc::downgrade(tv));
        b.connect_clicked(move |_| {
            if let (Some(a), Some(t)) = (aw.upgrade(), tw.upgrade()) {
                // Rebuilding the list destroys this button; do it after the click returns.
                glib::idle_add_local_once(move || f(&a, &t));
            }
        });
        row.pack_start(&b, false, false, 0);
    };
    button(
        "go-up-symbolic",
        "Move up (left in the tab bar)",
        index > 0,
        |a, t| a.move_tab(t, -1),
    );
    button(
        "go-down-symbolic",
        "Move down (right in the tab bar)",
        index + 1 < count,
        |a, t| a.move_tab(t, 1),
    );
    button("document-edit-symbolic", "Edit", true, |a, t| {
        a.edit_tab_dialog(t)
    });
    button("user-trash-symbolic", "Remove", true, |a, t| {
        a.confirm_remove(t)
    });
    row.upcast()
}

fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(text)));
    l.set_halign(gtk::Align::Start);
    l.set_margin_bottom(4);
    l
}

/// A title/subtitle row with a switch. `apply` returns whether the change stuck;
/// if not, the switch flips back.
fn switch_row(
    list: &gtk::ListBox,
    title: &str,
    subtitle: &str,
    active: bool,
    apply: impl Fn(bool) -> bool + 'static,
) -> gtk::ListBoxRow {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_border_width(10);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let t = gtk::Label::new(Some(title));
    t.set_halign(gtk::Align::Start);
    let s = gtk::Label::new(Some(subtitle));
    s.set_halign(gtk::Align::Start);
    s.style_context().add_class("dim-label");
    s.style_context().add_class("caption");
    text.pack_start(&t, false, false, 0);
    text.pack_start(&s, false, false, 0);
    row.pack_start(&text, true, true, 0);
    let sw = gtk::Switch::new();
    sw.set_active(active);
    sw.set_valign(gtk::Align::Center);
    sw.connect_state_set(move |sw, on| {
        if !apply(on) {
            let sw = sw.clone();
            glib::idle_add_local_once(move || sw.set_active(!on));
        }
        glib::Propagation::Proceed
    });
    row.pack_start(&sw, false, false, 0);
    let lbr = gtk::ListBoxRow::new();
    lbr.set_activatable(false);
    lbr.add(&row);
    list.add(&lbr);
    lbr
}
