//! Add/edit tab window, like the Mac's: one-click services on top, the web
//! address and name, then the rarely needed knobs (own login, notifications,
//! icon tag, browser identity) folded under "Options" with a one-line summary.

use super::icons;
use crate::config::{self, Tab, USER_AGENT_PRESETS};
use crate::links;
use crate::presets::PRESETS;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const UA_DEFAULT: &str = "Safari";
const UA_CUSTOM: &str = "Custom\u{2026}";

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
    let tab = cx.editing;
    let dialog = gtk::Dialog::new();
    dialog.set_title(if adding { "Add tab" } else { "Edit tab" });
    dialog.set_transient_for(Some(cx.parent));
    dialog.set_modal(true);
    dialog.set_destroy_with_parent(true);
    dialog.set_default_width(500);
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let ok = dialog.add_button(
        if adding { "Add Tab" } else { "Save" },
        gtk::ResponseType::Accept,
    );
    ok.style_context().add_class("suggested-action");
    dialog.set_default_response(gtk::ResponseType::Accept);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 14);
    page.set_border_width(20);
    let heading = gtk::Label::new(Some(if adding { "Add a web app" } else { "Edit tab" }));
    heading.set_xalign(0.0);
    heading.style_context().add_class("form-title");
    page.pack_start(&heading, false, false, 0);

    let entry = |value: Option<&str>, placeholder: &str| {
        let e = gtk::Entry::new();
        e.set_activates_default(true);
        e.set_placeholder_text(Some(placeholder));
        e.set_text(value.unwrap_or(""));
        e
    };
    let url = entry(tab.map(|t| t.url.as_str()), "app.example.com");
    let name = entry(tab.map(|t| t.name.as_str()), "Shown when you hover the tab");
    let url_error = caption("Enter a web address, like example.com");
    url_error.style_context().add_class("error");
    url_error.set_no_show_all(true);
    let hint = caption("");
    hint.set_no_show_all(true);

    if adding {
        page.pack_start(&presets_row(&url, &name), false, false, 0);
    }
    page.pack_start(
        &labelled(
            if adding {
                "Or any web address"
            } else {
                "Web address"
            },
            &url,
        ),
        false,
        false,
        0,
    );
    page.pack_start(&url_error, false, false, 0);
    page.pack_start(&labelled("Name", &name), false, false, 0);
    page.pack_start(&hint, false, false, 0);

    // Options.
    let own_login = gtk::Switch::new();
    own_login.set_active(tab.is_some_and(|t| t.session.is_some()));
    let notify = gtk::Switch::new();
    notify.set_active(tab.is_none_or(|t| t.notifications));
    let tag = gtk::Entry::new();
    tag.set_max_length(2);
    tag.set_width_chars(3);
    tag.set_alignment(0.5);
    tag.set_placeholder_text(Some("W"));
    tag.set_text(tab.and_then(|t| t.label.as_deref()).unwrap_or(""));
    tag.set_activates_default(true);
    tag.set_size_request(44, -1);

    let current_ua = tab.and_then(|t| t.user_agent.clone());
    let ua_names: Vec<&str> = std::iter::once(UA_DEFAULT)
        .chain(USER_AGENT_PRESETS.iter().map(|(n, _)| *n))
        .chain(std::iter::once(UA_CUSTOM))
        .collect();
    let ua_selected = match current_ua.as_deref() {
        None => UA_DEFAULT,
        Some(ua) => USER_AGENT_PRESETS
            .iter()
            .find(|(_, v)| *v == ua)
            .map(|(n, _)| *n)
            .unwrap_or(UA_CUSTOM),
    };
    let ua_custom = gtk::Entry::new();
    ua_custom.set_placeholder_text(Some(config::DEFAULT_USER_AGENT));
    ua_custom.set_no_show_all(true);
    if ua_selected == UA_CUSTOM {
        ua_custom.set_text(current_ua.as_deref().unwrap_or(""));
        ua_custom.show();
    }
    // A segmented control: linked toggle buttons, one active.
    let segments = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    segments.style_context().add_class("linked");
    segments.set_halign(gtk::Align::Start);
    let ua_choice = Rc::new(RefCell::new(ua_selected.to_string()));
    let mut group: Option<gtk::RadioButton> = None;
    for n in &ua_names {
        let b = match &group {
            None => gtk::RadioButton::with_label(n),
            Some(g) => gtk::RadioButton::with_label_from_widget(g, n),
        };
        b.set_mode(false);
        b.set_active(*n == ua_selected);
        let (choice, custom, n) = (ua_choice.clone(), ua_custom.clone(), n.to_string());
        b.connect_toggled(move |b| {
            if b.is_active() {
                custom.set_visible(n == UA_CUSTOM);
                if n == UA_CUSTOM {
                    custom.grab_focus();
                }
                *choice.borrow_mut() = n.clone();
            }
        });
        segments.pack_start(&b, false, false, 0);
        group.get_or_insert(b);
    }
    let ua_block = gtk::Box::new(gtk::Orientation::Vertical, 8);
    ua_block.pack_start(&segments, false, false, 0);
    ua_block.pack_start(&ua_custom, false, false, 0);
    ua_block.pack_start(
        &caption("Change it only if a site says your browser isn\u{2019}t supported."),
        false,
        false,
        0,
    );

    let options = gtk::Box::new(gtk::Orientation::Vertical, 14);
    options.set_margin_start(18);
    options.set_margin_top(8);
    options.pack_start(
        &option_line(
            "Own login",
            "Separate cookies and storage, for a second account",
            &own_login,
        ),
        false,
        false,
        0,
    );
    options.pack_start(
        &option_line("Notifications", "Unread badges show either way", &notify),
        false,
        false,
        0,
    );
    options.pack_start(
        &option_line(
            "Icon tag",
            "One or two letters on the icon, to tell similar tabs apart",
            &tag,
        ),
        false,
        false,
        0,
    );
    let ua_title = gtk::Label::new(Some("Browser identity"));
    ua_title.set_xalign(0.0);
    let ua_section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    ua_section.pack_start(&ua_title, false, false, 0);
    ua_section.pack_start(&ua_block, false, false, 0);
    options.pack_start(&ua_section, false, false, 0);

    let options_title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let opt_label = gtk::Label::new(Some("Options"));
    opt_label.style_context().add_class("options-title");
    let summary = gtk::Label::new(None);
    summary.style_context().add_class("dim-label");
    options_title.pack_start(&opt_label, false, false, 0);
    options_title.pack_start(&summary, false, false, 0);
    let expander = gtk::Expander::new(None);
    expander.set_label_widget(Some(&options_title));
    expander.add(&options);
    let non_default = tab.is_some()
        || own_login.is_active()
        || !notify.is_active()
        || !tag.text().is_empty()
        || ua_selected != UA_DEFAULT;
    expander.set_expanded(non_default);
    page.pack_start(&expander, false, false, 0);

    // "Shared login · Notifications on · Tag W · Chrome"
    let update_summary: Rc<dyn Fn()> = {
        let (summary, own_login, notify, tag, choice) = (
            summary.clone(),
            own_login.clone(),
            notify.clone(),
            tag.clone(),
            ua_choice.clone(),
        );
        Rc::new(move || {
            let mut parts = vec![
                if own_login.is_active() {
                    "Own login"
                } else {
                    "Shared login"
                }
                .to_string(),
                if notify.is_active() {
                    "Notifications on"
                } else {
                    "Muted"
                }
                .to_string(),
            ];
            let t = tag.text();
            if !t.trim().is_empty() {
                parts.push(format!("Tag {}", t.trim()));
            }
            let ua = choice.borrow();
            if ua.as_str() != UA_DEFAULT {
                parts.push(ua.trim_end_matches('\u{2026}').to_string());
            }
            summary.set_text(&parts.join(" \u{b7} "));
        })
    };

    // Warn when a second tab on the same site would share the first one's login.
    let existing: Rc<Vec<Tab>> = Rc::new(cx.existing.to_vec());
    let editing_url = tab.map(|t| t.url.clone());
    let update_hint: Rc<dyn Fn()> = {
        let (hint, own_login, existing, url) = (
            hint.clone(),
            own_login.clone(),
            existing.clone(),
            url.clone(),
        );
        Rc::new(move || {
            let site_of = |u: &str| {
                url::Url::parse(u)
                    .ok()
                    .and_then(|u| u.host_str().map(links::site))
            };
            let site = site_of(&config::normalize_url(&url.text()));
            let twin = existing.iter().find(|t| {
                Some(&t.url) != editing_url.as_ref()
                    && t.session.is_none()
                    && site.is_some()
                    && site_of(&t.url) == site
            });
            match twin {
                Some(t) if !own_login.is_active() => {
                    hint.set_text(&format!(
                        "This would share its login with your \u{201c}{}\u{201d} tab. Turn on Own login in Options to use a different account.",
                        t.name
                    ));
                    hint.show();
                }
                _ => hint.hide(),
            }
        })
    };
    {
        let (h, s, e) = (
            update_hint.clone(),
            update_summary.clone(),
            url_error.clone(),
        );
        url.connect_changed(move |u| {
            u.style_context().remove_class("error");
            e.hide();
            h();
            s();
        });
    }
    for sw in [&own_login, &notify] {
        let (h, s) = (update_hint.clone(), update_summary.clone());
        sw.connect_active_notify(move |_| {
            h();
            s();
        });
    }
    {
        let s = update_summary.clone();
        tag.connect_changed(move |_| s());
    }
    for b in segments.children() {
        let s = update_summary.clone();
        if let Ok(b) = b.downcast::<gtk::RadioButton>() {
            b.connect_toggled(move |_| s());
        }
    }
    update_hint();
    update_summary();

    dialog.content_area().pack_start(&page, true, true, 0);
    dialog.show_all();
    if adding {
        url.grab_focus();
    } else {
        name.grab_focus();
    }

    let result = loop {
        if dialog.run() != gtk::ResponseType::Accept {
            break None;
        }
        let choice = ua_choice.borrow().clone();
        let user_agent = match choice.as_str() {
            UA_CUSTOM => Some(ua_custom.text().to_string()),
            UA_DEFAULT => None,
            preset => USER_AGENT_PRESETS
                .iter()
                .find(|(n, _)| *n == preset)
                .map(|(_, v)| v.to_string()),
        };
        let mut t =
            Tab::new(&name.text(), &url.text(), user_agent.as_deref()).with_label(&tag.text());
        t.notifications = notify.is_active();
        t.session = if own_login.is_active() {
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
        url_error.show();
        url.grab_focus();
    };
    dialog.close();
    result
}

fn caption(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l.set_max_width_chars(60);
    l.style_context().add_class("dim-label");
    l.style_context().add_class("caption");
    l
}

/// A small label above its field.
fn labelled(label: &str, field: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    l.style_context().add_class("dim-label");
    l.style_context().add_class("caption");
    b.pack_start(&l, false, false, 0);
    b.pack_start(field, false, false, 0);
    b
}

/// Title and hint on the left, the control on the right.
fn option_line(title: &str, hint: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let t = gtk::Label::new(Some(title));
    t.set_xalign(0.0);
    text.pack_start(&t, false, false, 0);
    text.pack_start(&caption(hint), false, false, 0);
    row.pack_start(&text, true, true, 0);
    control.set_valign(gtk::Align::Center);
    row.pack_start(control, false, false, 0);
    row
}

/// The row of service chips (icon over name); clicking one fills in URL and name.
fn presets_row(url: &gtk::Entry, name: &gtk::Entry) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_homogeneous(true);
    for preset in PRESETS {
        let icon = gtk::Image::from_icon_name(Some("applications-internet"), gtk::IconSize::Dnd);
        icon.set_pixel_size(32);
        // Icons come from the favicon cache, or are fetched on first use.
        match icons::load_cached(preset.url) {
            Some(p) => {
                icon.connect_realize(move |i| icons::show(i, &p, 32));
            }
            None => {
                let weak = icon.downgrade();
                let page = preset.url;
                icons::fetch(crate::favicon::candidates(page, "[]"), move |p| {
                    let (Some(icon), Some(p)) = (weak.upgrade(), p) else {
                        return;
                    };
                    icons::store(page, &p);
                    icons::show(&icon, &p, 32);
                });
            }
        }
        let text = gtk::Label::new(Some(preset.name));
        text.style_context().add_class("caption");
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
            glib::idle_add_local_once({
                let name = name.clone();
                move || name.grab_focus()
            });
        });
        row.pack_start(&b, true, true, 0);
    }
    row.upcast()
}
