//! "Start with computer": an XDG autostart entry that launches hidden in the tray.
//! The file itself is the source of truth, so `make autostart` and the settings
//! switch always agree.

use std::path::{Path, PathBuf};
use std::{fs, io};

pub fn entry_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("autostart/webtabs.desktop")
}

pub fn is_enabled(entry: &Path) -> bool {
    entry.exists()
}

pub fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=WebTabs\nComment=Your web apps in one tiny window\n\
         Exec=\"{}\" --hidden\nIcon=webtabs\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    )
}

pub fn set(entry: &Path, enabled: bool, exe: &Path) -> io::Result<()> {
    if enabled {
        if let Some(dir) = entry.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(entry, desktop_entry(exe))
    } else {
        match fs::remove_file(entry) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_disable_roundtrip() {
        let dir = std::env::temp_dir().join(format!("webtabs-autostart-{}", std::process::id()));
        let entry = dir.join("autostart/webtabs.desktop");
        let _ = fs::remove_dir_all(&dir);
        assert!(!is_enabled(&entry));
        set(&entry, true, Path::new("/opt/web tabs/webtabs")).unwrap();
        assert!(is_enabled(&entry));
        let text = fs::read_to_string(&entry).unwrap();
        assert!(text.contains("Exec=\"/opt/web tabs/webtabs\" --hidden"));
        set(&entry, false, Path::new("x")).unwrap();
        assert!(!is_enabled(&entry));
        set(&entry, false, Path::new("x")).unwrap(); // idempotent
        fs::remove_dir_all(&dir).unwrap();
    }
}
