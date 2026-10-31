//! Picks the best icon for a tab from the `<link>` tags a page declares.
//! Platform front ends collect the links (via JS) and download the winner.

use serde::Deserialize;
use url::Url;

/// Runs in the page; returns a JSON array of the declared icons.
pub const COLLECT_LINKS_JS: &str = r#"JSON.stringify(Array.from(document.querySelectorAll(
  'link[rel~="icon"], link[rel~="apple-touch-icon"], link[rel~="apple-touch-icon-precomposed"]'
)).map(l => ({ href: l.href, rel: l.rel, sizes: l.getAttribute('sizes') || '', type: l.type || '' })))"#;

#[derive(Debug, Deserialize)]
struct Link {
    href: String,
    #[serde(default)]
    rel: String,
    #[serde(default)]
    sizes: String,
    #[serde(default, rename = "type")]
    mime: String,
}

/// Icon URLs best-first: vector icons, then the largest declared bitmap,
/// then `/favicon.ico` as the last resort.
pub fn candidates(page_url: &str, links_json: &str) -> Vec<String> {
    let links: Vec<Link> = serde_json::from_str(links_json).unwrap_or_default();
    let mut scored: Vec<(u32, String)> = links
        .into_iter()
        .filter(|l| l.href.starts_with("http") || l.href.starts_with("data:image/"))
        .map(|l| (score(&l), l.href))
        .collect();
    // Stable sort keeps document order between equally good icons.
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    let mut out: Vec<String> = Vec::new();
    for (_, href) in scored {
        if !out.contains(&href) {
            out.push(href);
        }
    }
    if let Ok(fallback) = Url::parse(page_url).and_then(|u| u.join("/favicon.ico")) {
        let fallback = fallback.to_string();
        if !out.contains(&fallback) {
            out.push(fallback);
        }
    }
    out
}

fn score(l: &Link) -> u32 {
    let path = l.href.split(['?', '#']).next().unwrap_or("");
    if l.mime.contains("svg") || path.ends_with(".svg") || l.sizes.eq_ignore_ascii_case("any") {
        return 1000;
    }
    let declared = l
        .sizes
        .split_whitespace()
        .filter_map(|s| {
            s.to_ascii_lowercase()
                .split_once('x')
                .and_then(|(w, _)| w.parse::<u32>().ok())
        })
        .max();
    match declared {
        Some(px) => px.min(999),
        None if l.rel.contains("apple-touch-icon") => 180,
        None => 16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_vector_then_largest_then_fallback() {
        let json = r#"[
            {"href":"https://x.com/fav16.png","rel":"icon","sizes":"16x16","type":"image/png"},
            {"href":"https://x.com/touch.png","rel":"apple-touch-icon","sizes":"","type":""},
            {"href":"https://x.com/fav96.png","rel":"icon","sizes":"32x32 96x96","type":""},
            {"href":"https://x.com/logo.svg?v=2","rel":"icon","sizes":"","type":""},
            {"href":"https://x.com/plain.ico","rel":"shortcut icon","sizes":"","type":""},
            {"href":"https://x.com/fav16.png","rel":"icon","sizes":"16x16","type":""},
            {"href":"javascript:alert(1)","rel":"icon","sizes":"","type":""}
        ]"#;
        assert_eq!(
            candidates("https://x.com/app/inbox", json),
            vec![
                "https://x.com/logo.svg?v=2",
                "https://x.com/touch.png",
                "https://x.com/fav96.png",
                "https://x.com/fav16.png",
                "https://x.com/plain.ico",
                "https://x.com/favicon.ico",
            ]
        );
    }

    #[test]
    fn garbage_still_yields_site_favicon() {
        assert_eq!(
            candidates("https://web.whatsapp.com/", "not json"),
            vec!["https://web.whatsapp.com/favicon.ico"]
        );
        assert_eq!(
            candidates("https://a.b/", "[]"),
            vec!["https://a.b/favicon.ico"]
        );
        assert!(candidates("not a url", "[]").is_empty());
    }
}
