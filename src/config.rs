//! Tab configuration: a single human-editable TOML file.
//!
//! Location: `$WEBTABS_CONFIG` or `<config_dir>/webtabs/tabs.toml`
//! (`~/.config/webtabs/tabs.toml` on Linux,
//! `~/Library/Application Support/webtabs/tabs.toml` on macOS,
//! `%APPDATA%\webtabs\tabs.toml` on Windows).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::{fs, io};

/// Safari 26 UA (keep it current: Slack rejects old Safari versions). WebKit is the engine, so sites should ship us their WebKit code
/// path. A Linux WebKitGTK UA gets "unsupported browser" from WhatsApp/Slack/Meet.
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

/// Named user agents offered in the tab editor. "Default" means no override.
pub const USER_AGENT_PRESETS: &[(&str, &str)] = &[
    (
        "Chrome",
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36",
    ),
    (
        "Firefox",
        "Mozilla/5.0 (X11; Linux x86_64; rv:150.0) Gecko/20100101 Firefox/150.0",
    ),
];

const HEADER: &str = "\
# webtabs config. Edit freely; the app reloads this file when it changes.
#
# notifications   = true    # desktop notifications from your tabs
# tray            = true    # show the tray icon
# close_to_tray   = true    # closing the window keeps the app running in the tray
# unread_in_title = true    # total unread count in the window title
# unread_in_dock  = true    # unread count on the Dock / launcher icon
# gpu_acceleration = true   # Linux/Windows: draw pages on the GPU (restart to apply)
# icon_size       = 22      # tab icon size in points, 16-32
# tint            = \"sage\"  # title bar colour: graphite, sand, sage, sky, lavender,
#                           # rose or \"#rrggbb\" (default: the system's)
# user_agent      = \"...\"   # global user-agent override (default: Safari)
#
# [[tab]]
# name          = \"WhatsApp\"
# url           = \"https://web.whatsapp.com\"
# notifications = false     # optional: mute this tab
# label         = \"W\"       # optional 1-2 letter tag on the icon
# session       = \"slack-2\" # optional: isolated login storage (own cookies)
# user_agent    = \"...\"     # optional per-tab override

";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_true")]
    pub notifications: bool,
    #[serde(default = "default_true")]
    pub tray: bool,
    #[serde(default = "default_true")]
    pub close_to_tray: bool,
    #[serde(default = "default_true")]
    pub unread_in_title: bool,
    /// Unread count on the Dock icon (macOS) or launcher icon (Linux docks and
    /// panels that support `com.canonical.Unity.LauncherEntry`).
    #[serde(default = "default_true")]
    pub unread_in_dock: bool,
    /// Linux and Windows: render pages on the GPU. Off falls back to CPU painting, an escape
    /// hatch for driver trouble. Read once at startup. Written only when off, so
    /// the macOS bridge (which doesn't know it) never sees it.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub gpu_acceleration: bool,
    /// Tab icon size in points, within `ICON_SIZES`. Written only when changed.
    #[serde(
        default = "default_icon_size",
        skip_serializing_if = "is_default_icon_size"
    )]
    pub icon_size: u32,
    /// Title bar colour: one of `TINTS` or `#rrggbb`; `None` is the system's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, rename = "tab")]
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub name: String,
    pub url: String,
    /// `false` mutes this tab's desktop notifications (badges still show).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub notifications: bool,
    /// Short tag drawn on the icon, to tell e.g. two Slack tabs apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `Some(key)` gives the tab its own isolated storage (cookies, logins);
    /// `None` shares the default profile with the other tabs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

fn default_true() -> bool {
    true
}

pub const ICON_SIZE_DEFAULT: u32 = 22;
pub const ICON_SIZES: std::ops::RangeInclusive<u32> = 16..=32;
/// Title bar colours every front end offers by name (soft tints that keep text
/// and icons readable, each with a light and a dark variant of its own).
pub const TINTS: &[&str] = &["graphite", "sand", "sage", "sky", "lavender", "rose"];

fn default_icon_size() -> u32 {
    ICON_SIZE_DEFAULT
}

fn is_default_icon_size(size: &u32) -> bool {
    *size == ICON_SIZE_DEFAULT
}

/// A known tint name or `#rrggbb`, lowercased; anything else means the default.
pub fn normalize_tint(tint: &str) -> Option<String> {
    let tint = tint.trim().to_ascii_lowercase();
    let hex = tint.len() == 7
        && tint.starts_with('#')
        && tint[1..].bytes().all(|b| b.is_ascii_hexdigit());
    (hex || TINTS.contains(&tint.as_str())).then_some(tint)
}

fn is_true(b: &bool) -> bool {
    *b
}

impl Default for Config {
    fn default() -> Self {
        let tab = |name: &str, url: &str| Tab::new(name, url, None);
        Config {
            notifications: true,
            tray: true,
            close_to_tray: true,
            unread_in_title: true,
            unread_in_dock: true,
            gpu_acceleration: true,
            icon_size: ICON_SIZE_DEFAULT,
            tint: None,
            user_agent: None,
            tabs: vec![
                tab("WhatsApp", "https://web.whatsapp.com"),
                tab("Gmail", "https://mail.google.com"),
                tab("Calendar", "https://calendar.google.com"),
            ],
        }
    }
}

impl Tab {
    /// Builds a tab from loosely typed input: trims everything, adds `https://`
    /// when the scheme is missing, and falls back to the host as the name.
    pub fn new(name: &str, url: &str, user_agent: Option<&str>) -> Tab {
        let url = normalize_url(url);
        let name = match name.trim() {
            "" => url::Url::parse(&url)
                .ok()
                .and_then(|u| {
                    u.host_str()
                        .map(|h| h.trim_start_matches("www.").to_string())
                })
                .unwrap_or_else(|| url.clone()),
            n => n.to_string(),
        };
        let user_agent = user_agent
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        Tab {
            name,
            url,
            notifications: true,
            label: None,
            session: None,
            user_agent,
        }
    }

    pub fn with_label(mut self, label: &str) -> Tab {
        let label: String = label.trim().chars().take(2).collect();
        self.label = (!label.is_empty()).then_some(label);
        self
    }

    pub fn is_valid(&self) -> bool {
        url::Url::parse(&self.url)
            .map(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
            .unwrap_or(false)
    }
}

impl Config {
    pub fn user_agent_for<'a>(&'a self, tab: &'a Tab) -> &'a str {
        tab.user_agent
            .as_deref()
            .or(self.user_agent.as_deref())
            .unwrap_or(DEFAULT_USER_AGENT)
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let mut cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        // Appearance is cosmetic: out-of-range or unknown values fall back
        // instead of making the whole file invalid.
        cfg.icon_size = cfg.icon_size.clamp(*ICON_SIZES.start(), *ICON_SIZES.end());
        cfg.tint = cfg.tint.as_deref().and_then(normalize_tint);
        // Normalize hand-written entries the same way the edit dialog does.
        cfg.tabs = cfg
            .tabs
            .iter()
            .map(|t| Tab {
                notifications: t.notifications,
                session: t.session.as_deref().map(slug).filter(|s| !s.is_empty()),
                ..Tab::new(&t.name, &t.url, t.user_agent.as_deref())
                    .with_label(t.label.as_deref().unwrap_or(""))
            })
            .collect();
        if let Some(bad) = cfg.tabs.iter().find(|t| !t.is_valid()) {
            return Err(format!(
                "tab \"{}\" has an invalid url: {}",
                bad.name, bad.url
            ));
        }
        Ok(cfg)
    }

    pub fn to_toml(&self) -> String {
        format!(
            "{HEADER}{}",
            toml::to_string_pretty(self).expect("config always serializes")
        )
    }

    /// Loads the config, creating it with defaults on first run.
    pub fn load_or_create(path: &Path) -> Result<Config, String> {
        match fs::read_to_string(path) {
            Ok(text) => Config::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let cfg = Config::default();
                cfg.save(path)
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                Ok(cfg)
            }
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    /// Atomic write (temp file + rename) so the file watcher never sees half a file.
    /// A symlinked config (dotfiles) stays a symlink: we write through to its target.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let target = match fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => fs::canonicalize(path)?,
            _ => path.to_path_buf(),
        };
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = target.with_extension("toml.tmp");
        fs::write(&tmp, self.to_toml())?;
        fs::rename(&tmp, &target)
    }
}

/// A fresh isolated-session key for a tab named `name`: "slack", "slack-2", ...
pub fn new_session_key(name: &str, taken: &[&str]) -> String {
    let base = match slug(name) {
        s if s.is_empty() => "tab".to_string(),
        s => s,
    };
    (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|k| !taken.contains(&k.as_str()))
        .expect("unbounded range")
}

/// Lowercase ASCII letters/digits/dashes only; safe as a directory name.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

pub fn normalize_url(url: &str) -> String {
    let url = url.trim();
    if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

pub fn config_path() -> PathBuf {
    std::env::var_os("WEBTABS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("webtabs/tabs.toml")
        })
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("WEBTABS_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("webtabs")
        })
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("WEBTABS_DATA")
        .map(|d| PathBuf::from(d).join("cache"))
        .unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("webtabs")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_file_with_defaults() {
        let cfg = Config::parse("[[tab]]\nname = \"WA\"\nurl = \"web.whatsapp.com\"\n").unwrap();
        assert!(cfg.close_to_tray);
        assert_eq!(cfg.user_agent, None);
        assert_eq!(
            cfg.tabs,
            vec![Tab::new("WA", "https://web.whatsapp.com", None)]
        );
    }

    #[test]
    fn empty_file_is_valid_with_no_tabs() {
        assert_eq!(Config::parse("").unwrap().tabs, vec![]);
    }

    #[test]
    fn rejects_invalid_toml_and_urls() {
        assert!(Config::parse("[[tab]\n").is_err());
        assert!(Config::parse("[[tab]]\nname = \"x\"\n").is_err()); // missing url
        assert!(Config::parse("[[tab]]\nname = \"x\"\nurl = \"ftp://x.com\"\n").is_err());
        assert!(Config::parse("[[tab]]\nname = \"x\"\nurl = \"https://\"\n").is_err());
    }

    #[test]
    fn tab_new_normalizes_input() {
        let t = Tab::new("  ", " www.example.com/app ", Some("  "));
        assert_eq!(t.url, "https://www.example.com/app");
        assert_eq!(t.name, "example.com");
        assert_eq!(t.user_agent, None);
        assert_eq!(
            Tab::new("X", "http://a.b", Some(" UA "))
                .user_agent
                .as_deref(),
            Some("UA")
        );
    }

    #[test]
    fn options_default_on_and_mute_roundtrips() {
        let cfg = Config::parse(
            "tray = false\n[[tab]]\nname = \"a\"\nurl = \"a.com\"\nnotifications = false\n\
             [[tab]]\nname = \"b\"\nurl = \"b.com\"\n",
        )
        .unwrap();
        assert!(cfg.notifications && cfg.close_to_tray && cfg.unread_in_title);
        assert!(cfg.unread_in_dock);
        let off = Config::parse("unread_in_dock = false\n").unwrap();
        assert!(!off.unread_in_dock);
        assert_eq!(Config::parse(&off.to_toml()).unwrap(), off);
        assert!(cfg.gpu_acceleration);
        assert!(
            !cfg.to_toml()
                .lines()
                .any(|l| l.starts_with("gpu_acceleration"))
        );
        let cpu = Config::parse("gpu_acceleration = false\n").unwrap();
        assert!(!cpu.gpu_acceleration);
        assert_eq!(Config::parse(&cpu.to_toml()).unwrap(), cpu);
        assert!(!cfg.tray);
        assert!(!cfg.tabs[0].notifications);
        assert!(cfg.tabs[1].notifications);
        let text = cfg.to_toml();
        // Unmuted tabs stay terse; muted ones keep the flag.
        let muted = text
            .lines()
            .filter(|l| !l.starts_with('#') && l.contains("notifications = false"))
            .count();
        assert_eq!(muted, 1);
        assert_eq!(Config::parse(&text).unwrap(), cfg);
    }

    #[test]
    fn label_and_session_are_sanitized() {
        let cfg = Config::parse(
            "[[tab]]\nname = \"S\"\nurl = \"app.slack.com\"\nlabel = \" Work \"\n\
             session = \"../Evil Dir/\"\n",
        )
        .unwrap();
        assert_eq!(cfg.tabs[0].label.as_deref(), Some("Wo"));
        assert_eq!(cfg.tabs[0].session.as_deref(), Some("evil-dir"));
        assert_eq!(Config::parse(&cfg.to_toml()).unwrap(), cfg);
        let blank = Config::parse(
            "[[tab]]\nname = \"S\"\nurl = \"a.com\"\nlabel = \"\"\nsession = \"//\"\n",
        )
        .unwrap();
        assert_eq!(
            (blank.tabs[0].label.clone(), blank.tabs[0].session.clone()),
            (None, None)
        );
    }

    #[test]
    fn session_keys_are_unique_slugs() {
        assert_eq!(new_session_key("Slack", &[]), "slack");
        assert_eq!(new_session_key("Slack", &["slack"]), "slack-2");
        assert_eq!(new_session_key("Slack", &["slack", "slack-2"]), "slack-3");
        assert_eq!(new_session_key("Gmail (work)", &[]), "gmail-work");
        assert_eq!(new_session_key("  ", &[]), "tab");
        assert_eq!(new_session_key("Ñandú", &[]), "and");
    }

    #[test]
    fn appearance_is_normalized() {
        let cfg = Config::parse("").unwrap();
        assert_eq!((cfg.icon_size, cfg.tint.clone()), (22, None));
        let text = cfg.to_toml();
        assert!(
            !text
                .lines()
                .any(|l| l.starts_with("icon_size") || l.starts_with("tint"))
        );
        let cfg = Config::parse("icon_size = 40\ntint = \" Sage \"\n").unwrap();
        assert_eq!((cfg.icon_size, cfg.tint.as_deref()), (32, Some("sage")));
        assert_eq!(Config::parse(&cfg.to_toml()).unwrap(), cfg);
        assert_eq!(Config::parse("icon_size = 2").unwrap().icon_size, 16);
        assert_eq!(normalize_tint("#ABCdef").as_deref(), Some("#abcdef"));
        assert_eq!(normalize_tint("blue"), None);
        assert_eq!(normalize_tint("#abc"), None);
    }

    #[test]
    #[cfg(unix)]
    fn save_keeps_symlinks() {
        let dir = std::env::temp_dir().join(format!("webtabs-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("dotfiles")).unwrap();
        let real = dir.join("dotfiles/tabs.toml");
        let link = dir.join("tabs.toml");
        fs::write(&real, "").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        Config::default().save(&link).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            Config::parse(&fs::read_to_string(&real).unwrap()).unwrap(),
            Config::default()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn user_agent_precedence() {
        let mut cfg = Config::default();
        let plain = Tab::new("a", "a.com", None);
        let custom = Tab::new("b", "b.com", Some("tab-ua"));
        assert_eq!(cfg.user_agent_for(&plain), DEFAULT_USER_AGENT);
        cfg.user_agent = Some("global-ua".into());
        assert_eq!(cfg.user_agent_for(&plain), "global-ua");
        assert_eq!(cfg.user_agent_for(&custom), "tab-ua");
    }

    #[test]
    fn roundtrips_through_toml() {
        let mut cfg = Config {
            close_to_tray: false,
            ..Config::default()
        };
        cfg.tabs
            .push(Tab::new("Custom", "https://x.io", Some("ua")));
        let text = cfg.to_toml();
        assert!(text.starts_with("# webtabs config"));
        assert_eq!(Config::parse(&text).unwrap(), cfg);
    }

    #[test]
    fn load_or_create_writes_defaults_then_reads_them() {
        let dir = std::env::temp_dir().join(format!("webtabs-test-{}", std::process::id()));
        let path = dir.join("nested/tabs.toml");
        let _ = fs::remove_dir_all(&dir);
        let created = Config::load_or_create(&path).unwrap();
        assert_eq!(created, Config::default());
        assert!(path.exists());
        assert_eq!(Config::load_or_create(&path).unwrap(), created);
        fs::write(&path, "garbage = [").unwrap();
        assert!(Config::load_or_create(&path).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
