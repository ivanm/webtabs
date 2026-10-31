//! Draws the tab bar: site icons with unread bubbles and tags, "+" and the gear.
//! GDI+ for anti-aliased shapes and icons, GDI (ClearType) for text, all into a
//! back buffer so resizing doesn't flicker.

use super::{App, Hit, wide};
use crate::badge::Badge;
use ::windows::Win32::Foundation::{COLORREF, RECT, SIZE};
use ::windows::Win32::Graphics::Gdi::*;
use ::windows::Win32::Graphics::GdiPlus::*;
use ::windows::core::HSTRING;

/// Layout in DIPs (scaled with the window's DPI).
pub const BAR_H: i32 = 46;
pub const CELL: i32 = 40;
pub const CELL_Y: i32 = 3;
pub const PAD_X: i32 = 6;
pub const GAP: i32 = 4;
const ICON: i32 = 26;

const BAR_BG: u32 = 0xf3f3f3;
const BORDER: u32 = 0xe3e3e3;
const CONTENT_BG: u32 = 0xffffff;
const RED: u32 = 0xffe5484d;
const BLUE: u32 = 0xff3b82f6;
const ACTIVE: u32 = 0x24000000;
const HOVER: u32 = 0x12000000;
/// Fallback icon colours, picked by tab name.
const PALETTE: &[u32] = &[
    0xff2563eb, 0xff16a34a, 0xffd97706, 0xffdc2626, 0xff7c3aed, 0xff0891b2, 0xffdb2777,
];

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

    fn round_rect(&self, argb: u32, r: RECT, radius: i32) {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let d = (radius * 2).min(w).min(h).max(1);
        unsafe {
            let mut brush = std::ptr::null_mut();
            GdipCreateSolidFill(argb, &mut brush);
            let mut path = std::ptr::null_mut();
            GdipCreatePath(FillModeAlternate, &mut path);
            GdipAddPathArcI(path, r.left, r.top, d, d, 180.0, 90.0);
            GdipAddPathArcI(path, r.right - d, r.top, d, d, 270.0, 90.0);
            GdipAddPathArcI(path, r.right - d, r.bottom - d, d, d, 0.0, 90.0);
            GdipAddPathArcI(path, r.left, r.bottom - d, d, d, 90.0, 90.0);
            GdipClosePathFigure(path);
            GdipFillPath(self.0, brush as *mut GpBrush, path);
            GdipDeletePath(path);
            GdipDeleteBrush(brush as *mut GpBrush);
        }
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
                CLEARTYPE_QUALITY,
                0,
                &HSTRING::from(face),
            )
        })
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

const CENTER: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0);

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

fn draw(app: &App, hdc: HDC, client: RECT) {
    let px = |v| app.px(v);
    let bar_h = app.bar_height();
    fill(hdc, client, CONTENT_BG);
    fill(
        hdc,
        RECT {
            bottom: bar_h,
            ..client
        },
        BAR_BG,
    );
    fill(
        hdc,
        RECT {
            top: bar_h - 1,
            bottom: bar_h,
            ..client
        },
        BORDER,
    );

    let views = app.all_views();
    let current = app.current.get();
    let hover = app.hover.get();
    let icon_px = px(ICON);
    let badge_font = Font::new(px(11), 700, "Segoe UI");
    let tag_font = Font::new(px(9), 700, "Segoe UI");
    let letter_font = Font::new(px(15), 600, "Segoe UI");
    let glyph_font = Font::new(px(16), 400, "Segoe Fluent Icons");

    // Pass 1: highlights and icons.
    let mut letters = Vec::new();
    {
        let g = Gfx::new(hdc);
        for (i, tv) in views.iter().enumerate() {
            let cell = app.cell_rect(Hit::Tab(i));
            if tv.id == current {
                g.round_rect(ACTIVE, cell, px(10));
            } else if hover == Some(Hit::Tab(i)) {
                g.round_rect(HOVER, cell, px(10));
            }
            let r = centered(cell, icon_px);
            match tv.icon.borrow().as_ref() {
                Some(img) => g.image(img, r),
                None => {
                    let name = tv.tab.borrow().name.clone();
                    let hash = name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
                    g.round_rect(PALETTE[hash as usize % PALETTE.len()], r, px(7));
                    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                    letters.push((letter, r));
                }
            }
        }
        for hit in [Hit::Add, Hit::Gear] {
            if hover == Some(hit) {
                g.round_rect(HOVER, app.cell_rect(hit), px(10));
            }
        }
    }

    // Pass 2: text on the icons, "+" and the gear.
    for (letter, r) in &letters {
        text(hdc, &letter_font, 0xffffff, letter, *r, CENTER);
    }
    text(hdc, &glyph_font, 0x333333, "\u{E710}", app.cell_rect(Hit::Add), CENTER);
    text(hdc, &glyph_font, 0x333333, "\u{E713}", app.cell_rect(Hit::Gear), CENTER);

    // Pass 3: unread bubbles (top-right) and tags (bottom-left), then their text.
    let mut labels = Vec::new();
    {
        let g = Gfx::new(hdc);
        for (i, tv) in views.iter().enumerate() {
            let cell = app.cell_rect(Hit::Tab(i));
            let badge = tv.badge.get();
            if badge != Badge::None {
                let label = badge.label();
                let bh = px(16);
                let bw = (text_width(hdc, &badge_font, &label) + px(8)).max(bh);
                let r = RECT {
                    left: cell.right - bw + px(3),
                    top: cell.top,
                    right: cell.right + px(3),
                    bottom: cell.top + bh,
                };
                g.round_rect(RED, r, bh / 2);
                labels.push((label, r, &badge_font));
            }
            if let Some(tag) = tv.tab.borrow().label.clone() {
                let th = px(13);
                let tw = (text_width(hdc, &tag_font, &tag) + px(6)).max(th);
                let r = RECT {
                    left: cell.left + px(1),
                    top: cell.bottom - th,
                    right: cell.left + px(1) + tw,
                    bottom: cell.bottom,
                };
                g.round_rect(BLUE, r, px(4));
                labels.push((tag, r, &tag_font));
            }
        }
    }
    for (label, r, font) in labels {
        text(hdc, font, 0xffffff, &label, r, CENTER);
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
        text(hdc, &title, 0x222222, "No tabs yet", row(mid - px(40), mid), CENTER);
        text(
            hdc,
            &hint,
            0x666666,
            "Click + to add WhatsApp, Slack, Gmail\u{2026} or edit tabs.toml from the gear menu",
            row(mid, mid + px(30)),
            CENTER,
        );
    }
}
