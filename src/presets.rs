//! One-click services offered by the "Add tab" window.

pub struct Preset {
    pub name: &'static str,
    pub url: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "WhatsApp",
        url: "https://web.whatsapp.com/",
    },
    // Opens the last-used workspace; switch or add workspaces inside Slack.
    Preset {
        name: "Slack",
        url: "https://app.slack.com/client/",
    },
    Preset {
        name: "Telegram",
        url: "https://web.telegram.org/a/",
    },
    Preset {
        name: "Gmail",
        url: "https://mail.google.com/mail/u/0/",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Tab;

    #[test]
    fn presets_are_valid_tabs() {
        for p in PRESETS {
            let t = Tab::new(p.name, p.url, None);
            assert!(t.is_valid(), "{}", p.name);
            assert_eq!(t.url, p.url, "preset urls are already normalized");
        }
    }
}
