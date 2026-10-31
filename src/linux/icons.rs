//! Site icons: download the best declared icon, cache it on disk, and render it
//! crisp at any HiDPI scale.

use crate::config::{DEFAULT_USER_AGENT, data_dir};
use gtk::gdk::prelude::GdkPixbufExt;
use gtk::gdk_pixbuf::{InterpType, Pixbuf, PixbufLoader};
use gtk::prelude::*;
use gtk::{gio, glib};
use soup::prelude::*;
use std::path::PathBuf;

/// Cached icons are stored at this size; tabs draw them at 32px (64px on HiDPI).
const CACHE_PX: i32 = 128;
/// Icons are tiny; anything bigger is not an icon (or is hostile).
const MAX_ICON_BYTES: usize = 1024 * 1024;
const MAX_ICON_SIDE: i32 = 1024;

thread_local! {
    static SESSION: soup::Session = {
        let s = soup::Session::new();
        s.set_user_agent(DEFAULT_USER_AGENT);
        s.set_timeout(15);
        s
    };
}

/// Tries each URL in order and hands the first one that decodes to `done`.
pub fn fetch(urls: Vec<String>, done: impl FnOnce(Option<Pixbuf>) + 'static) {
    fetch_next(urls.into_iter(), Box::new(done));
}

fn fetch_next(mut urls: std::vec::IntoIter<String>, done: Box<dyn FnOnce(Option<Pixbuf>)>) {
    let Some(url) = urls.next() else {
        return done(None);
    };
    if let Some(data) = url.strip_prefix("data:") {
        return match decode_data_url(data)
            .filter(|b| b.len() <= MAX_ICON_BYTES)
            .and_then(|b| decode(&b))
        {
            Some(p) => done(Some(p)),
            None => fetch_next(urls, done),
        };
    }
    let Ok(msg) = soup::Message::new("GET", &url) else {
        return fetch_next(urls, done);
    };
    let msg2 = msg.clone();
    SESSION.with(|s| {
        s.send_and_read_async(
            &msg,
            glib::Priority::LOW,
            None::<&gio::Cancellable>,
            move |res| {
                let ok = msg2.status() == soup::Status::Ok;
                let body = res.ok().filter(|b| ok && b.len() <= MAX_ICON_BYTES);
                match body.and_then(|b| decode(&b)) {
                    Some(p) => done(Some(p)),
                    None => fetch_next(urls, done),
                }
            },
        )
    });
}

fn decode_data_url(data: &str) -> Option<Vec<u8>> {
    let (meta, payload) = data.split_once(',')?;
    if meta.ends_with(";base64") {
        glib::base64_decode(payload).into()
    } else {
        Some(
            glib::Uri::unescape_string(payload, None::<&str>)?
                .as_bytes()
                .to_vec(),
        )
    }
}

/// Decodes PNG/ICO/SVG/... SVGs are rasterized at cache size instead of their
/// (often tiny) intrinsic size so they stay sharp.
pub fn decode(bytes: &[u8]) -> Option<Pixbuf> {
    let loader = PixbufLoader::new();
    loader.connect_size_prepared(|l, w, h| {
        if w <= 0 || h <= 0 {
            return;
        }
        let is_svg = l
            .format()
            .and_then(|f| f.name())
            .is_some_and(|n| n.contains("svg"));
        let side = w.max(h);
        // SVGs render at cache size (sharp); huge declared sizes are scaled
        // down before decoding so a hostile image can't allocate gigabytes.
        if (is_svg && side < CACHE_PX) || side > MAX_ICON_SIDE {
            let k = CACHE_PX as f64 / side as f64;
            l.set_size(
                ((w as f64 * k) as i32).max(1),
                ((h as f64 * k) as i32).max(1),
            );
        }
    });
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    loader.pixbuf().filter(|p| p.width() > 0 && p.height() > 0)
}

/// Fits `p` into a transparent `size`×`size` square, keeping aspect ratio.
pub fn fit(p: &Pixbuf, size: i32) -> Option<Pixbuf> {
    let k = size as f64 / p.width().max(p.height()) as f64;
    let (w, h) = (
        ((p.width() as f64 * k).round() as i32).max(1),
        ((p.height() as f64 * k).round() as i32).max(1),
    );
    let scaled = p.scale_simple(w, h, InterpType::Hyper)?;
    let canvas = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, size, size)?;
    canvas.fill(0);
    scaled.copy_area(0, 0, w, h, &canvas, (size - w) / 2, (size - h) / 2);
    Some(canvas)
}

/// Shows `p` at `px` logical pixels, using device pixels on HiDPI screens.
pub fn show(image: &gtk::Image, p: &Pixbuf, px: i32) {
    let scale = image.scale_factor().max(1);
    if let Some(fitted) = fit(p, px * scale) {
        let surface = fitted.create_surface(scale, image.window().as_ref());
        image.set_from_surface(surface.as_ref());
    }
}

/// One file per tab URL (two tabs on the same host can have different icons).
fn cache_path(page_url: &str) -> Option<PathBuf> {
    let u = url::Url::parse(page_url).ok()?;
    let key: String = format!("{}{}", u.host_str()?, u.path())
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(
        data_dir()
            .join("favicons")
            .join(format!("{}.png", key.trim_end_matches('_'))),
    )
}

pub fn load_cached(page_url: &str) -> Option<Pixbuf> {
    Pixbuf::from_file(cache_path(page_url)?).ok()
}

pub fn store(page_url: &str, p: &Pixbuf) {
    let (Some(path), Some(fitted)) = (cache_path(page_url), fit(p, CACHE_PX)) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = fitted.savev(&path, "png", &[]) {
        eprintln!("webtabs: cannot cache icon {}: {e}", path.display());
    }
}
