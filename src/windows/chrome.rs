//! Window chrome helpers: the system light/dark setting, DWM's dark frame, the
//! resize border the custom title bar has to leave, and the taskbar button's
//! unread badge (ITaskbarList3 overlay icon).

use crate::badge::Badge;
use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use ::windows::Win32::Graphics::GdiPlus::*;
use ::windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use ::windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use ::windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use ::windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
use ::windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, HICON, SM_CXPADDEDBORDER, SM_CXSMICON, SM_CYFRAME,
};
use ::windows::core::BOOL;
use ::windows::core::{HSTRING, w};
use std::cell::{Cell, RefCell};

/// Apps use the dark theme (Settings › Personalization › Colors › app mode).
pub fn system_dark() -> bool {
    let mut value = 1u32;
    let mut size = 4u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    }
    .is_ok();
    ok && value == 0
}

/// Dark window frame and border (Windows 10 20H1+, 11) to match the theme.
pub fn set_dark_frame(hwnd: HWND, dark: bool) {
    let value = BOOL::from(dark);
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const BOOL as *const _,
            size_of::<BOOL>() as u32,
        );
    }
}

/// The sizing border's thickness at the window's DPI, which a maximized
/// window hangs off-screen and the top edge keeps for resizing.
pub fn frame_thickness(hwnd: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    unsafe {
        GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
}

/// The taskbar button's unread badge.
#[derive(Default)]
pub struct Taskbar {
    list: RefCell<Option<ITaskbarList3>>,
    icon: Cell<Option<isize>>,
    shown: Cell<Option<Badge>>,
}

impl Taskbar {
    /// Call on "TaskbarButtonCreated" (also after Explorer restarts).
    pub fn connect(&self) {
        let list: Option<ITaskbarList3> =
            unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }.ok();
        if let Some(l) = &list
            && unsafe { l.HrInit() }.is_err()
        {
            return;
        }
        *self.list.borrow_mut() = list;
        self.shown.set(None);
    }

    /// Shows `badge` on the button (`Badge::None` clears it).
    pub fn set(&self, hwnd: HWND, badge: Badge) {
        let Some(list) = self.list.borrow().clone() else {
            return;
        };
        if self.shown.get() == Some(badge) {
            return;
        }
        self.shown.set(Some(badge));
        let icon = if badge == Badge::None {
            None
        } else {
            let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
            let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
            badge_icon(&badge.label(), size)
        };
        let description = match badge {
            Badge::Count(n) => HSTRING::from(format!("{n} unread")),
            Badge::Dot => HSTRING::from("New activity"),
            Badge::None => HSTRING::new(),
        };
        unsafe {
            let _ = list.SetOverlayIcon(hwnd, icon.unwrap_or_default(), &description);
        }
        if let Some(old) = self.icon.replace(icon.map(|i| i.0 as isize)) {
            unsafe {
                let _ = DestroyIcon(HICON(old as *mut _));
            }
        }
    }
}

/// GDI+'s PixelFormat32bppARGB (missing from the bindings).
const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;

/// A red disc with the count (or a dot) as an icon of `size` pixels.
fn badge_icon(label: &str, size: i32) -> Option<HICON> {
    unsafe {
        let mut bmp: *mut GpBitmap = std::ptr::null_mut();
        if GdipCreateBitmapFromScan0(size, size, 0, PIXEL_FORMAT_32BPP_ARGB, None, &mut bmp) != Ok {
            return None;
        }
        let mut g: *mut GpGraphics = std::ptr::null_mut();
        GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g);
        GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        GdipSetTextRenderingHint(g, TextRenderingHintAntiAliasGridFit);
        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
        GdipCreateSolidFill(0xff000000 | crate::appearance::BADGE, &mut brush);
        let s = size as f32;
        let dot = label == "\u{2022}" || label.is_empty();
        if dot {
            let d = s * 0.6;
            GdipFillEllipse(g, brush as *mut GpBrush, s - d, 0.0, d, d);
        } else {
            GdipFillEllipse(g, brush as *mut GpBrush, 0.0, 0.0, s, s);
            let mut family: *mut GpFontFamily = std::ptr::null_mut();
            GdipCreateFontFamilyFromName(
                &HSTRING::from("Segoe UI"),
                std::ptr::null_mut(),
                &mut family,
            );
            let mut font: *mut GpFont = std::ptr::null_mut();
            let em = if label.chars().count() > 1 {
                s * 0.5
            } else {
                s * 0.66
            };
            GdipCreateFont(family, em, FontStyleBold.0, UnitPixel, &mut font);
            let mut format: *mut GpStringFormat = std::ptr::null_mut();
            GdipCreateStringFormat(0, 0, &mut format);
            GdipSetStringFormatAlign(format, StringAlignmentCenter);
            GdipSetStringFormatLineAlign(format, StringAlignmentCenter);
            let mut white: *mut GpSolidFill = std::ptr::null_mut();
            GdipCreateSolidFill(0xffffffff, &mut white);
            let text: Vec<u16> = label.encode_utf16().collect();
            let rect = RectF {
                X: 0.0,
                Y: 0.0,
                Width: s,
                Height: s,
            };
            GdipDrawString(
                g,
                ::windows::core::PCWSTR(text.as_ptr()),
                text.len() as i32,
                font,
                &rect,
                format,
                white as *mut GpBrush,
            );
            GdipDeleteBrush(white as *mut GpBrush);
            GdipDeleteStringFormat(format);
            GdipDeleteFont(font);
            GdipDeleteFontFamily(family);
        }
        GdipDeleteBrush(brush as *mut GpBrush);
        GdipDeleteGraphics(g);
        let mut icon = HICON::default();
        let status = GdipCreateHICONFromBitmap(bmp, &mut icon);
        GdipDisposeImage(bmp as *mut GpImage);
        (status == Ok).then_some(icon)
    }
}
