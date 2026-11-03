//! Draws the title bar: site icons with unread bubbles, tags and the active
//! marker, "+" and settings, and the caption buttons (the window has no system
//! title bar; see `mod.rs`'s WM_NCCALCSIZE). Geometry and colours come from
//! `crate::appearance`, shared with Linux and mirroring the Mac's TabBar.
//! GDI+ for anti-aliased shapes and icons, GDI (ClearType) for text, all into a
//! back buffer so resizing doesn't flicker.

use super::{App, Hit, wide};
use crate::appearance::{ACCENT_DARK, ACCENT_LIGHT, BADGE};
use crate::badge::Badge;
use ::windows::Win32::Foundation::{COLORREF, RECT, SIZE};
use ::windows::Win32::Graphics::Gdi::*;
use ::windows::Win32::Graphics::GdiPlus::*;
use ::windows::core::HSTRING;

/// Left padding before the first tab, in DIPs.
pub const PAD_X: i32 = 10;
/// The + and settings tiles, in DIPs.
pub const TOOL: i32 = 28;
/// Caption buttons (minimize, maximize, close), in DIPs: Windows 11's width.
pub const CAPTION_W: i32 = 46;

/// Windows 11's own title bar colours, used when no tint is set.
pub const BAR_LIGHT: u32 = 0xF3F3F3;
pub const BAR_DARK: u32 = 0x202020;
/// The window behind the pages (seen while a tab loads, and when empty).
const PAGE_LIGHT: u32 = 0xFFFFFF;
const PAGE_DARK: u32 = 0x1C1C1C;
const CLOSE_HOVER: u32 = 0xC42B1C;
const CLOSE_PRESSED: u32 = 0xC83C31;
/// Fallback icon colours, picked by tab name.
const PALETTE: &[u32] = &[
    0x2563eb, 0x16a34a, 0xd97706, 0xdc2626, 0x7c3aed, 0x0891b2, 0xdb2777,
];

/// What the bar is painted with: the tint (or Windows' default) and whether
/// text and glyphs on it are light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub bar: u32,
    pub light_text: bool,
    /// The system app theme (pages area, empty state).
    pub dark: bool,
}

impl Theme {
    fn label(&self) -> u32 {
        if self.light_text { 0xFFFFFF } else { 0x000000 }
    }
    fn text(&self) -> u32 {
        if self.light_text { 0xF3F3F3 } else { 0x1B1B1B }
    }
    fn secondary(&self) -> u32 {
        if self.light_text { 0xC8C8C8 } else { 0x5F5F5F }
    }
    fn accent(&self) -> u32 {
        if self.light_text {
            ACCENT_DARK
        } else {
            ACCENT_LIGHT
        }
    }
    pub fn page(&self) -> u32 {
        if self.dark { PAGE_DARK } else { PAGE_LIGHT }
    }
    /// The label colour at `alpha` over the bar, flattened (for GDI).
    fn over_bar(&self, alpha: f32) -> u32 {
        mix(self.bar, self.label(), alpha)
    }
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xff) as f32;
    let m = |s| ((ch(a, s) * (1.0 - t) + ch(b, s) * t).round() as u32) << s;
    m(16) | m(8) | m(0)
}

fn argb(rgb: u32, alpha: f32) -> u32 {
    (((alpha.clamp(0.0, 1.0) * 255.0).round() as u32) << 24) | (rgb & 0xffffff)
}

/// GDI colour from 0xRRGGBB.
fn rgb(c: u32) -> COLORREF {
    COLORREF(((c >> 16) & 0xff) | (c & 0xff00) | ((c & 0xff) << 16))
}

struct Gfx(*mut GpGraphics);

impl Gfx {
    fn new(hdc: HDC) -> Gfx {
        let mut g = std::ptr::null_mut();
        unsafe {
            GdipCreateFromHDC(hdc, &mut g);
            GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
            GdipSetInterpolationMode(g, InterpolationModeHighQualityBicubic);
            GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
        }
        Gfx(g)
    }

    /// A filled rounded rectangle in float pixels.
    fn rrect(&self, argb: u32, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let d = (radius * 2.0).min(w).min(h).max(0.5);
        unsafe {
            let mut brush = std::ptr::null_mut();
            GdipCreateSolidFill(argb, &mut brush);
            let mut path = std::ptr::null_mut();
            GdipCreatePath(FillModeAlternate, &mut path);
            GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
            GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
            GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
            GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
            GdipClosePathFigure(path);
            GdipFillPath(self.0, brush as *mut GpBrush, path);
            GdipDeletePath(path);
            GdipDeleteBrush(brush as *mut GpBrush);
        }
    }

    fn rect(&self, argb: u32, r: RECT, radius: f32) {
        self.rrect(
            argb,
            r.left as f32,
            r.top as f32,
            (r.right - r.left) as f32,
            (r.bottom - r.top) as f32,
            radius,
        );
    }

    fn image(&self, img: &super::icons::Image, r: RECT) {
        unsafe {
            GdipDrawImageRectI(
                self.0,
                img.as_image(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
            );
        }
    }
}

impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteGraphics(self.0);
        }
    }
}

struct Font(HFONT);

impl Font {
    fn new(px: i32, weight: i32, face: &str) -> Font {
        Font(unsafe {
            CreateFontW(
                -px,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                // Grayscale, like Windows' own caption glyphs (ClearType fringes on tints).
                ANTIALIASED_QUALITY,
                0,
                &HSTRING::from(face),
            )
        })
    }

    /// Windows 11's icon font, or Windows 10's (same code points).
    fn icons(hdc: HDC, px: i32) -> Font {
        let fluent = Font::new(px, 400, "Segoe Fluent Icons");
        let mut name = [0u16; 64];
        let len = unsafe {
            SelectObject(hdc, fluent.0.into());
            GetTextFaceW(hdc, Some(&mut name))
        };
        let face = String::from_utf16_lossy(&name[..(len.max(1) as usize - 1).min(63)]);
        if face.eq_ignore_ascii_case("Segoe Fluent Icons") {
            fluent
        } else {
            Font::new(px, 400, "Segoe MDL2 Assets")
        }
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

fn text(hdc: HDC, font: &Font, color: u32, s: &str, mut r: RECT, flags: DRAW_TEXT_FORMAT) {
    let mut buf = wide(s);
    unsafe {
        SelectObject(hdc, font.0.into());
        SetTextColor(hdc, rgb(color));
        DrawTextW(hdc, &mut buf, &mut r, flags);
    }
}

fn text_width(hdc: HDC, font: &Font, s: &str) -> i32 {
    let mut size = SIZE::default();
    unsafe {
        SelectObject(hdc, font.0.into());
        let _ = GetTextExtentPoint32W(hdc, &wide(s), &mut size);
    }
    size.cx
}

fn fill(hdc: HDC, r: RECT, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(rgb(color));
        FillRect(hdc, &r, brush);
        let _ = DeleteObject(brush.into());
    }
}

fn centered(cell: RECT, size: i32) -> RECT {
    let x = cell.left + (cell.right - cell.left - size) / 2;
    let y = cell.top + (cell.bottom - cell.top - size) / 2;
    RECT {
        left: x,
        top: y,
        right: x + size,
        bottom: y + size,
    }
}

const CENTER: DRAW_TEXT_FORMAT =
    DRAW_TEXT_FORMAT(DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0);

pub fn paint(app: &App) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(app.hwnd, &mut ps) };
    let client = app.client();
    let (w, h) = (client.right.max(1), client.bottom.max(1));
    unsafe {
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, bmp.into());
        SetBkMode(mem, TRANSPARENT);
        draw(app, mem, client);
        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        let _ = EndPaint(app.hwnd, &ps);
    }
}

/// Overlay strength for a tile's state, as on the Mac.
fn overlay(app: &App, hit: Hit, active: bool) -> f32 {
    if app.pressed.get() == Some(hit) && app.hover.get() == Some(hit) {
        0.12
    } else if active {
        0.08
    } else if app.hover.get() == Some(hit) {
        0.05
    } else {
        0.0
    }
}

fn draw(app: &App, hdc: HDC, client: RECT) {
    let px = |v: i32| app.px(v);
    let pxf = |v: f32| v * app.dpi() as f32 / 96.0;
    let theme = app.theme();
    let m = app.metrics();
    let bar_h = app.bar_height();
    fill(hdc, client, theme.page());
    let bar = RECT {
        bottom: bar_h,
        ..client
    };
    fill(hdc, bar, theme.bar);
    // A hairline under the bar, like Windows 11's.
    fill(
        hdc,
        RECT {
            top: bar_h - px(1).max(1),
            ..bar
        },
        theme.over_bar(0.08),
    );

    let views = app.all_views();
    let current = app.current.get();
    let icon_px = px(m.icon);
    let tile_r = pxf(m.radius as f32);
    let badge_font = Font::new(px(10), 700, "Segoe UI");
    let tag_font = Font::new(px(8), 800, "Segoe UI");
    let letter_font = Font::new(px(m.icon * 6 / 10), 600, "Segoe UI");
    let glyph_font = Font::icons(hdc, px(14));
    let caption_font = Font::icons(hdc, px(10));

    // Pass 1: tile highlights, icons, the active marker.
    let mut letters = Vec::new();
    {
        let g = Gfx::new(hdc);
        for (i, tv) in views.iter().enumerate() {
            let hit = Hit::Tab(i);
            let tile = app.cell_rect(hit);
            let a = overlay(app, hit, tv.id == current);
            if a > 0.0 {
                g.rect(argb(theme.label(), a), tile, tile_r);
            }
            let r = centered(tile, icon_px);
            match tv.icon.borrow().as_ref() {
                Some(img) => g.image(img, r),
                None => {
                    let name = tv.tab.borrow().name.clone();
                    let hash = name
                        .bytes()
                        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
                    g.rect(
                        argb(PALETTE[hash as usize % PALETTE.len()], 1.0),
                        r,
                        icon_px as f32 * 0.27,
                    );
                    let letter: String = name
                        .chars()
                        .next()
                        .map(|c| c.to_uppercase().collect())
                        .unwrap_or_default();
                    letters.push((letter, r));
                }
            }
            if tv.id == current {
                let (mx, my, mw, mh) = m.marker();
                let tab_left = tile.left - px(m.tile_x);
                let tab_top = tile.top - px(m.tile_y);
                g.rrect(
                    argb(theme.accent(), 1.0),
                    tab_left as f32 + pxf(mx as f32),
                    tab_top as f32 + pxf(my as f32),
                    pxf(mw as f32),
                    pxf(mh as f32),
                    pxf(1.5),
                );
            }
        }
        for hit in [Hit::Add, Hit::Gear] {
            let a = overlay(app, hit, false);
            if a > 0.0 {
                g.rect(argb(theme.label(), a), app.cell_rect(hit), pxf(6.0));
            }
        }
        for hit in [Hit::Minimize, Hit::Maximize, Hit::Close] {
            let hovered = app.hover.get() == Some(hit);
            let pressed = hovered && app.pressed.get() == Some(hit);
            let r = app.cell_rect(hit);
            if hit == Hit::Close && hovered {
                let c = if pressed { CLOSE_PRESSED } else { CLOSE_HOVER };
                g.rect(argb(c, 1.0), r, 0.0);
            } else if hovered {
                g.rect(
                    argb(theme.label(), if pressed { 0.04 } else { 0.06 }),
                    r,
                    0.0,
                );
            }
        }
    }

    // Pass 2: text on the icons, "+", settings and the caption glyphs.
    for (letter, r) in &letters {
        text(hdc, &letter_font, 0xffffff, letter, *r, CENTER);
    }
    text(
        hdc,
        &glyph_font,
        theme.secondary(),
        "\u{E710}",
        app.cell_rect(Hit::Add),
        CENTER,
    );
    text(
        hdc,
        &glyph_font,
        theme.secondary(),
        "\u{E713}",
        app.cell_rect(Hit::Gear),
        CENTER,
    );
    let zoomed = app.is_zoomed();
    let inactive = !app.active.get();
    for (hit, glyph) in [
        (Hit::Minimize, "\u{E921}"),
        (Hit::Maximize, if zoomed { "\u{E923}" } else { "\u{E922}" }),
        (Hit::Close, "\u{E8BB}"),
    ] {
        let color = if hit == Hit::Close && app.hover.get() == Some(Hit::Close) {
            0xFFFFFF
        } else if inactive {
            theme.secondary()
        } else {
            theme.text()
        };
        text(hdc, &caption_font, color, glyph, app.cell_rect(hit), CENTER);
    }

    // Pass 3: unread bubbles (top-right) and tags (bottom-left), each with a
    // ring in the bar's colour, then their text.
    let mut labels = Vec::new();
    {
        let g = Gfx::new(hdc);
        for (i, tv) in views.iter().enumerate() {
            let tile = app.cell_rect(Hit::Tab(i));
            let badge = tv.badge.get();
            if badge != Badge::None {
                let label = badge.label();
                let (h, ring) = (pxf(16.0), pxf(2.0));
                let w = ((text_width(hdc, &badge_font, &label) as f32) + pxf(8.0)).max(h);
                // Outer (ring) box: right = tile.maxX + 6, top = tile.minY - 3.
                let right = tile.right as f32 + pxf(6.0);
                let top = tile.top as f32 - pxf(3.0);
                let (ow, oh) = (w + 2.0 * ring, h + 2.0 * ring);
                g.rrect(argb(theme.bar, 1.0), right - ow, top, ow, oh, oh / 2.0);
                g.rrect(
                    argb(BADGE, 1.0),
                    right - ow + ring,
                    top + ring,
                    w,
                    h,
                    h / 2.0,
                );
                let r = RECT {
                    left: (right - ow + ring).round() as i32,
                    top: (top + ring).round() as i32,
                    right: (right - ring).round() as i32,
                    bottom: (top + ring + h).round() as i32,
                };
                labels.push((label, r, &badge_font, 0xFFFFFF));
            }
            if let Some(tag) = tv.tab.borrow().label.clone() {
                let (h, ring) = (pxf(13.0), pxf(1.5));
                let w = ((text_width(hdc, &tag_font, &tag) as f32) + pxf(6.0)).max(h);
                let (ow, oh) = (w + 2.0 * ring, h + 2.0 * ring);
                let left = tile.left as f32;
                let top = tile.bottom as f32 - oh;
                g.rrect(argb(theme.bar, 1.0), left, top, ow, oh, pxf(4.0) + ring);
                g.rrect(
                    argb(theme.label(), 1.0),
                    left + ring,
                    top + ring,
                    w,
                    h,
                    pxf(4.0),
                );
                let r = RECT {
                    left: (left + ring).round() as i32,
                    top: (top + ring).round() as i32,
                    right: (left + ring + w).round() as i32,
                    bottom: (top + ring + h).round() as i32,
                };
                labels.push((tag, r, &tag_font, theme.bar));
            }
        }
    }
    for (label, r, font, color) in labels {
        text(hdc, font, color, &label, r, CENTER);
    }

    if views.is_empty() {
        let title = Font::new(px(22), 600, "Segoe UI");
        let hint = Font::new(px(14), 400, "Segoe UI");
        let mid = (client.bottom + bar_h) / 2;
        let row = |top: i32, bottom: i32| RECT {
            top,
            bottom,
            ..client
        };
        let (fg, dim) = if theme.dark {
            (0xEEEEEE, 0xA3A3AA)
        } else {
            (0x222222, 0x666666)
        };
        text(
            hdc,
            &title,
            fg,
            "No tabs yet",
            row(mid - px(40), mid),
            CENTER,
        );
        text(
            hdc,
            &hint,
            dim,
            "Click + to add WhatsApp, Slack, Gmail\u{2026} or any web app",
            row(mid, mid + px(30)),
            CENTER,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_blends_channels() {
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(mix(0xF3F3F3, 0x000000, 0.0), 0xF3F3F3);
        assert_eq!(argb(0x123456, 1.0), 0xff123456);
    }
}
