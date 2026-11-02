//! "Start with computer" on Windows: a value under the per-user Run key
//! (HKCU\Software\Microsoft\Windows\CurrentVersion\Run), the place Task
//! Manager's Startup tab lists and lets people turn off. The value itself is
//! the source of truth, so the switch and Task Manager always agree.

use ::windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use ::windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use ::windows::core::{PCWSTR, w};
use std::path::Path;

const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const NAME: PCWSTR = w!("webtabs");

/// The Run value: start hidden in the tray, like Linux's autostart entry.
pub fn command(exe: &Path) -> String {
    format!("\"{}\" --hidden", exe.display())
}

pub fn is_enabled() -> bool {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN,
            NAME,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    }
    .is_ok()
}

pub fn set(enabled: bool) -> Result<(), String> {
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut data: Vec<u16> = command(&exe).encode_utf16().collect();
        data.push(0);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN,
                NAME,
                REG_SZ.0,
                Some(data.as_ptr() as *const _),
                (data.len() * 2) as u32,
            )
        }
        .ok()
        .map_err(|e| e.message())
    } else {
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, NAME) };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        r.ok().map_err(|e| e.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_quotes_paths_with_spaces() {
        assert_eq!(
            command(Path::new(
                r"C:\Users\A B\AppData\Local\Programs\WebTabs\webtabs.exe"
            )),
            r#""C:\Users\A B\AppData\Local\Programs\WebTabs\webtabs.exe" --hidden"#
        );
    }
}
