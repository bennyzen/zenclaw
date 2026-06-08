//! Convert LLM markdown into Telegram-flavored HTML chunks.
//!
//! Telegram's HTML subset has no `<table>`, no `<br>`, no headings, and no
//! native lists (see https://core.telegram.org/bots/api#html-style). We
//! downconvert the common LLM markdown subset and render tables as monospace
//! `<pre>` blocks (the accepted best, per telegramify-markdown). Output is
//! split into chunks no longer than `TELEGRAM_LIMIT` so no single sendMessage
//! exceeds Telegram's hard cap.

/// Telegram's maximum message length, in characters.
const TELEGRAM_LIMIT: usize = 4096;

/// Escape the three HTML-significant characters in text content.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a value that will appear inside an HTML attribute (also escapes `"`).
fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_replaces_html_significant_chars() {
        assert_eq!(escape("a < b & c > d"), "a &lt; b &amp; c &gt; d");
        assert_eq!(escape("plain"), "plain");
    }

    #[test]
    fn escape_attr_also_escapes_quotes() {
        assert_eq!(escape_attr(r#"a"b&c"#), "a&quot;b&amp;c");
    }
}
