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

/// True when the character before index `i` is alphanumeric (used to suppress
/// intra-word underscore emphasis like `snake_case`).
fn prev_is_alnum(chars: &[char], i: usize) -> bool {
    i > 0 && chars[i - 1].is_alphanumeric()
}

/// Index of the next occurrence of `target` at or after `from`.
fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&j| chars[j] == target)
}

/// Index of the first char of the next `delim delim` pair at or after `from`.
fn find_double(chars: &[char], from: usize, delim: char) -> Option<usize> {
    let mut j = from;
    while j + 1 < chars.len() {
        if chars[j] == delim && chars[j + 1] == delim {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// Parse `[text](url)` starting at `start` (which must point at `[`).
/// Returns (text, url, index-after-closing-paren).
fn parse_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let close_text = find_char(chars, start + 1, ']')?;
    if close_text + 1 >= chars.len() || chars[close_text + 1] != '(' {
        return None;
    }
    let close_url = find_char(chars, close_text + 2, ')')?;
    let text: String = chars[start + 1..close_text].iter().collect();
    let url: String = chars[close_text + 2..close_url].iter().collect();
    Some((text, url, close_url + 1))
}

/// Render an inline string (one logical line) to Telegram HTML.
fn render_inline(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];

        // Inline code: highest precedence, no formatting inside.
        if c == '`' {
            if let Some(close) = find_char(&chars, i + 1, '`') {
                let content: String = chars[i + 1..close].iter().collect();
                out.push_str("<code>");
                out.push_str(&escape(&content));
                out.push_str("</code>");
                i = close + 1;
                continue;
            }
        }

        // Link [text](url).
        if c == '[' {
            if let Some((text, url, next)) = parse_link(&chars, i) {
                out.push_str("<a href=\"");
                out.push_str(&escape_attr(&url));
                out.push_str("\">");
                out.push_str(&render_inline(&text));
                out.push_str("</a>");
                i = next;
                continue;
            }
        }

        // Bold: ** or __ (underscore form flank-guarded).
        if (c == '*' || c == '_') && i + 1 < chars.len() && chars[i + 1] == c {
            let guarded = c == '_' && prev_is_alnum(&chars, i);
            if !guarded {
                if let Some(close) = find_double(&chars, i + 2, c) {
                    if close > i + 2 {
                        let content: String = chars[i + 2..close].iter().collect();
                        out.push_str("<b>");
                        out.push_str(&render_inline(&content));
                        out.push_str("</b>");
                        i = close + 2;
                        continue;
                    }
                }
            }
        }

        // Strikethrough: ~~ ... ~~
        if c == '~' && i + 1 < chars.len() && chars[i + 1] == '~' {
            if let Some(close) = find_double(&chars, i + 2, '~') {
                if close > i + 2 {
                    let content: String = chars[i + 2..close].iter().collect();
                    out.push_str("<s>");
                    out.push_str(&render_inline(&content));
                    out.push_str("</s>");
                    i = close + 2;
                    continue;
                }
            }
        }

        // Italic: single * or _ (underscore form flank-guarded).
        if c == '*' || c == '_' {
            let guarded = c == '_' && prev_is_alnum(&chars, i);
            if !guarded {
                if let Some(close) = find_char(&chars, i + 1, c) {
                    if close > i + 1 {
                        let content: String = chars[i + 1..close].iter().collect();
                        out.push_str("<i>");
                        out.push_str(&render_inline(&content));
                        out.push_str("</i>");
                        i = close + 1;
                        continue;
                    }
                }
            }
        }

        // Default: escape literal text.
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// List marker kind.
enum Marker {
    Unordered,
    Ordered(u64),
}

/// True if the line opens or closes a fenced code block.
fn is_fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// If `line` opens a fence, return Some(language) (None = no language tag).
fn fence_lang(line: &str) -> Option<Option<String>> {
    let t = line.trim_start();
    t.strip_prefix("```").map(|rest| {
        let lang = rest.trim();
        if lang.is_empty() {
            None
        } else {
            Some(lang.to_string())
        }
    })
}

/// If `line` is an ATX heading, return (level, text).
fn heading(line: &str) -> Option<(usize, &str)> {
    let bytes = line.as_bytes();
    let mut level = 0;
    while level < bytes.len() && bytes[level] == b'#' {
        level += 1;
    }
    if (1..=6).contains(&level) && line[level..].starts_with(' ') {
        Some((level, line[level..].trim_start()))
    } else {
        None
    }
}

/// If `line` is a list item, return (indent_spaces, marker, text).
fn list_item(line: &str) -> Option<(usize, Marker, &str)> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(m) {
            return Some((indent, Marker::Unordered, rest));
        }
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        if let Some(rest) = t[digits.len()..].strip_prefix(". ") {
            if let Ok(n) = digits.parse::<u64>() {
                return Some((indent, Marker::Ordered(n), rest));
            }
        }
    }
    None
}

/// Convert markdown into a sequence of Telegram-HTML blocks (no trailing
/// newlines; one block per heading / paragraph line / list item / blockquote /
/// code fence / table).
fn render_blocks(md: &str) -> Vec<String> {
    let lines: Vec<&str> = md.lines().collect();
    let mut blocks: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // Fenced code block.
        if let Some(lang) = fence_lang(line) {
            let mut body = String::new();
            i += 1;
            while i < lines.len() && !is_fence(lines[i]) {
                body.push_str(lines[i]);
                body.push('\n');
                i += 1;
            }
            i += 1; // consume the closing fence (or run off the end)
            let mut block = String::from("<pre>");
            match lang {
                Some(l) => {
                    block.push_str(&format!("<code class=\"language-{}\">", escape(&l)));
                    block.push_str(&escape(&body));
                    block.push_str("</code>");
                }
                None => block.push_str(&escape(&body)),
            }
            block.push_str("</pre>");
            blocks.push(block);
            continue;
        }

        // Table detection is added in Task 4 (here, a pipe row falls through
        // to a paragraph).

        // Heading.
        if let Some((level, text)) = heading(line) {
            let inner = render_inline(text);
            if level <= 2 {
                blocks.push(format!("<u><b>{}</b></u>", inner));
            } else {
                blocks.push(format!("<b>{}</b>", inner));
            }
            i += 1;
            continue;
        }

        // Blockquote (group consecutive `>` lines).
        if line.starts_with('>') {
            let mut body = String::new();
            let mut first = true;
            while i < lines.len() && lines[i].starts_with('>') {
                let t = lines[i]
                    .strip_prefix("> ")
                    .or_else(|| lines[i].strip_prefix('>'))
                    .unwrap_or("");
                if !first {
                    body.push('\n');
                }
                body.push_str(&render_inline(t));
                first = false;
                i += 1;
            }
            blocks.push(format!("<blockquote>{}</blockquote>", body));
            continue;
        }

        // List item.
        if let Some((indent, marker, text)) = list_item(line) {
            let pad = " ".repeat(indent);
            let bullet = match marker {
                Marker::Unordered => "\u{2022} ".to_string(),
                Marker::Ordered(n) => format!("{}. ", n),
            };
            blocks.push(format!("{}{}{}", pad, bullet, render_inline(text)));
            i += 1;
            continue;
        }

        // Blank line: block separator, nothing emitted.
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        // Paragraph line.
        blocks.push(render_inline(line));
        i += 1;
    }
    blocks
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

    #[test]
    fn inline_bold_and_italic() {
        assert_eq!(render_inline("**b** and *i*"), "<b>b</b> and <i>i</i>");
        assert_eq!(render_inline("__b__ and _i_"), "<b>b</b> and <i>i</i>");
    }

    #[test]
    fn inline_code_is_not_formatted_inside() {
        assert_eq!(render_inline("use `a*b` now"), "use <code>a*b</code> now");
        assert_eq!(render_inline("`x < y`"), "<code>x &lt; y</code>");
    }

    #[test]
    fn inline_link() {
        assert_eq!(
            render_inline("see [docs](https://x.io/a?b=1&c=2)"),
            "see <a href=\"https://x.io/a?b=1&amp;c=2\">docs</a>"
        );
    }

    #[test]
    fn inline_strikethrough() {
        assert_eq!(render_inline("~~gone~~"), "<s>gone</s>");
    }

    #[test]
    fn inline_underscore_in_word_is_literal() {
        // snake_case must not become italic
        assert_eq!(render_inline("call foo_bar_baz now"), "call foo_bar_baz now");
    }

    #[test]
    fn inline_escapes_stray_html() {
        assert_eq!(render_inline("a < b & c"), "a &lt; b &amp; c");
    }

    #[test]
    fn inline_unclosed_delimiter_is_literal() {
        assert_eq!(render_inline("2 * 3 = 6"), "2 * 3 = 6");
    }

    #[test]
    fn block_heading_levels() {
        assert_eq!(render_blocks("# Title"), vec!["<u><b>Title</b></u>"]);
        assert_eq!(render_blocks("### Sub"), vec!["<b>Sub</b>"]);
    }

    #[test]
    fn block_unordered_and_ordered_lists() {
        assert_eq!(
            render_blocks("- one\n- two"),
            vec!["\u{2022} one", "\u{2022} two"]
        );
        assert_eq!(
            render_blocks("1. first\n2. second"),
            vec!["1. first", "2. second"]
        );
    }

    #[test]
    fn block_nested_list_keeps_indent() {
        assert_eq!(
            render_blocks("- a\n  - b"),
            vec!["\u{2022} a", "  \u{2022} b"]
        );
    }

    #[test]
    fn block_blockquote_groups_consecutive_lines() {
        assert_eq!(
            render_blocks("> a\n> b"),
            vec!["<blockquote>a\nb</blockquote>"]
        );
    }

    #[test]
    fn block_fenced_code_with_language() {
        let out = render_blocks("```rust\nlet x = 1 < 2;\n```");
        assert_eq!(
            out,
            vec!["<pre><code class=\"language-rust\">let x = 1 &lt; 2;\n</code></pre>"]
        );
    }

    #[test]
    fn block_fenced_code_without_language() {
        let out = render_blocks("```\nplain\n```");
        assert_eq!(out, vec!["<pre>plain\n</pre>"]);
    }

    #[test]
    fn block_paragraph_renders_inline() {
        assert_eq!(render_blocks("hello **world**"), vec!["hello <b>world</b>"]);
    }

    #[test]
    fn block_blank_lines_are_dropped() {
        assert_eq!(
            render_blocks("a\n\nb"),
            vec!["a", "b"]
        );
    }
}
