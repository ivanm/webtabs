//! The tab bar's look, drawn with cairo to match the Mac and Windows bars
//! (`src/appearance.rs`): flat rounded tiles that light up on hover and while
//! active, an orange marker under the active tab, the unread bubble and icon tag
//! cut into the bar with a ring in its colour, and the + and settings tiles.
//! The bar sits under the window's own title bar, so every desktop keeps its
//! decorations; `tint` colours only the bar.

use crate::appearance::{self, BarMetrics, Tint};
use gtk::gdk::prelude::GdkContextExt;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{cairo, gdk, glib, pango};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Fallback icon colours (no site icon yet), picked by tab name.
const PALETTE: &[u32] = &[
    0x2563eb, 0x16a34a, 0xd97706, 0xdc2626, 0x7c3aed, 0x0891b2, 0xdb2777,
];

/// Colours and geometry every tile draws with; one per window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub metrics: BarMetrics,
    /// The bar's background (the theme's, or the tint).
    pub bar: Rgb,
    /// Label colour on the bar (glyphs, highlights, the tag pill).
    pub fg: Rgb,
    /// Whether the bar is dark (picks the marker's shade).
    pub dark: bool,
    /// Whether `bar` is a tint (painted) rather than the theme's own background.
    pub tinted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb(pub f64, pub f64, pub f64);

impl Rgb {
    pub fn hex(v: u32) -> Rgb {
        let c = |s: u32| ((v >> s) & 0xff) as f64 / 255.0;
        Rgb(c(16), c(8), c(0))
    }
    fn from_rgba(c: &gdk::RGBA) -> Rgb {
        Rgb(c.red(), c.green(), c.blue())
    }
    fn set(&self, cr: &cairo::Context, alpha: f64) {
        cr.set_source_rgba(self.0, self.1, self.2, alpha);
    }
    pub fn css(&self) -> String {
        let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", b(self.0), b(self.1), b(self.2))
    }
    fn luminance_dark(&self) -> bool {
        0.2126 * self.0 + 0.7152 * self.1 + 0.0722 * self.2 < 0.5
    }
}

impl Look {
    /// Resolves the theme's colours (read from `widget`'s style) and the config.
    pub fn resolve(widget: &impl IsA<gtk::Widget>, icon_size: u32, tint: Option<&str>) -> Look {
        let sc = widget.style_context();
        let theme_bg = sc
            .lookup_color("theme_bg_color")
            .map(|c| Rgb::from_rgba(&c))
            .unwrap_or(Rgb(0.96, 0.96, 0.96));
        let theme_fg = Rgb::from_rgba(&sc.color(gtk::StateFlags::NORMAL));
        // The GTK theme's dark preference: what its window background looks like.
        let theme_dark = theme_bg.luminance_dark();
        let metrics = BarMetrics::new(icon_size);
        match Tint::parse(tint) {
            None => Look {
                metrics,
                bar: theme_bg,
                fg: theme_fg,
                dark: theme_dark,
                tinted: false,
            },
            Some(t) => {
                let light_text = t.light_text(theme_dark);
                Look {
                    metrics,
                    bar: Rgb::hex(t.color(theme_dark)),
                    fg: if light_text {
                        Rgb(0.93, 0.93, 0.94)
                    } else {
                        Rgb(0.11, 0.11, 0.12)
                    },
                    dark: light_text,
                    tinted: true,
                }
            }
        }
    }

    fn accent(&self) -> Rgb {
        Rgb::hex(if self.dark {
            appearance::ACCENT_DARK
        } else {
            appearance::ACCENT_LIGHT
        })
    }
}

/// What a tile shows; set through `Tile`'s methods.
#[derive(Default)]
struct State {
    icon: Option<Pixbuf>,
    /// The icon scaled for the current size and HiDPI scale, keyed by both.
    scaled: Option<(i32, Pixbuf)>,
    name: String,
    badge: String,
    tag: String,
    active: bool,
    hover: bool,
    pressed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Tab,
    Add,
    Settings,
}

/// One drawn tile: a tab, or the bar's + and settings buttons.
pub struct Tile {
    pub area: gtk::DrawingArea,
    kind: Kind,
    look: Rc<Cell<Look>>,
    state: RefCell<State>,
    on_click: RefCell<Option<Box<dyn Fn()>>>,
}

impl Tile {
    pub fn new(kind: Kind, look: &Rc<Cell<Look>>) -> Rc<Tile> {
        let area = gtk::DrawingArea::new();
        area.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::BUTTON_RELEASE_MASK
                | gdk::EventMask::ENTER_NOTIFY_MASK
                | gdk::EventMask::LEAVE_NOTIFY_MASK,
        );
        area.set_valign(gtk::Align::Start);
        let tile = Rc::new(Tile {
            area,
            kind,
            look: look.clone(),
            state: RefCell::default(),
            on_click: RefCell::default(),
        });
        tile.resize();
        let w = Rc::downgrade(&tile);
        tile.area.connect_draw(move |_, cr| {
            if let Some(t) = w.upgrade() {
                t.draw(cr);
            }
            glib::Propagation::Stop
        });
        let w = Rc::downgrade(&tile);
        tile.area.connect_enter_notify_event(move |a, _| {
            if let Some(t) = w.upgrade() {
                t.state.borrow_mut().hover = true;
                a.queue_draw();
            }
            if let Some(win) = a.window() {
                win.set_cursor(gdk::Cursor::from_name(&win.display(), "pointer").as_ref());
            }
            glib::Propagation::Proceed
        });
        let w = Rc::downgrade(&tile);
        tile.area.connect_leave_notify_event(move |a, _| {
            if let Some(t) = w.upgrade() {
                let mut s = t.state.borrow_mut();
                s.hover = false;
                s.pressed = false;
                a.queue_draw();
            }
            if let Some(win) = a.window() {
                win.set_cursor(None);
            }
            glib::Propagation::Proceed
        });
        let w = Rc::downgrade(&tile);
        tile.area.connect_button_press_event(move |a, ev| {
            if ev.button() == 1
                && ev.event_type() == gdk::EventType::ButtonPress
                && let Some(t) = w.upgrade()
            {
                t.state.borrow_mut().pressed = true;
                a.queue_draw();
            }
            glib::Propagation::Proceed
        });
        let w = Rc::downgrade(&tile);
        tile.area.connect_button_release_event(move |a, ev| {
            let Some(t) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let was = std::mem::replace(&mut t.state.borrow_mut().pressed, false);
            a.queue_draw();
            if ev.button() == 1 && was && t.state.borrow().hover {
                // Run after the event: the handler may rebuild the bar.
                let w = Rc::downgrade(&t);
                glib::idle_add_local_once(move || {
                    if let Some(t) = w.upgrade()
                        && let Some(f) = t.on_click.borrow().as_ref()
                    {
                        f();
                    }
                });
            }
            glib::Propagation::Proceed
        });
        tile
    }

    pub fn connect_click(&self, f: impl Fn() + 'static) {
        *self.on_click.borrow_mut() = Some(Box::new(f));
    }

    /// Follows a new icon size: tabs leave room for their bubble and the gap.
    pub fn resize(&self) {
        let m = self.look.get().metrics;
        match self.kind {
            Kind::Tab => self.area.set_size_request(m.tile + m.gap, m.height),
            Kind::Add | Kind::Settings => self.area.set_size_request(28, m.height),
        }
        self.state.borrow_mut().scaled = None;
        self.area.queue_draw();
    }

    pub fn set_icon(&self, icon: Option<&Pixbuf>) {
        let mut s = self.state.borrow_mut();
        s.icon = icon.cloned();
        s.scaled = None;
        drop(s);
        self.area.queue_draw();
    }

    pub fn set_name(&self, name: &str) {
        self.state.borrow_mut().name = name.to_string();
        self.area.queue_draw();
    }

    pub fn set_badge(&self, text: &str) {
        self.state.borrow_mut().badge = text.to_string();
        self.area.queue_draw();
    }

    pub fn set_tag(&self, tag: Option<&str>) {
        self.state.borrow_mut().tag = tag.unwrap_or("").to_string();
        self.area.queue_draw();
    }

    pub fn set_active(&self, active: bool) {
        if self
            .state
            .replace_with(|s| State {
                active,
                ..std::mem::take(s)
            })
            .active
            != active
        {
            self.area.queue_draw();
        }
    }

    fn draw(&self, cr: &cairo::Context) {
        let look = self.look.get();
        match self.kind {
            Kind::Tab => self.draw_tab(cr, &look),
            Kind::Add | Kind::Settings => self.draw_glyph(cr, &look),
        }
    }

    fn highlight(&self, cr: &cairo::Context, look: &Look, x: f64, y: f64, side: f64, r: f64) {
        let s = self.state.borrow();
        let alpha = if s.pressed {
            0.12
        } else if s.active {
            0.08
        } else if s.hover {
            0.05
        } else {
            0.0
        };
        if alpha > 0.0 {
            rounded(cr, x, y, side, side, r);
            look.fg.set(cr, alpha);
            let _ = cr.fill();
        }
    }

    fn draw_tab(&self, cr: &cairo::Context, look: &Look) {
        let m = look.metrics;
        let (tx, ty, side) = (m.tile_x as f64, m.tile_y as f64, m.tile as f64);
        self.highlight(cr, look, tx, ty, side, m.radius as f64);

        // Icon, centred in the tile, crisp on HiDPI.
        let icon = m.icon as f64;
        let (ix, iy) = (tx + (side - icon) / 2.0, ty + (side - icon) / 2.0);
        let scale = self.area.scale_factor().max(1);
        let px = m.icon * scale;
        let scaled = {
            let mut s = self.state.borrow_mut();
            if s.scaled.as_ref().map(|(k, _)| *k) != Some(px) {
                s.scaled = s
                    .icon
                    .as_ref()
                    .and_then(|p| super::icons::fit(p, px))
                    .map(|p| (px, p));
            }
            s.scaled.as_ref().map(|(_, p)| p.clone())
        };
        match scaled {
            Some(p) => {
                let _ = cr.save();
                cr.translate(ix, iy);
                cr.scale(1.0 / scale as f64, 1.0 / scale as f64);
                cr.set_source_pixbuf(&p, 0.0, 0.0);
                let _ = cr.paint();
                let _ = cr.restore();
            }
            None => self.letter_icon(cr, ix, iy, icon),
        }

        let s = self.state.borrow();
        if s.active {
            let (mx, my, mw, mh) = m.marker();
            rounded(cr, mx as f64, my as f64, mw as f64, mh as f64, 1.5);
            look.accent().set(cr, 1.0);
            let _ = cr.fill();
        }
        // Unread bubble: its top-right corner (ring included) at
        // (tile.maxX + 6, tile.minY - 3).
        if !s.badge.is_empty() {
            let layout = text_layout(&self.area, &s.badge, 10.0, pango::Weight::Bold);
            let (tw, _) = layout.pixel_size();
            let (h, ring) = (16.0, 2.0);
            let w = (tw as f64 + 8.0).max(h);
            let right = tx + side + 6.0;
            let (x, y) = (right - ring - w, ty - 3.0 + ring);
            pill(
                cr,
                look,
                x,
                y,
                w,
                h,
                h / 2.0,
                ring,
                Rgb::hex(appearance::BADGE),
            );
            Rgb(1.0, 1.0, 1.0).set(cr, 1.0);
            centre_text(cr, &layout, x, y, w, h);
        }
        // Icon tag: bottom-left, flush with the tile.
        if !s.tag.is_empty() {
            let layout = text_layout(&self.area, &s.tag, 8.0, pango::Weight::Heavy);
            let (tw, _) = layout.pixel_size();
            let (h, ring) = (13.0, 1.5);
            let w = (tw as f64 + 6.0).max(h);
            let (x, y) = (tx + ring, ty + side - ring - h);
            pill(cr, look, x, y, w, h, 4.0, ring, look.fg);
            look.bar.set(cr, 1.0);
            centre_text(cr, &layout, x, y, w, h);
        }
    }

    /// No site icon yet: the name's initial on a coloured rounded square.
    fn letter_icon(&self, cr: &cairo::Context, x: f64, y: f64, size: f64) {
        let s = self.state.borrow();
        let hash = s
            .name
            .bytes()
            .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        rounded(cr, x, y, size, size, (size / 4.0).round());
        Rgb::hex(PALETTE[hash as usize % PALETTE.len()]).set(cr, 1.0);
        let _ = cr.fill();
        let letter: String = s
            .name
            .trim()
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "?".into());
        let layout = text_layout(&self.area, &letter, size * 0.55, pango::Weight::Bold);
        Rgb(1.0, 1.0, 1.0).set(cr, 1.0);
        centre_text(cr, &layout, x, y, size, size);
    }

    /// The + and settings tiles: 28×28, centred in the bar, a 14 px glyph.
    fn draw_glyph(&self, cr: &cairo::Context, look: &Look) {
        let m = look.metrics;
        let y = ((m.height - 28) / 2) as f64;
        self.highlight(cr, look, 0.0, y, 28.0, 8.0);
        // Secondary label colour.
        look.fg.set(cr, 0.62);
        cr.set_line_width(1.5);
        cr.set_line_cap(cairo::LineCap::Round);
        let (cx, cy) = (14.0, y + 14.0);
        match self.kind {
            Kind::Add => {
                cr.move_to(cx - 6.0, cy);
                cr.line_to(cx + 6.0, cy);
                cr.move_to(cx, cy - 6.0);
                cr.line_to(cx, cy + 6.0);
                let _ = cr.stroke();
            }
            _ => {
                // Three sliders (like SF Symbols' slider.horizontal.3).
                for (dy, knob) in [(-4.5, 3.0), (0.0, -2.5), (4.5, 1.0)] {
                    let ly = cy + dy;
                    cr.move_to(cx - 7.0, ly);
                    cr.line_to(cx + knob - 2.0, ly);
                    cr.move_to(cx + knob + 2.0, ly);
                    cr.line_to(cx + 7.0, ly);
                    let _ = cr.stroke();
                    cr.arc(cx + knob, ly, 1.8, 0.0, std::f64::consts::TAU);
                    let _ = cr.stroke();
                }
            }
        }
    }
}

fn text_layout(
    area: &gtk::DrawingArea,
    text: &str,
    px: f64,
    weight: pango::Weight,
) -> pango::Layout {
    let layout = area.create_pango_layout(Some(text));
    let mut font = area.pango_context().font_description().unwrap_or_default();
    font.set_absolute_size(px * pango::SCALE as f64);
    font.set_weight(weight);
    layout.set_font_description(Some(&font));
    layout
}

/// Draws `layout` centred in the box, optically (by ink extents).
fn centre_text(cr: &cairo::Context, layout: &pango::Layout, x: f64, y: f64, w: f64, h: f64) {
    let (ink, _) = layout.pixel_extents();
    cr.move_to(
        x + (w - ink.width() as f64) / 2.0 - ink.x() as f64,
        y + (h - ink.height() as f64) / 2.0 - ink.y() as f64,
    );
    pangocairo::functions::show_layout(cr, layout);
}

/// A pill with a ring in the bar's colour around it, so it reads as cut into
/// the icon.
#[allow(clippy::too_many_arguments)]
fn pill(
    cr: &cairo::Context,
    look: &Look,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    r: f64,
    ring: f64,
    fill: Rgb,
) {
    rounded(
        cr,
        x - ring,
        y - ring,
        w + 2.0 * ring,
        h + 2.0 * ring,
        r + ring,
    );
    look.bar.set(cr, 1.0);
    let _ = cr.fill();
    rounded(cr, x, y, w, h, r);
    fill.set(cr, 1.0);
    let _ = cr.fill();
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    use std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

/// Paints the bar background in the tint, or leaves the theme's.
pub fn paint_bar(bar: &gtk::Box, look: &Look) {
    thread_local! {
        static CSS: gtk::CssProvider = gtk::CssProvider::new();
    }
    CSS.with(|css| {
        let rule = if look.tinted {
            format!(".tabbar {{ background-color: {}; }}", look.bar.css())
        } else {
            String::new()
        };
        if let Err(e) = css.load_from_data(rule.as_bytes()) {
            eprintln!("webtabs: tint css: {e}");
        }
        // Added once; reloading the provider restyles the bar.
        if !bar.style_context().has_class("tint-ready") {
            bar.style_context().add_class("tint-ready");
            bar.style_context()
                .add_provider(css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
        }
    });
    bar.set_size_request(-1, look.metrics.height);
}

/// Whether the GTK theme is dark (its window background is), for accents
/// outside the bar.
pub fn theme_dark(widget: &impl IsA<gtk::Widget>) -> bool {
    widget
        .style_context()
        .lookup_color("theme_bg_color")
        .map(|c| Rgb::from_rgba(&c).luminance_dark())
        .unwrap_or(false)
}

/// The brand orange for the theme (sidebar dot, swatch ring).
pub fn accent_css(dark: bool) -> String {
    Rgb::hex(if dark {
        appearance::ACCENT_DARK
    } else {
        appearance::ACCENT_LIGHT
    })
    .css()
}

/// A site icon at `size` (the letter fallback when there's none), with the
/// optional tag pill, for lists outside the bar.
pub fn icon_area(
    icon: Option<&Pixbuf>,
    name: &str,
    tag: Option<&str>,
    size: i32,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_size_request(size + 3, size + 3);
    area.set_valign(gtk::Align::Center);
    let (icon, name, tag) = (icon.cloned(), name.to_string(), tag.map(String::from));
    area.connect_draw(move |a, cr| {
        let (x, y, s) = (3.0, 0.0, size as f64);
        let scale = a.scale_factor().max(1);
        match icon
            .as_ref()
            .and_then(|p| super::icons::fit(p, size * scale))
        {
            Some(p) => {
                let _ = cr.save();
                cr.translate(x, y);
                cr.scale(1.0 / scale as f64, 1.0 / scale as f64);
                cr.set_source_pixbuf(&p, 0.0, 0.0);
                let _ = cr.paint();
                let _ = cr.restore();
            }
            None => {
                let hash = name
                    .bytes()
                    .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
                rounded(cr, x, y, s, s, (s / 4.0).round());
                Rgb::hex(PALETTE[hash as usize % PALETTE.len()]).set(cr, 1.0);
                let _ = cr.fill();
                let letter: String = name
                    .trim()
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().collect())
                    .unwrap_or_else(|| "?".into());
                let da: &gtk::DrawingArea = a;
                let layout = text_layout(da, &letter, s * 0.55, pango::Weight::Bold);
                Rgb(1.0, 1.0, 1.0).set(cr, 1.0);
                centre_text(cr, &layout, x, y, s, s);
            }
        }
        if let Some(tag) = tag.as_deref() {
            let sc = a.style_context();
            let fg = Rgb::from_rgba(&sc.color(gtk::StateFlags::NORMAL));
            let bg = sc
                .lookup_color("theme_bg_color")
                .map(|c| Rgb::from_rgba(&c))
                .unwrap_or(Rgb(1.0, 1.0, 1.0));
            let da: &gtk::DrawingArea = a;
            let layout = text_layout(da, tag, 7.0, pango::Weight::Heavy);
            let (tw, _) = layout.pixel_size();
            let (h, ring) = (12.0, 1.5);
            let w = (tw as f64 + 5.0).max(h);
            let (px, py) = (x - 3.0 + ring, y + s + 3.0 - ring - h);
            rounded(
                cr,
                px - ring,
                py - ring,
                w + 2.0 * ring,
                h + 2.0 * ring,
                3.0 + ring,
            );
            bg.set(cr, 1.0);
            let _ = cr.fill();
            rounded(cr, px, py, w, h, 3.0);
            fg.set(cr, 1.0);
            let _ = cr.fill();
            bg.set(cr, 1.0);
            centre_text(cr, &layout, px, py, w, h);
        }
        glib::Propagation::Stop
    });
    area
}

/// A round colour swatch for the Appearance pane: `None` is the theme default
/// (half light, half dark), `Some(None)` the custom picker (a colour wheel
/// until a colour is picked).
pub fn swatch(
    color: Option<Option<u32>>,
    selected: Rc<Cell<bool>>,
    dark_theme: bool,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_size_request(30, 30);
    let accent = Rgb::hex(if dark_theme {
        appearance::ACCENT_DARK
    } else {
        appearance::ACCENT_LIGHT
    });
    area.connect_draw(move |a, cr| {
        use std::f64::consts::{PI, TAU};
        let (cx, cy, r) = (15.0, 15.0, 10.5);
        let fg = Rgb::from_rgba(&a.style_context().color(gtk::StateFlags::NORMAL));
        match color {
            None => {
                cr.arc(cx, cy, r, PI / 4.0, PI * 1.25);
                Rgb::hex(0xF2F2F4).set(cr, 1.0);
                let _ = cr.fill();
                cr.arc(cx, cy, r, PI * 1.25, PI * 2.25);
                Rgb::hex(0x2A2A2D).set(cr, 1.0);
                let _ = cr.fill();
            }
            Some(Some(rgb)) => {
                cr.arc(cx, cy, r, 0.0, TAU);
                Rgb::hex(rgb).set(cr, 1.0);
                let _ = cr.fill();
            }
            Some(None) => {
                let hues = [0xE5484D, 0xF0A000, 0x3FB950, 0x2F81F7, 0xA371F7];
                let step = TAU / hues.len() as f64;
                for (i, h) in hues.iter().enumerate() {
                    cr.move_to(cx, cy);
                    cr.arc(cx, cy, r, i as f64 * step, (i + 1) as f64 * step);
                    cr.close_path();
                    Rgb::hex(*h).set(cr, 1.0);
                    let _ = cr.fill();
                }
            }
        }
        // A hairline so pale swatches show on pale backgrounds.
        cr.arc(cx, cy, r, 0.0, TAU);
        fg.set(cr, 0.18);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        if selected.get() {
            cr.arc(cx, cy, r + 3.0, 0.0, TAU);
            accent.set(cr, 1.0);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        }
        glib::Propagation::Stop
    });
    area
}

/// The Settings preview: a rounded strip painted like the bar.
pub fn paint_preview(preview: &gtk::Box, look: &Look) {
    thread_local! {
        static CSS: gtk::CssProvider = gtk::CssProvider::new();
    }
    CSS.with(|css| {
        let rule = format!(".preview {{ background-color: {}; }}", look.bar.css());
        if let Err(e) = css.load_from_data(rule.as_bytes()) {
            eprintln!("webtabs: preview css: {e}");
        }
        // Each settings window has a new preview box; attach once per box.
        if !preview.style_context().has_class("painted") {
            preview.style_context().add_class("painted");
            preview
                .style_context()
                .add_provider(css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
        }
    });
    preview.set_size_request(300, look.metrics.height);
}
