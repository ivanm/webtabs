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
    "okta.com",
    "auth0.com",
    "onelogin.com",
    "duosecurity.com",
];

/// Sites that are only auth on specific paths ("Sign in with GitHub"), not the
/// whole host: a PR link shared in Slack must still open in the real browser.
const AUTH_PATHS: &[(&str, &[&str])] = &[("github.com", &["/login", "/session"])];

/// Schemes we hand to the desktop. Anything else (`file:`, custom app schemes a
/// page could use to launch local programs) is dropped.
const EXTERNAL_SCHEMES: &[&str] = &["http", "https", "mailto", "tel"];

pub fn is_safe_external(uri: &str) -> bool {
    Url::parse(uri).is_ok_and(|u| EXTERNAL_SCHEMES.contains(&u.scheme()))
}

/// Same registrable site, e.g. a Meet popup (meet.google.com) opened from a
/// Calendar tab (calendar.google.com). Used to scope camera/mic grants.
pub fn same_site(a: &str, b: &str) -> bool {
    let site_of = |u: &str| Url::parse(u).ok().and_then(|u| u.host_str().map(site));
    matches!((site_of(a), site_of(b)), (Some(x), Some(y)) if x == y)
}

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
    if is_auth_host(host) || is_auth_path(host, target.path()) {
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

fn is_auth_path(host: &str, path: &str) -> bool {
    AUTH_PATHS
        .iter()
        .any(|(h, paths)| host == *h && paths.iter().any(|p| path.starts_with(p)))
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
    fn github_only_auth_paths_stay_in_app() {
        let slack = "https://app.slack.com/client";
        assert_eq!(
            route(slack, "https://github.com/ivanm/webtabs/pull/1"),
            External
        );
        assert_eq!(
            route(slack, "https://github.com/login/oauth/authorize?x=1"),
            InApp
        );
        assert_eq!(
            route(slack, "https://github.com/sessions/two-factor"),
            InApp
        );
    }

    #[test]
    fn external_scheme_allowlist() {
        assert!(is_safe_external("https://example.com"));
        assert!(is_safe_external("mailto:a@b.com"));
        assert!(is_safe_external("tel:+123"));
        assert!(!is_safe_external("file:///etc/passwd"));
        assert!(!is_safe_external("audittest://pwn"));
        assert!(!is_safe_external("javascript:alert(1)"));
        assert!(!is_safe_external("not a url"));
    }

    #[test]
    fn same_site_matching() {
        assert!(same_site(
            "https://calendar.google.com/",
            "https://meet.google.com/abc"
        ));
        assert!(!same_site("https://app.slack.com/", "https://evil.com/"));
        assert!(!same_site("about:blank", "https://a.com"));
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
