//! Add/edit tab window: one-click presets on top, the form below, rarely needed
//! knobs (user agent) folded into "Advanced".

use super::icons;
use crate::config::{self, DEFAULT_USER_AGENT, Tab, USER_AGENT_PRESETS};
use crate::links;
use crate::presets::PRESETS;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;

const UA_DEFAULT_ID: &str = "default";
const UA_CUSTOM_ID: &str = "custom";

pub struct Context<'a> {
    pub parent: &'a gtk::Window,
    /// The tab being edited; `None` adds a new one (and shows the presets).
    pub editing: Option<&'a Tab>,
    /// All current tabs, to hint at isolation and keep session keys unique.
    pub existing: &'a [Tab],
}

/// Returns the new/edited tab, or `None` on cancel.
pub fn run(cx: Context) -> Option<Tab> {
    let adding = cx.editing.is_none();
    let dialog = gtk::Dialog::with_buttons(
        Some(if adding { "Add tab" } else { "Edit tab" }),
        Some(cx.parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            (
                if adding { "Add tab" } else { "Save" },
                gtk::ResponseType::Accept,
            ),
        ],
    );
    dialog.set_default_response(gtk::ResponseType::Accept);
    dialog.set_default_width(500);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 14);
    page.set_border_width(18);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(12);
    let row_label = |row: i32, text: &str| {
        let l = gtk::Label::new(Some(text));
        l.set_halign(gtk::Align::End);
        grid.attach(&l, 0, row, 1, 1);
    };
    let entry = |row: i32, label: &str, value: Option<&str>, placeholder: &str| {
        row_label(row, label);
        let e = gtk::Entry::new();
        e.set_hexpand(true);
        e.set_activates_default(true);
        e.set_placeholder_text(Some(placeholder));
        e.set_text(value.unwrap_or(""));
        grid.attach(&e, 1, row, 1, 1);
        e
    };
    let tab = cx.editing;
    let url = entry(
        0,
        "URL",
        tab.map(|t| t.url.as_str()),
        "https://… or pick a service above",
    );
    let name = entry(
        1,
        "Name",
        tab.map(|t| t.name.as_str()),
        "shown when you hover the icon",
    );
    let label = entry(
        2,
        "Icon tag",
        tab.and_then(|t| t.label.as_deref()),
        "e.g. W",
    );
    label.set_max_length(2);
    label.set_width_chars(7);
    label.set_tooltip_text(Some(
        "Optional 1-2 letters drawn on the icon, to tell similar tabs apart",
    ));
    label.set_hexpand(false);
    label.set_halign(gtk::Align::Start);

    let isolated =
        gtk::CheckButton::with_label("Isolated login (own cookies, for a second account)");
    isolated.set_active(tab.is_some_and(|t| t.session.is_some()));
    grid.attach(&isolated, 1, 3, 1, 1);
    let iso_hint = gtk::Label::new(None);
    iso_hint.set_halign(gtk::Align::Start);
    iso_hint.set_line_wrap(true);
    iso_hint.set_xalign(0.0);
    iso_hint.style_context().add_class("dim-label");
    iso_hint.style_context().add_class("caption");
    iso_hint.set_no_show_all(true);
    grid.attach(&iso_hint, 1, 4, 1, 1);
    let mute = gtk::CheckButton::with_label("Mute notifications");
    mute.set_active(tab.is_some_and(|t| !t.notifications));
    grid.attach(&mute, 1, 5, 1, 1);

    // Advanced: user agent.
    let ua_box = gtk::Grid::new();
    ua_box.set_row_spacing(8);
    ua_box.set_column_spacing(12);
    ua_box.set_margin_top(8);
    let ua_label = gtk::Label::new(Some("User agent"));
    ua_label.set_halign(gtk::Align::End);
    let ua_combo = gtk::ComboBoxText::new();
    ua_combo.append(Some(UA_DEFAULT_ID), "Default (Safari)");
    for (name, _) in USER_AGENT_PRESETS {
        ua_combo.append(Some(name), name);
    }
    ua_combo.append(Some(UA_CUSTOM_ID), "Custom…");
    let ua_custom = gtk::Entry::new();
    ua_custom.set_hexpand(true);
    ua_custom.set_placeholder_text(Some(DEFAULT_USER_AGENT));
    ua_custom.set_no_show_all(true);
    let current_ua = tab.and_then(|t| t.user_agent.clone());
    let ua_id = match current_ua.as_deref() {
        None => UA_DEFAULT_ID,
        Some(ua) => USER_AGENT_PRESETS
            .iter()
            .find(|(_, v)| *v == ua)
            .map(|(n, _)| *n)
            .unwrap_or(UA_CUSTOM_ID),
    };
    ua_combo.set_active_id(Some(ua_id));
    if ua_id == UA_CUSTOM_ID {
        ua_custom.set_text(current_ua.as_deref().unwrap_or(""));
    }
    let ua_hint = gtk::Label::new(Some(
        "Change only if a site says your browser isn't supported. The tab reloads.",
    ));
    ua_hint.set_halign(gtk::Align::Start);
    ua_hint.set_xalign(0.0);
    ua_hint.set_line_wrap(true);
    ua_hint.style_context().add_class("dim-label");
    ua_hint.style_context().add_class("caption");
    ua_box.attach(&ua_label, 0, 0, 1, 1);
    ua_box.attach(&ua_combo, 1, 0, 1, 1);
    ua_box.attach(&ua_custom, 1, 1, 1, 1);
    ua_box.attach(&ua_hint, 1, 2, 1, 1);
    let sync_custom = {
        let ua_custom = ua_custom.clone();
        move |c: &gtk::ComboBoxText| {
            ua_custom.set_visible(c.active_id().as_deref() == Some(UA_CUSTOM_ID))
        }
    };
    sync_custom(&ua_combo);
    ua_combo.connect_changed(sync_custom);
    let advanced = gtk::Expander::new(Some("Advanced"));
    advanced.add(&ua_box);
    advanced.set_expanded(ua_id != UA_DEFAULT_ID);

    // Warn when a second tab on the same site would share the first one's login.
    let existing: Rc<Vec<Tab>> = Rc::new(cx.existing.to_vec());
    let editing_url = tab.map(|t| t.url.clone());
    let update_hint = {
        let (iso_hint, isolated, existing) = (iso_hint.clone(), isolated.clone(), existing.clone());
        move |url_text: &str| {
            let url = config::normalize_url(url_text);
            let site_of = |u: &str| {
                url::Url::parse(u)
                    .ok()
                    .and_then(|u| u.host_str().map(links::site))
            };
            let site = site_of(&url);
            let twin = existing.iter().find(|t| {
                Some(&t.url) != editing_url.as_ref()
                    && t.session.is_none()
                    && site.is_some()
                    && site_of(&t.url) == site
            });
            match twin {
                Some(t) if !isolated.is_active() => {
                    iso_hint.set_text(&format!(
                        "Shares its login with your \u{201c}{}\u{201d} tab. Tick Isolated login to use a different account.",
                        t.name
                    ));
                    iso_hint.show();
                }
                _ => iso_hint.hide(),
            }
        }
    };
    let hint = Rc::new(update_hint);
    {
        let hint = hint.clone();
        url.connect_changed(move |e| hint(&e.text()));
    }
    {
        let (hint, url) = (hint.clone(), url.clone());
        isolated.connect_toggled(move |_| hint(&url.text()));
    }
    hint(&url.text());

    if adding {
        page.pack_start(&presets_row(&url, &name), false, false, 0);
    }
    page.pack_start(&grid, false, false, 0);
    page.pack_start(&advanced, false, false, 0);
    dialog.content_area().pack_start(&page, true, true, 0);
    dialog.show_all();
    if adding {
        url.grab_focus();
    }

    let result = loop {
        if dialog.run() != gtk::ResponseType::Accept {
            break None;
        }
        let user_agent = match ua_combo.active_id().as_deref() {
            Some(UA_CUSTOM_ID) => Some(ua_custom.text().to_string()),
            Some(UA_DEFAULT_ID) | None => None,
            Some(preset) => USER_AGENT_PRESETS
                .iter()
                .find(|(n, _)| *n == preset)
                .map(|(_, v)| v.to_string()),
        };
        let mut t =
            Tab::new(&name.text(), &url.text(), user_agent.as_deref()).with_label(&label.text());
        t.notifications = !mute.is_active();
        t.session = if isolated.is_active() {
            // Keep the existing storage when editing an already isolated tab.
            tab.and_then(|t| t.session.clone()).or_else(|| {
                let taken: Vec<&str> = existing
                    .iter()
                    .filter_map(|t| t.session.as_deref())
                    .collect();
                Some(config::new_session_key(&t.name, &taken))
            })
        } else {
            None
        };
        if t.is_valid() {
            break Some(t);
        }
        url.style_context().add_class("error");
        url.grab_focus();
    };
    dialog.close();
    result
}

/// The row of service buttons; clicking one fills in URL and name.
fn presets_row(url: &gtk::Entry, name: &gtk::Entry) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_homogeneous(true);
    for preset in PRESETS {
        let icon = gtk::Image::from_icon_name(Some("applications-internet"), gtk::IconSize::Dialog);
        icon.set_pixel_size(40);
        // Icons come from the favicon cache, or are fetched on first use.
        match icons::load_cached(preset.url) {
            Some(p) => {
                icon.connect_realize(move |i| icons::show(i, &p, 40));
            }
            None => {
                let weak = icon.downgrade();
                let page = preset.url;
                icons::fetch(crate::favicon::candidates(page, "[]"), move |p| {
                    let (Some(icon), Some(p)) = (weak.upgrade(), p) else {
                        return;
                    };
                    icons::store(page, &p);
                    icons::show(&icon, &p, 40);
                });
            }
        }
        let text = gtk::Label::new(Some(preset.name));
        let col = gtk::Box::new(gtk::Orientation::Vertical, 6);
        col.pack_start(&icon, false, false, 0);
        col.pack_start(&text, false, false, 0);
        let b = gtk::Button::new();
        b.add(&col);
        b.set_relief(gtk::ReliefStyle::None);
        b.style_context().add_class("preset");
        b.set_tooltip_text(Some(preset.url));
        super::hand_cursor(&b);
        let (url, name) = (url.clone(), name.clone());
        b.connect_clicked(move |_| {
            url.set_text(preset.url);
            name.set_text(preset.name);
            url.style_context().remove_class("error");
            glib::idle_add_local_once({
                let name = name.clone();
                move || name.grab_focus()
            });
        });
        row.pack_start(&b, true, true, 0);
    }
    row.upcast()
}
