//! Settings window (gear, Ctrl+,): a sidebar with General, Tabs, Appearance and
//! About, like the Mac's. Sections are flat, each under a small uppercase
//! heading. Every change applies immediately and is written to tabs.toml; the
//! icon-size slider and the colour picker save after a short pause.

use super::{App, TabView, tabbar, web};
use crate::appearance::{self, Tint};
use crate::autostart;
use crate::config::{Config, ICON_SIZES};
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const PAGES: &[&str] = &["General", "Tabs", "Appearance", "About"];
const WEBSITE: &str = "https://github.com/ivanm/webtabs";
const VAAN: &str = "https://vaan.dev";
const GPL: &str = "https://www.gnu.org/licenses/gpl-3.0.html";
/// Drag-and-drop target for reordering rows (this app only).
const ROW_TARGET: &str = "webtabs/tab-row";

pub fn open(app: &Rc<App>) {
    open_at(app, "General");
}

/// Opens the window (or brings it back) on `page`.
pub fn open_at(app: &Rc<App>, page: &str) {
    if let Some(ui) = app.settings_ui.borrow().as_ref()
        && let Some(dialog) = ui.dialog.upgrade()
    {
        ui.select(page);
        dialog.present();
        return;
    }
    let dialog = gtk::Dialog::new();
    dialog.set_title("Settings");
    dialog.set_transient_for(Some(&app.window));
    dialog.set_modal(true);
    dialog.set_destroy_with_parent(true);
    dialog.set_default_size(760, 680);
    dialog.style_context().add_class("settings");

    let side = gtk::ListBox::new();
    side.style_context().add_class("settings-side");
    side.set_size_request(176, -1);
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    let dark = tabbar::theme_dark(&app.window);
    install_css(dark);

    let ui = Rc::new(Ui {
        dialog: dialog.downgrade(),
        side: side.clone(),
        stack: stack.clone(),
        tabs_list: gtk::Box::new(gtk::Orientation::Vertical, 0),
        size: gtk::Scale::with_range(
            gtk::Orientation::Horizontal,
            *ICON_SIZES.start() as f64,
            *ICON_SIZES.end() as f64,
            2.0,
        ),
        size_label: gtk::Label::new(None),
        tint_label: gtk::Label::new(None),
        swatches: RefCell::default(),
        preview: gtk::Box::new(gtk::Orientation::Horizontal, 0),
        preview_look: Rc::new(Cell::new(app.look.get())),
        preview_tiles: RefCell::default(),
        close_row: RefCell::default(),
        syncing: Cell::new(false),
    });

    for name in PAGES {
        let dot = gtk::Label::new(Some("\u{25cf}"));
        dot.style_context().add_class("side-dot");
        let label = gtk::Label::new(Some(name));
        label.set_xalign(0.0);
        let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row_box.pack_start(&dot, false, false, 0);
        row_box.pack_start(&label, true, true, 0);
        let row = gtk::ListBoxRow::new();
        row.add(&row_box);
        // Selection shows where you are; a focus ring on top is noise.
        row.set_can_focus(false);
        row.style_context().add_class("side-row");
        side.add(&row);
    }
    stack.add_named(&scrolled(general(app, &ui)), "General");
    stack.add_named(&tabs_page(app, &ui), "Tabs");
    stack.add_named(&scrolled(appearance_page(app, &ui)), "Appearance");
    stack.add_named(&about(app), "About");
    {
        let stack = stack.clone();
        side.connect_row_selected(move |_, row| {
            if let Some(row) = row
                && let Some(name) = PAGES.get(row.index() as usize)
            {
                stack.set_visible_child_name(name);
            }
        });
    }

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.pack_start(&side, false, false, 0);
    body.pack_start(&stack, true, true, 0);
    dialog.content_area().pack_start(&body, true, true, 0);
    dialog.content_area().set_border_width(0);

    let refresh: Rc<dyn Fn()> = {
        let (app, ui) = (Rc::downgrade(app), Rc::downgrade(&ui));
        Rc::new(move || {
            if let (Some(app), Some(ui)) = (app.upgrade(), ui.upgrade()) {
                ui.sync(&app);
            }
        })
    };
    app.settings_refresh.replace(Some(refresh.clone()));
    app.settings_ui.replace(Some(ui.clone()));
    ui.sync(app);
    ui.select(page_or_general(page));

    dialog.show_all();
    dialog.run();
    app.settings_ui.replace(None);
    app.settings_refresh.replace(None);
    dialog.close();
}

fn page_or_general(page: &str) -> &str {
    if PAGES.contains(&page) {
        page
    } else {
        "General"
    }
}

/// A tint swatch: its value (`Some("custom")` for the picker), the drawn
/// circle, and whether it's the selected one.
type Swatch = (Option<String>, gtk::DrawingArea, Rc<Cell<bool>>);

/// The open window's widgets that follow App state.
pub struct Ui {
    dialog: glib::WeakRef<gtk::Dialog>,
    side: gtk::ListBox,
    stack: gtk::Stack,
    tabs_list: gtk::Box,
    size: gtk::Scale,
    size_label: gtk::Label,
    tint_label: gtk::Label,
    /// (tint value, its swatch, selected flag); "custom" marks the picker.
    swatches: RefCell<Vec<Swatch>>,
    preview: gtk::Box,
    preview_look: Rc<Cell<tabbar::Look>>,
    preview_tiles: RefCell<Vec<Rc<tabbar::Tile>>>,
    /// "Close to tray": greyed out while there's no tray to close to.
    close_row: RefCell<Option<gtk::Widget>>,
    /// Set while controls are moved to match the config, so they don't write back.
    syncing: Cell<bool>,
}

impl Ui {
    fn select(&self, page: &str) {
        let i = PAGES.iter().position(|p| *p == page).unwrap_or(0);
        if let Some(row) = self.side.row_at_index(i as i32) {
            self.side.select_row(Some(&row));
        }
        self.stack.set_visible_child_name(page_or_general(page));
    }

    fn sync(&self, app: &Rc<App>) {
        self.syncing.set(true);
        let cfg = app.config.borrow().clone();
        if let Some(row) = self.close_row.borrow().as_ref() {
            row.set_sensitive(cfg.tray);
        }
        fill_tabs(app, self);
        self.size.set_value(cfg.icon_size as f64);
        self.size_label.set_text(&format!("{} pt", cfg.icon_size));
        let custom = cfg.tint.as_deref().is_some_and(|t| t.starts_with('#'));
        for (tint, area, selected) in self.swatches.borrow().iter() {
            let on = if tint.as_deref() == Some("custom") {
                custom
            } else {
                *tint == cfg.tint
            };
            selected.set(on);
            area.queue_draw();
        }
        self.tint_label.set_text(&appearance::tint_label(
            cfg.tint.as_deref(),
            "Default (theme)",
        ));
        self.sync_preview(app);
        self.syncing.set(false);
    }

    /// The mini tab bar: the first four tabs, the first active with a "3".
    fn sync_preview(&self, app: &Rc<App>) {
        let look = app.look.get();
        self.preview_look.set(look);
        tabbar::paint_preview(&self.preview, &look);
        for t in self.preview_tiles.borrow().iter() {
            self.preview.remove(&t.area);
        }
        let mut tiles = Vec::new();
        for (i, tv) in app.tabs.borrow().iter().take(4).enumerate() {
            let t = tabbar::Tile::new(tabbar::Kind::Tab, &self.preview_look);
            t.set_icon(tv.pixbuf.borrow().as_ref());
            let tab = tv.tab.borrow();
            t.set_name(&tab.name);
            t.set_tag(tab.label.as_deref());
            t.set_active(i == 0);
            t.set_badge(if i == 0 { "3" } else { "" });
            t.area.set_sensitive(false);
            tiles.push(t);
        }
        tiles.push(tabbar::Tile::new(tabbar::Kind::Add, &self.preview_look));
        for t in &tiles {
            self.preview.pack_start(&t.area, false, false, 0);
            t.area.show();
        }
        *self.preview_tiles.borrow_mut() = tiles;
    }
}

// ---- General ----------------------------------------------------------------------

fn general(app: &Rc<App>, ui: &Rc<Ui>) -> gtk::Box {
    let entry = autostart::entry_path();
    let login = switch_row(
        "Open at login",
        Some("Opens hidden in the tray when you log in"),
        autostart::is_enabled(&entry),
        {
            let app = Rc::downgrade(app);
            move |on| {
                let exe = std::env::current_exe().unwrap_or_else(|_| "webtabs".into());
                match autostart::set(&entry, on, &exe) {
                    Ok(()) => true,
                    Err(e) => {
                        if let Some(app) = app.upgrade() {
                            app.error("Could not change Open at login", &e.to_string());
                        }
                        false
                    }
                }
            }
        },
    );
    let opt =
        |title: &str, sub: Option<&str>, get: fn(&Config) -> bool, set: fn(&mut Config, bool)| {
            let current = get(&app.config.borrow());
            let app = Rc::downgrade(app);
            switch_row(title, sub, current, move |on| {
                if let Some(app) = app.upgrade() {
                    app.update_options(|c| set(c, on));
                }
                true
            })
        };
    let notifications = opt(
        "Show notifications",
        Some("Notifications from your tabs"),
        |c| c.notifications,
        |c, v| c.notifications = v,
    );
    let title = opt(
        "Unread count in window title",
        Some("e.g. \u{201c}WebTabs (5)\u{201d}"),
        |c| c.unread_in_title,
        |c, v| c.unread_in_title = v,
    );
    let launcher = opt(
        "Unread count on launcher icon",
        Some("On docks and panels that show badges"),
        |c| c.unread_in_dock,
        |c, v| c.unread_in_dock = v,
    );
    let tray = opt(
        "Show icon in tray",
        Some("Unread indicator and quick menu in the panel"),
        |c| c.tray,
        |c, v| c.tray = v,
    );
    let close = opt(
        "Close to tray",
        Some("Closing the window keeps WebTabs running"),
        |c| c.close_to_tray,
        |c, v| c.close_to_tray = v,
    );
    close.set_margin_start(18);
    // Closing to a tray that isn't there would make the app unreachable.
    close.set_sensitive(app.config.borrow().tray);
    ui.close_row.replace(Some(close.clone().upcast()));
    let gpu = opt(
        "GPU acceleration",
        Some("Applies after restarting WebTabs"),
        |c| c.gpu_acceleration,
        |c, v| c.gpu_acceleration = v,
    );
    page(&[
        section("Startup", &[login]),
        section("Notifications", &[notifications, title, launcher]),
        section("Tray", &[tray, close]),
        section("Performance", &[gpu]),
    ])
}

// ---- Tabs -------------------------------------------------------------------------

fn tabs_page(app: &Rc<App>, ui: &Rc<Ui>) -> gtk::Box {
    let hint = note("Drag a tab to change its place in the tab bar.");
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.add(&ui.tabs_list);

    let add = gtk::Button::with_label("Add Tab\u{2026}");
    let toml = gtk::Button::with_label("Open tabs.toml");
    toml.set_relief(gtk::ReliefStyle::None);
    toml.style_context().add_class("link");
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
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.pack_start(&add, false, false, 0);
    actions.pack_end(&toml, false, false, 0);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.style_context().add_class("settings-page");
    page.pack_start(&heading("Tabs"), false, false, 0);
    page.pack_start(&hint, false, false, 0);
    page.pack_start(&scroll, true, true, 0);
    page.pack_start(&actions, false, false, 0);
    page
}

fn fill_tabs(app: &Rc<App>, ui: &Ui) {
    let list = &ui.tabs_list;
    list.foreach(|c| list.remove(c));
    let tabs = app.tabs.borrow().clone();
    if tabs.is_empty() {
        let empty = note("No tabs yet");
        empty.set_margin_top(18);
        list.pack_start(&empty, false, false, 0);
    }
    for (i, tv) in tabs.iter().enumerate() {
        if i > 0 {
            list.pack_start(
                &gtk::Separator::new(gtk::Orientation::Horizontal),
                false,
                false,
                0,
            );
        }
        list.pack_start(&tab_row(app, tv, i, tabs.len()), false, false, 0);
    }
    list.show_all();
}

fn tab_row(app: &Rc<App>, tv: &Rc<TabView>, index: usize, count: usize) -> gtk::Widget {
    let tab = tv.tab.borrow().clone();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.style_context().add_class("tab-row");

    // Drag handle: six dots; dragging it onto another row moves the tab there.
    let grip = gtk::EventBox::new();
    let dots = gtk::DrawingArea::new();
    dots.set_size_request(14, 24);
    dots.connect_draw(|a, cr| {
        let c = a.style_context().color(gtk::StateFlags::NORMAL);
        cr.set_source_rgba(c.red(), c.green(), c.blue(), 0.4);
        for (x, y) in [
            (5.0, 7.0),
            (9.0, 7.0),
            (5.0, 12.0),
            (9.0, 12.0),
            (5.0, 17.0),
            (9.0, 17.0),
        ] {
            cr.arc(x, y, 1.2, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
        glib::Propagation::Stop
    });
    grip.add(&dots);
    grip.set_tooltip_text(Some("Drag to reorder"));
    let targets = [gtk::TargetEntry::new(
        ROW_TARGET,
        gtk::TargetFlags::SAME_APP,
        0,
    )];
    grip.drag_source_set(
        gdk::ModifierType::BUTTON1_MASK,
        &targets,
        gdk::DragAction::MOVE,
    );
    let id = tv.id;
    grip.connect_drag_data_get(move |_, _, data, _, _| {
        data.set_text(&id.to_string());
    });
    grip.connect_realize(|g| {
        if let Some(win) = g.window() {
            win.set_cursor(gdk::Cursor::from_name(&win.display(), "grab").as_ref());
        }
    });
    row.pack_start(&grip, false, false, 0);

    row.pack_start(
        &tabbar::icon_area(
            tv.pixbuf.borrow().as_ref(),
            &tab.name,
            tab.label.as_deref(),
            26,
        ),
        false,
        false,
        0,
    );

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(&tab.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.pack_start(&name, false, true, 0);
    if tab.session.is_some() {
        let chip = gtk::Label::new(Some("Own login"));
        chip.style_context().add_class("chip");
        title.pack_start(&chip, false, false, 0);
    }
    let url = gtk::Label::new(Some(&tab.url));
    url.set_xalign(0.0);
    url.set_ellipsize(gtk::pango::EllipsizeMode::End);
    url.style_context().add_class("dim-label");
    url.style_context().add_class("caption");
    text.pack_start(&title, false, false, 0);
    text.pack_start(&url, false, false, 0);
    text.set_valign(gtk::Align::Center);
    row.pack_start(&text, true, true, 0);

    let bell = gtk::Button::new();
    bell.set_relief(gtk::ReliefStyle::None);
    bell.set_valign(gtk::Align::Center);
    let on = tab.notifications;
    bell.add(&glyph(if on { Glyph::Bell } else { Glyph::BellOff }));
    bell.set_tooltip_text(Some(if on {
        "Notifications on (click to mute)"
    } else {
        "Muted (click to unmute)"
    }));
    if !on {
        bell.style_context().add_class("dim-label");
    }
    let (aw, tw) = (Rc::downgrade(app), Rc::downgrade(tv));
    bell.connect_clicked(move |_| {
        if let (Some(a), Some(t)) = (aw.upgrade(), tw.upgrade()) {
            glib::idle_add_local_once(move || a.set_tab_notifications(&t, !on));
        }
    });
    row.pack_start(&bell, false, false, 0);

    let more = gtk::MenuButton::new();
    more.set_relief(gtk::ReliefStyle::None);
    more.set_valign(gtk::Align::Center);
    more.add(&glyph(Glyph::More));
    more.set_tooltip_text(Some("Edit, move or remove"));
    let menu = gtk::Menu::new();
    let item = |label: &str, enabled: bool, f: fn(&Rc<App>, &Rc<TabView>)| {
        let mi = gtk::MenuItem::with_label(label);
        mi.set_sensitive(enabled);
        let (aw, tw) = (Rc::downgrade(app), Rc::downgrade(tv));
        mi.connect_activate(move |_| {
            if let (Some(a), Some(t)) = (aw.upgrade(), tw.upgrade()) {
                // Rebuilding the list destroys this menu; do it after it closes.
                glib::idle_add_local_once(move || f(&a, &t));
            }
        });
        menu.append(&mi);
    };
    item("Edit\u{2026}", true, |a, t| a.edit_tab_dialog(t));
    item("Move Up", index > 0, |a, t| a.move_tab(t, -1));
    item("Move Down", index + 1 < count, |a, t| a.move_tab(t, 1));
    menu.append(&gtk::SeparatorMenuItem::new());
    item("Remove\u{2026}", true, |a, t| a.confirm_remove(t));
    menu.show_all();
    more.set_popup(Some(&menu));
    row.pack_start(&more, false, false, 0);

    // The whole row takes drops.
    let drop = gtk::EventBox::new();
    drop.add(&row);
    drop.drag_dest_set(gtk::DestDefaults::ALL, &targets, gdk::DragAction::MOVE);
    let aw = Rc::downgrade(app);
    drop.connect_drag_data_received(move |_, _, _, _, data, _, _| {
        let Some(app) = aw.upgrade() else { return };
        let moved = data.text().and_then(|t| t.parse::<u64>().ok());
        let tabs = app.tabs.borrow().clone();
        if let Some(tv) = moved.and_then(|id| tabs.iter().find(|t| t.id == id).cloned()) {
            glib::idle_add_local_once(move || app.move_tab_to(&tv, index));
        }
    });
    drop.upcast()
}

// ---- Appearance -------------------------------------------------------------------

fn appearance_page(app: &Rc<App>, ui: &Rc<Ui>) -> gtk::Box {
    ui.preview.style_context().add_class("preview");
    ui.preview.set_halign(gtk::Align::Start);
    ui.preview.set_size_request(300, -1);

    let size = &ui.size;
    size.set_draw_value(false);
    size.set_hexpand(true);
    for v in ICON_SIZES.step_by(2) {
        size.add_mark(v as f64, gtk::PositionType::Bottom, None);
    }
    size.set_round_digits(0);
    let weak = (Rc::downgrade(app), Rc::downgrade(ui));
    size.connect_value_changed(move |s| {
        let (Some(app), Some(ui)) = (weak.0.upgrade(), weak.1.upgrade()) else {
            return;
        };
        // Snap to even sizes, like the Mac's tick marks.
        let v = ((s.value() / 2.0).round() * 2.0) as u32;
        if ui.syncing.get() || v == app.config.borrow().icon_size {
            return;
        }
        ui.size_label.set_text(&format!("{v} pt"));
        app.update_options_later(move |c| c.icon_size = v);
    });
    ui.size_label.set_width_chars(6);
    ui.size_label.set_xalign(1.0);
    let small = gtk::Label::new(Some("\u{25a0}"));
    small.style_context().add_class("size-small");
    let big = gtk::Label::new(Some("\u{25a0}"));
    big.style_context().add_class("size-big");
    for g in [&small, &big] {
        g.style_context().add_class("dim-label");
    }
    let size_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    size_row.pack_start(&small, false, false, 0);
    size_row.pack_start(size, true, true, 0);
    size_row.pack_start(&big, false, false, 0);
    size_row.pack_start(&ui.size_label, false, false, 0);

    let dark = tabbar::theme_dark(&app.window);
    let swatch_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let mut entries: Vec<(Option<String>, &str)> = vec![(None, "Default")];
    for name in appearance::tint_names() {
        entries.push((Some(name.to_string()), name));
    }
    entries.push((Some("custom".into()), "Custom"));
    for (tint, label) in entries {
        let selected = Rc::new(Cell::new(false));
        let color = match tint.as_deref() {
            None => None,
            Some("custom") => Some(None),
            Some(t) => Tint::parse(Some(t)).map(|c| Some(c.color(dark))),
        };
        let area = tabbar::swatch(color, selected.clone(), dark);
        let button = gtk::EventBox::new();
        button.add(&area);
        button.set_tooltip_text(Some(&appearance::tint_label(
            if label == "Default" || label == "Custom" {
                None
            } else {
                tint.as_deref()
            },
            label,
        )));
        let weak = Rc::downgrade(app);
        let value = tint.clone();
        button.connect_button_release_event(move |b, _| {
            let Some(app) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match value.as_deref() {
                Some("custom") => pick_color(&app, b),
                _ => {
                    let v = value.clone();
                    app.update_options(move |c| c.tint = v);
                }
            }
            glib::Propagation::Stop
        });
        swatch_row.pack_start(&button, false, false, 0);
        ui.swatches.borrow_mut().push((tint, area, selected));
    }
    ui.tint_label.set_xalign(0.0);
    ui.tint_label.style_context().add_class("dim-label");
    ui.tint_label.style_context().add_class("caption");

    let preview_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview_wrap.pack_start(&ui.preview, false, false, 0);
    page(&[
        preview_wrap.upcast(),
        section(
            "Icon size",
            &[
                size_row.upcast(),
                note("Above 24 pt the title bar grows to fit the icons.").upcast(),
            ],
        ),
        section(
            "Tab bar color",
            &[
                swatch_row.upcast(),
                ui.tint_label.clone().upcast(),
                note("Tints the tab bar. Pages keep their own colors.").upcast(),
            ],
        ),
    ])
}

/// The Custom swatch: GTK's colour chooser, applied live while it's open.
fn pick_color(app: &Rc<App>, anchor: &impl IsA<gtk::Widget>) {
    let parent = anchor
        .toplevel()
        .and_then(|w| w.downcast::<gtk::Window>().ok());
    let chooser = gtk::ColorChooserDialog::new(Some("Tab bar color"), parent.as_ref());
    chooser.set_use_alpha(false);
    let start = app
        .config
        .borrow()
        .tint
        .as_deref()
        .filter(|t| t.starts_with('#'))
        .and_then(|t| u32::from_str_radix(&t[1..], 16).ok())
        .unwrap_or(0xF4E3D0);
    let c = |s: u32| ((start >> s) & 0xff) as f64 / 255.0;
    chooser.set_rgba(&gdk::RGBA::new(c(16), c(8), c(0), 1.0));
    let hex = |c: &gdk::RGBA| {
        let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", b(c.red()), b(c.green()), b(c.blue()))
    };
    let weak = Rc::downgrade(app);
    chooser.connect_rgba_notify(move |ch| {
        if let Some(app) = weak.upgrade() {
            let v = hex(&ch.rgba());
            if app.config.borrow().tint.as_deref() != Some(v.as_str()) {
                app.update_options_later(move |c| c.tint = Some(v));
            }
        }
    });
    let ok = chooser.run() == gtk::ResponseType::Ok;
    let picked = hex(&chooser.rgba());
    chooser.close();
    if ok {
        app.update_options(move |c| c.tint = Some(picked));
    }
}

// ---- About ------------------------------------------------------------------------

fn about(app: &Rc<App>) -> gtk::Box {
    let icon = gtk::Image::new();
    if let Some(p) = super::app_icons()
        .ok()
        .and_then(|l| l.into_iter().max_by_key(|p| p.width()))
        .and_then(|p: Pixbuf| p.scale_simple(72, 72, gtk::gdk_pixbuf::InterpType::Hyper))
    {
        icon.set_from_pixbuf(Some(&p));
    }
    let name = gtk::Label::new(Some("WebTabs"));
    name.style_context().add_class("about-name");
    // "v0.3.0 by vaan.dev", the name a link to the site.
    let version = gtk::Label::new(None);
    version.set_markup(&format!(
        "v{} by <a href=\"{VAAN}\">vaan.dev</a>",
        env!("CARGO_PKG_VERSION")
    ));
    version.style_context().add_class("dim-label");
    version.connect_activate_link(|_, uri| {
        web::open_external(uri);
        glib::Propagation::Stop
    });
    let license = gtk::Label::new(None);
    license.set_markup(&format!(
        "Licensed under the <a href=\"{GPL}\">GPL 3.0 or later</a>"
    ));
    license.style_context().add_class("dim-label");
    license.style_context().add_class("caption");
    license.connect_activate_link(|_, uri| {
        web::open_external(uri);
        glib::Propagation::Stop
    });
    let toml = gtk::Button::with_label("Open tabs.toml");
    let site = gtk::Button::with_label("Website");
    let weak = Rc::downgrade(app);
    toml.connect_clicked(move |_| {
        if let Some(app) = weak.upgrade() {
            web::open_external(&app.config_uri());
        }
    });
    site.connect_clicked(|_| web::open_external(WEBSITE));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::Center);
    buttons.pack_start(&toml, false, false, 0);
    buttons.pack_start(&site, false, false, 0);
    let stack = gtk::Box::new(gtk::Orientation::Vertical, 8);
    stack.set_valign(gtk::Align::Center);
    stack.set_halign(gtk::Align::Center);
    for w in [
        icon.upcast_ref::<gtk::Widget>(),
        name.upcast_ref(),
        version.upcast_ref(),
        license.upcast_ref(),
    ] {
        stack.pack_start(w, false, false, 0);
    }
    buttons.set_margin_top(10);
    stack.pack_start(&buttons, false, false, 0);
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.pack_start(&stack, true, true, 0);
    page
}

// ---- building blocks --------------------------------------------------------------

#[derive(Clone, Copy)]
enum Glyph {
    Bell,
    BellOff,
    More,
}

/// Row icons drawn in the label colour, the same on every icon theme (some
/// themes have no bell, or draw "more" as vertical dots).
fn glyph(kind: Glyph) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    a.set_size_request(16, 16);
    a.connect_draw(move |a, cr| {
        let c = a.style_context().color(a.state_flags());
        let alpha = if matches!(kind, Glyph::BellOff) {
            0.45
        } else {
            0.8
        };
        cr.set_source_rgba(c.red(), c.green(), c.blue(), alpha);
        cr.set_line_width(1.4);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        match kind {
            Glyph::More => {
                for x in [3.5, 8.0, 12.5] {
                    cr.arc(x, 8.0, 1.3, 0.0, std::f64::consts::TAU);
                    let _ = cr.fill();
                }
            }
            Glyph::Bell | Glyph::BellOff => {
                // The Mac row's bell: dome, flared rim, clapper.
                cr.move_to(3.5, 11.5);
                cr.line_to(12.5, 11.5);
                cr.line_to(11.5, 10.0);
                cr.line_to(11.5, 7.0);
                cr.arc_negative(8.0, 7.0, 3.5, 0.0, std::f64::consts::PI);
                cr.line_to(4.5, 10.0);
                cr.close_path();
                cr.move_to(6.5, 13.5);
                cr.curve_to(6.8, 14.6, 9.2, 14.6, 9.5, 13.5);
                if matches!(kind, Glyph::BellOff) {
                    cr.move_to(2.0, 2.0);
                    cr.line_to(14.0, 14.0);
                }
                let _ = cr.stroke();
            }
        }
        glib::Propagation::Stop
    });
    a
}

fn scrolled(child: gtk::Box) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    s.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    s.add(&child);
    s
}

fn page(children: &[gtk::Widget]) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 20);
    page.style_context().add_class("settings-page");
    for c in children {
        page.pack_start(c, false, false, 0);
    }
    page
}

fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(&text.to_uppercase()));
    l.style_context().add_class("section-heading");
    l.set_xalign(0.0);
    l
}

fn note(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l.style_context().add_class("dim-label");
    l.style_context().add_class("caption");
    l
}

/// A heading, its rows, and a separator line under it.
fn section(title: &str, rows: &[gtk::Widget]) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.style_context().add_class("settings-section");
    b.pack_start(&heading(title), false, false, 0);
    for r in rows {
        b.pack_start(r, false, false, 0);
    }
    b.upcast()
}

/// A title/subtitle row with a switch. `apply` returns whether the change stuck;
/// if not, the switch flips back.
fn switch_row(
    title: &str,
    subtitle: Option<&str>,
    active: bool,
    apply: impl Fn(bool) -> bool + 'static,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let t = gtk::Label::new(Some(title));
    t.set_xalign(0.0);
    text.pack_start(&t, false, false, 0);
    if let Some(sub) = subtitle {
        let s = gtk::Label::new(Some(sub));
        s.set_xalign(0.0);
        s.style_context().add_class("dim-label");
        s.style_context().add_class("caption");
        text.pack_start(&s, false, false, 0);
    }
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
    row.upcast()
}

fn install_css(dark: bool) {
    thread_local! {
        static CSS: gtk::CssProvider = gtk::CssProvider::new();
    }
    let accent = tabbar::accent_css(dark);
    let css = format!(
        "
.settings-side {{ background: alpha(currentColor, 0.03);
                  border-right: 1px solid alpha(currentColor, 0.08); padding: 16px 10px; }}
.settings-side row {{ min-height: 32px; padding: 0 10px; border-radius: 7px; margin-bottom: 2px; }}
.settings-side row:selected {{ background: alpha(currentColor, 0.09); color: inherit; font-weight: 600; }}
.settings-side row:not(:selected) {{ opacity: 0.75; }}
.side-dot {{ font-size: 7px; color: transparent; }}
.settings-side row:selected .side-dot {{ color: {accent}; }}
.settings-page {{ padding: 22px 32px; }}
.settings-section {{ padding-bottom: 20px; border-bottom: 1px solid alpha(currentColor, 0.10); }}
.settings-section:last-child {{ border-bottom: none; }}
.section-heading {{ font-size: 11px; font-weight: 600; letter-spacing: 0.06em; opacity: 0.65; }}
.tab-row {{ padding: 6px 0; }}
.chip {{ font-size: 11px; opacity: 0.75; border: 1px solid alpha(currentColor, 0.25);
         border-radius: 5px; padding: 0 5px; }}
.link {{ color: {accent}; }}
.preview {{ border-radius: 10px; border: 1px solid alpha(currentColor, 0.12); padding: 0 10px; }}
.size-small {{ font-size: 9px; }}
.size-big {{ font-size: 17px; }}
.about-name {{ font-size: 22px; font-weight: 600; }}
"
    );
    CSS.with(|p| {
        if let Err(e) = p.load_from_data(css.as_bytes()) {
            eprintln!("webtabs: settings css: {e}");
        }
        if let Some(screen) = gdk::Screen::default() {
            gtk::StyleContext::add_provider_for_screen(
                &screen,
                p,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}
