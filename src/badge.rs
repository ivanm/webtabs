//! Unread-count detection from page titles, the convention every web chat/mail
//! app already follows: "(3) WhatsApp", "Inbox (12) - me@x.com - Gmail",
//! "* Slack" (unread, no count), "! Slack" (mention).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Badge {
    #[default]
    None,
    /// Unread activity without a number.
    Dot,
    Count(u32),
}

impl Badge {
    pub fn from_title(title: &str) -> Badge {
        let title = title.trim();
        if let Some(n) = first_bracketed_count(title) {
            return if n == 0 { Badge::None } else { Badge::Count(n) };
        }
        if title.starts_with('*') || title.starts_with('!') || title.starts_with('•') {
            return Badge::Dot;
        }
        Badge::None
    }

    pub fn count(self) -> u32 {
        match self {
            Badge::Count(n) => n,
            _ => 0,
        }
    }

    pub fn is_unread(self) -> bool {
        self != Badge::None
    }

    pub fn label(self) -> String {
        match self {
            Badge::None => String::new(),
            Badge::Dot => "•".into(),
            Badge::Count(n) if n > 99 => "99+".into(),
            Badge::Count(n) => n.to_string(),
        }
    }
}

/// Finds the first `(N)`, `(N+)` or `[N]` in the title.
fn first_bracketed_count(title: &str) -> Option<u32> {
    let bytes = title.as_bytes();
    for (i, &open) in bytes.iter().enumerate() {
        let close = match open {
            b'(' => b')',
            b'[' => b']',
            _ => continue,
        };
        let rest = &title[i + 1..];
        let end = rest.find(close as char)?;
        let inner = rest[..end].trim().trim_end_matches('+');
        if !inner.is_empty()
            && inner.len() <= 6
            && inner.bytes().all(|b| b.is_ascii_digit() || b == b',')
            && let Ok(n) = inner.replace(',', "").parse()
        {
            return Some(n);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_world_titles() {
        let cases = [
            ("(3) WhatsApp", Badge::Count(3)),
            ("WhatsApp", Badge::None),
            ("Inbox (12) - me@gmail.com - Gmail", Badge::Count(12)),
            ("Inbox - me@gmail.com - Gmail", Badge::None),
            ("Inbox (1,204) - me@gmail.com - Gmail", Badge::Count(1204)),
            ("(99+) Telegram", Badge::Count(99)),
            ("[5] Element", Badge::Count(5)),
            ("* general - Acme - Slack", Badge::Dot),
            ("! general - Acme - Slack", Badge::Dot),
            ("general - Acme - Slack", Badge::None),
            ("(0) Chat", Badge::None),
            ("Meeting (notes) (4)", Badge::Count(4)),
            ("Broken (12", Badge::None),
            ("", Badge::None),
        ];
        for (title, want) in cases {
            assert_eq!(Badge::from_title(title), want, "title: {title:?}");
        }
    }

    #[test]
    fn labels() {
        assert_eq!(Badge::None.label(), "");
        assert_eq!(Badge::Dot.label(), "•");
        assert_eq!(Badge::Count(7).label(), "7");
        assert_eq!(Badge::Count(150).label(), "99+");
        assert_eq!(Badge::Dot.count(), 0);
        assert!(Badge::Dot.is_unread() && !Badge::None.is_unread());
    }
}
