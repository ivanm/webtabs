//! Decides where a navigation goes: stay inside the app tab/popup, or open the
//! user's real browser. Keeps logins (OAuth popups, SSO redirects) in-app while
//! links people click in a chat or an email open externally.

use url::Url;

/// Identity providers that must stay in-app so the login completes in the tab's session.
const AUTH_HOSTS: &[&str] = &[
    "accounts.google.com",
    "accounts.youtube.com",
    "login.microsoftonline.com",
    "login.live.com",
    "appleid.apple.com",
    "github.com",
    "okta.com",
    "auth0.com",
    "onelogin.com",
    "duosecurity.com",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    InApp,
    External,
}

/// For new windows (`target=_blank`, `window.open`) and user-clicked links.
pub fn route(tab_url: &str, target: &str) -> Target {
    if target.is_empty() || target == "about:blank" {
        return Target::InApp; // script fills it in later; re-checked on its first navigation
    }
    let Ok(target) = Url::parse(target) else {
        return Target::External;
    };
    match target.scheme() {
        "http" | "https" => {}
        "blob" | "data" | "about" => return Target::InApp,
        _ => return Target::External, // mailto:, tel:, zoommtg:, slack:, ...
    }
    let Some(host) = target.host_str() else {
        return Target::External;
    };
    if is_auth_host(host) {
        return Target::InApp;
    }
    let tab_site = Url::parse(tab_url)
        .ok()
        .and_then(|u| u.host_str().map(site));
    if tab_site.as_deref() == Some(site(host).as_str()) {
        Target::InApp
    } else {
        Target::External
    }
}

fn is_auth_host(host: &str) -> bool {
    AUTH_HOSTS
        .iter()
        .any(|a| host == *a || host.ends_with(&format!(".{a}")))
}

/// Approximate registrable domain ("eTLD+1") without shipping the public suffix list:
/// `mail.google.com` -> `google.com`, `foo.bbc.co.uk` -> `bbc.co.uk`.
pub fn site(host: &str) -> String {
    let labels: Vec<&str> = host.trim_end_matches('.').split('.').collect();
    if labels.len() <= 2 || host.parse::<std::net::IpAddr>().is_ok() {
        return host.to_string();
    }
    let n = labels.len();
    // Two-letter ccTLD with a short second level (co.uk, com.br, co.jp) takes 3 labels.
    let take = if labels[n - 1].len() == 2 && labels[n - 2].len() <= 3 {
        3
    } else {
        2
    };
    labels[n - take.min(n)..].join(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use Target::*;

    #[test]
    fn site_extraction() {
        assert_eq!(site("mail.google.com"), "google.com");
        assert_eq!(site("google.com"), "google.com");
        assert_eq!(site("a.b.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(site("app.slack.com"), "slack.com");
        assert_eq!(site("web.whatsapp.com"), "whatsapp.com");
        assert_eq!(site("192.168.1.10"), "192.168.1.10");
        assert_eq!(site("localhost"), "localhost");
    }

    #[test]
    fn routing() {
        let gmail = "https://mail.google.com";
        assert_eq!(route(gmail, ""), InApp);
        assert_eq!(route(gmail, "about:blank"), InApp);
        assert_eq!(route(gmail, "https://calendar.google.com/x"), InApp);
        assert_eq!(
            route(
                gmail,
                "https://www.google.com/url?q=https://news.ycombinator.com"
            ),
            InApp
        );
        assert_eq!(route(gmail, "https://news.ycombinator.com"), External);
        assert_eq!(route(gmail, "mailto:a@b.com"), External);
        assert_eq!(route(gmail, "not a url"), External);
        assert_eq!(
            route(
                "https://app.slack.com",
                "https://login.microsoftonline.com/o"
            ),
            InApp
        );
        assert_eq!(
            route("https://app.slack.com", "https://acme.okta.com/sso"),
            InApp
        );
        assert_eq!(
            route("https://app.slack.com", "https://evilokta.com"),
            External
        );
        assert_eq!(
            route("https://web.whatsapp.com", "https://youtube.com/watch"),
            External
        );
        assert_eq!(
            route(
                "https://web.whatsapp.com",
                "blob:https://web.whatsapp.com/1"
            ),
            InApp
        );
    }
}
