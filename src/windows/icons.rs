//! Site icons and drawing helpers: GDI+ decodes PNG/ICO/JPEG/GIF/BMP, icons are
//! downloaded on a worker thread and cached on disk via the shared
//! `favicon::cache_path`.

use crate::favicon::cache_path;
use ::windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use ::windows::Win32::Graphics::GdiPlus::*;
use ::windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use ::windows::Win32::System::Com::Urlmon::URLDownloadToFileW;
use ::windows::Win32::UI::Shell::SHCreateMemStream;
use ::windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use ::windows::core::HSTRING;
use std::path::Path;

/// Icons are tiny; anything bigger is not an icon (or is hostile).
const MAX_ICON_BYTES: usize = 1024 * 1024;

pub fn startup_gdiplus() {
    let input = GdiplusStartupInput {
        GdiplusVersion: 1,
        ..Default::default()
    };
    let mut token = 0usize;
    unsafe {
        GdiplusStartup(&mut token, &input, std::ptr::null_mut());
    }
}

/// A decoded bitmap owned by GDI+.
pub struct Image(*mut GpBitmap);

impl Drop for Image {
    fn drop(&mut self) {
        unsafe {
            GdipDisposeImage(self.0 as *mut GpImage);
        }
    }
}

impl Image {
    pub fn decode(bytes: &[u8]) -> Option<Image> {
        if bytes.is_empty() || bytes.len() > MAX_ICON_BYTES {
            return None;
        }
        let stream = unsafe { SHCreateMemStream(Some(bytes)) }?;
        let mut bmp: *mut GpBitmap = std::ptr::null_mut();
        let status = unsafe { GdipCreateBitmapFromStream(&stream, &mut bmp) };
        if status != Ok || bmp.is_null() {
            return None;
        }
        let img = Image(bmp);
        let mut w = 0u32;
        unsafe { GdipGetImageWidth(bmp as *mut GpImage, &mut w) };
        (w > 0 && w <= 2048).then_some(img)
    }

    pub fn as_image(&self) -> *mut GpImage {
        self.0 as *mut GpImage
    }
}

pub fn load_cached(page_url: &str) -> Option<(Image, Vec<u8>)> {
    let bytes = std::fs::read(cache_path(page_url)?).ok()?;
    Image::decode(&bytes).map(|i| (i, bytes))
}

pub fn store(page_url: &str, bytes: &[u8]) {
    let Some(path) = cache_path(page_url) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Result of a background icon download, posted back to the UI thread.
pub struct Fetched {
    pub generation: u64,
    /// Empty when no candidate decoded.
    pub bytes: Vec<u8>,
}

/// Tries each URL in order on a worker thread and posts the first one that
/// decodes to `hwnd` as `msg` (wparam = tab id, lparam = `Box<Fetched>`).
pub fn fetch(urls: Vec<String>, hwnd: HWND, msg: u32, tab_id: u64, generation: u64) {
    let hwnd_raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let tmp = std::env::temp_dir().join(format!(
            "webtabs-icon-{}-{tab_id}-{generation}",
            std::process::id()
        ));
        let mut bytes = Vec::new();
        for url in urls {
            if let Some(b) = download(&url, &tmp).filter(|b| Image::decode(b).is_some()) {
                bytes = b;
                break;
            }
        }
        let _ = std::fs::remove_file(&tmp);
        unsafe { CoUninitialize() };
        let boxed = Box::into_raw(Box::new(Fetched { generation, bytes }));
        let posted = unsafe {
            PostMessageW(
                Some(HWND(hwnd_raw as _)),
                msg,
                WPARAM(tab_id as usize),
                LPARAM(boxed as isize),
            )
        };
        if posted.is_err() {
            // The window is gone; nobody will free it.
            drop(unsafe { Box::from_raw(boxed) });
        }
    });
}

fn download(url: &str, tmp: &Path) -> Option<Vec<u8>> {
    if let Some(data) = url.strip_prefix("data:") {
        return decode_data_url(data);
    }
    // GDI+ has no SVG decoder; the next candidate is a bitmap.
    let path = url.split(['?', '#']).next().unwrap_or("");
    if path.ends_with(".svg") || !url.starts_with("http") {
        return None;
    }
    unsafe {
        URLDownloadToFileW(
            None,
            &HSTRING::from(url),
            &HSTRING::from(tmp.as_os_str()),
            0,
            None,
        )
        .ok()?;
    }
    std::fs::read(tmp).ok().filter(|b| b.len() <= MAX_ICON_BYTES)
}

/// `image/png;base64,...` (the part after `data:`). SVG is skipped.
fn decode_data_url(data: &str) -> Option<Vec<u8>> {
    let (meta, payload) = data.split_once(',')?;
    if !meta.ends_with(";base64") || meta.contains("svg") {
        return None;
    }
    base64_decode(payload)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (!out.is_empty() && out.len() <= MAX_ICON_BYTES).then_some(out)
}
