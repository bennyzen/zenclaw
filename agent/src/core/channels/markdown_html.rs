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

const MAX_INLINE_DEPTH: usize = 32;

/// Max rendered grid width (chars) that still fits a phone's monospace <pre>
/// before Telegram wraps it. Wider tables switch to vertical records.
const MOBILE_GRID_MAX_WIDTH: usize = 34;

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

/// Find the `)` that closes a link URL opened at `from`, allowing balanced
/// nested parens inside the URL (e.g. Wikipedia `..._(programming)` links).
fn find_url_close(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = from;
    while j < chars.len() {
        match chars[j] {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return Some(j);
                }
                depth -= 1;
            }
            _ => {}
        }
        j += 1;
    }
    None
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
    let close_url = find_url_close(chars, close_text + 2)?;
    let text: String = chars[start + 1..close_text].iter().collect();
    let url: String = chars[close_text + 2..close_url].iter().collect();
    Some((text, url, close_url + 1))
}

/// Render an inline string (one logical line) to Telegram HTML.
fn render_inline(input: &str) -> String {
    render_inline_depth(input, 0)
}

fn render_inline_depth(input: &str, depth: usize) -> String {
    if depth >= MAX_INLINE_DEPTH {
        return escape(input);
    }
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
                out.push_str(&render_inline_depth(&text, depth + 1));
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
                        out.push_str(&render_inline_depth(&content, depth + 1));
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
                    out.push_str(&render_inline_depth(&content, depth + 1));
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
                        out.push_str(&render_inline_depth(&content, depth + 1));
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

/// A non-empty line containing at least one pipe is a candidate table row.
fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.contains('|')
}

/// A separator row is made only of `| - : space` and contains a dash.
fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Split a pipe row into trimmed cells, ignoring leading/trailing pipes.
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Right-pad each cell to its column width and join with " | ".
fn format_row(cells: &[String], widths: &[usize]) -> String {
    let padded: Vec<String> = widths
        .iter()
        .enumerate()
        .map(|(c, w)| {
            let cell = cells.get(c).map(|s| s.as_str()).unwrap_or("");
            let pad = w.saturating_sub(cell.chars().count());
            format!("{}{}", cell, " ".repeat(pad))
        })
        .collect();
    padded.join(" | ")
}

/// Render a table starting at `lines[0]` (header) + `lines[1]` (separator).
/// Narrow tables become an aligned monospace `<pre>` grid; tables too wide for
/// a phone become vertical records (first column as a bold title, the rest as
/// indented `Label: value` lines). Returns (html_block, lines_consumed).
fn render_table(lines: &[&str]) -> (String, usize) {
    let header = split_row(lines[0]);
    let ncols = header.len();
    let mut rows: Vec<Vec<String>> = vec![header];
    let mut consumed = 2; // header + separator
    let mut idx = 2;
    while idx < lines.len() && is_table_row(lines[idx]) && !is_table_separator(lines[idx]) {
        let mut cells = split_row(lines[idx]);
        cells.resize(ncols, String::new());
        rows.push(cells);
        consumed += 1;
        idx += 1;
    }

    let mut widths = vec![0usize; ncols];
    for row in &rows {
        for (c, cell) in row.iter().enumerate().take(ncols) {
            widths[c] = widths[c].max(cell.chars().count());
        }
    }
    let grid_width: usize = widths.iter().sum::<usize>() + 3 * ncols.saturating_sub(1);

    if rows.len() <= 1 || grid_width <= MOBILE_GRID_MAX_WIDTH {
        // Aligned monospace grid (fits a phone). Trailing padding is trimmed.
        let mut grid = String::new();
        for (ri, row) in rows.iter().enumerate() {
            if ri > 0 {
                grid.push('\n');
            }
            grid.push_str(format_row(row, &widths).trim_end());
        }
        (format!("<pre>{}</pre>", escape(&grid)), consumed)
    } else {
        // Too wide for a phone: vertical records.
        let header = &rows[0];
        let mut records: Vec<String> = Vec::new();
        for row in &rows[1..] {
            let mut out_lines: Vec<String> = Vec::new();
            let title = row.first().map(|s| s.as_str()).unwrap_or("");
            out_lines.push(format!("<b>{}</b>", escape(title)));
            for c in 1..ncols {
                let label = header.get(c).map(|s| s.as_str()).unwrap_or("");
                let value = row.get(c).map(|s| s.as_str()).unwrap_or("");
                out_lines.push(format!("  {}: {}", escape(label), escape(value)));
            }
            records.push(out_lines.join("\n"));
        }
        (records.join("\n\n"), consumed)
    }
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

        // GFM pipe table: a row followed by a separator row.
        if is_table_row(line)
            && i + 1 < lines.len()
            && is_table_separator(lines[i + 1])
        {
            let (html, consumed) = render_table(&lines[i..]);
            blocks.push(html);
            i += consumed;
            continue;
        }

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

/// One HTML token: an opening tag, a closing tag, or an atomic unit of text
/// (a single char, or a whole `&entity;` which must never be split).
enum HtmlTok {
    Open(String),
    Close(String),
    Atom(String),
}

/// Tokenize rendered HTML into tags and atomic text units. `&...;` entities
/// are kept whole so a split never lands inside an entity.
fn tokenize_html(s: &str) -> Vec<HtmlTok> {
    let chars: Vec<char> = s.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '<' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '>' {
                    j += 1;
                }
                let end = (j + 1).min(chars.len());
                let tag: String = chars[i..end].iter().collect();
                if tag.starts_with("</") {
                    toks.push(HtmlTok::Close(tag));
                } else {
                    toks.push(HtmlTok::Open(tag));
                }
                i = end;
            }
            '&' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != ';' && j - i <= 10 {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ';' {
                    toks.push(HtmlTok::Atom(chars[i..=j].iter().collect()));
                    i = j + 1;
                } else {
                    toks.push(HtmlTok::Atom(chars[i].to_string()));
                    i += 1;
                }
            }
            _ => {
                toks.push(HtmlTok::Atom(chars[i].to_string()));
                i += 1;
            }
        }
    }
    toks
}

/// The closing tag for an opening tag string (`<a href=..>` -> `</a>`).
fn close_for(open_tag: &str) -> String {
    let name: String = open_tag
        .trim_start_matches('<')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    format!("</{}>", name)
}

/// Split an oversize HTML block into pieces each <= `limit` chars AND each
/// independently well-formed: tags open at a split boundary are closed at the
/// end of the chunk and reopened at the start of the next. Keeps `<b>`/`<i>`/
/// `<a>` spans and `<pre><code class=..>` blocks valid within every chunk.
fn split_block(block: &str, limit: usize) -> Vec<String> {
    if block.chars().count() <= limit {
        return vec![block.to_string()];
    }
    let toks = tokenize_html(block);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    let mut open: Vec<String> = Vec::new();
    let mut progressed = false;

    for tok in toks {
        let tok_str = match &tok {
            HtmlTok::Open(s) | HtmlTok::Close(s) | HtmlTok::Atom(s) => s.clone(),
        };
        let tok_len = tok_str.chars().count();
        let close_reserve: usize =
            open.iter().map(|t| close_for(t).chars().count()).sum();
        let extra = if let HtmlTok::Open(s) = &tok {
            close_for(s).chars().count()
        } else {
            0
        };
        if progressed && cur_len + tok_len + close_reserve + extra > limit {
            let mut chunk = cur.clone();
            for t in open.iter().rev() {
                chunk.push_str(&close_for(t));
            }
            out.push(chunk);
            cur = open.concat();
            cur_len = cur.chars().count();
            progressed = false;
        }
        cur.push_str(&tok_str);
        cur_len += tok_len;
        progressed = true;
        match &tok {
            HtmlTok::Open(s) => open.push(s.clone()),
            HtmlTok::Close(_) => {
                open.pop();
            }
            HtmlTok::Atom(_) => {}
        }
    }
    if !cur.is_empty() {
        for t in open.iter().rev() {
            cur.push_str(&close_for(t));
        }
        out.push(cur);
    }
    out
}

/// Pack blocks (joined by single newlines) into chunks ≤ `limit`.
fn chunk_blocks(blocks: Vec<String>, limit: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for block in blocks {
        if block.chars().count() > limit {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            chunks.extend(split_block(&block, limit));
            continue;
        }
        let sep = if current.is_empty() { 0 } else { 1 };
        if !current.is_empty()
            && current.chars().count() + sep + block.chars().count() > limit
        {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(&block);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Convert LLM markdown into Telegram-HTML chunks, each ≤ `TELEGRAM_LIMIT`
/// characters. Returns an empty vec for empty/whitespace-only input.
pub fn render_telegram(md: &str) -> Vec<String> {
    chunk_blocks(render_blocks(md), TELEGRAM_LIMIT)
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

    #[test]
    fn table_renders_as_padded_pre() {
        let md = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 7 |";
        let out = render_blocks(md);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0],
            "<pre>Name  | Age\nAlice | 30\nBob   | 7</pre>"
        );
    }

    #[test]
    fn table_escapes_cell_html() {
        let md = "| A |\n| --- |\n| x<y |";
        let out = render_blocks(md);
        assert_eq!(out, vec!["<pre>A\nx&lt;y</pre>"]);
    }

    #[test]
    fn table_requires_separator_row() {
        // A lone pipe line with no separator is just a paragraph.
        let out = render_blocks("a | b | c");
        assert_eq!(out, vec!["a | b | c"]);
    }

    #[test]
    fn render_telegram_empty_input_yields_no_chunks() {
        assert!(render_telegram("").is_empty());
        assert!(render_telegram("   \n  ").is_empty());
    }

    #[test]
    fn render_telegram_single_chunk_joined_with_newlines() {
        assert_eq!(
            render_telegram("# Title\n\nbody **x**"),
            vec!["<u><b>Title</b></u>\nbody <b>x</b>"]
        );
    }

    #[test]
    fn render_telegram_splits_at_block_boundaries() {
        // 500 short paragraph blocks (~7 KB) force multiple chunks past 4096.
        let md: String = (0..500)
            .map(|n| format!("line number {}", n))
            .collect::<Vec<_>>()
            .join("\n\n");
        let chunks = render_telegram(&md);
        assert!(chunks.len() > 1, "expected multiple chunks");
        for c in &chunks {
            assert!(c.chars().count() <= 4096, "chunk too long: {}", c.chars().count());
        }
        // No block is split mid-line: every chunk starts with "line number".
        for c in &chunks {
            assert!(c.starts_with("line number"), "unexpected chunk start: {:?}", c);
        }
    }

    #[test]
    fn oversize_pre_block_is_split_with_reopened_tags() {
        // One code fence whose body alone exceeds the limit.
        let big_body: String = (0..1000)
            .map(|n| format!("row {}", n))
            .collect::<Vec<_>>()
            .join("\n");
        let md = format!("```\n{}\n```", big_body);
        let chunks = render_telegram(&md);
        assert!(chunks.len() > 1, "expected the pre block to be split");
        for c in &chunks {
            assert!(c.chars().count() <= 4096);
            assert!(c.starts_with("<pre>"), "chunk must reopen <pre>: {:?}", &c[..20.min(c.len())]);
            assert!(c.ends_with("</pre>"), "chunk must close </pre>");
        }
    }

    #[test]
    fn oversize_single_line_pre_is_hard_split() {
        let line = "x".repeat(10_000);
        let md = format!("```\n{}\n```", line);
        let chunks = render_telegram(&md);
        assert!(chunks.len() > 1, "expected multiple chunks");
        for c in &chunks {
            assert!(c.chars().count() <= 4096, "chunk too long: {}", c.chars().count());
            assert!(c.starts_with("<pre>") && c.ends_with("</pre>"), "chunk must be a valid pre block");
        }
    }

    #[test]
    fn oversize_bold_paragraph_splits_into_balanced_chunks() {
        let md = format!("**{}**", "a".repeat(5000));
        let chunks = render_telegram(&md);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() <= 4096, "chunk len {}", c.chars().count());
            assert_eq!(
                c.matches("<b>").count(),
                c.matches("</b>").count(),
                "unbalanced bold tags in chunk"
            );
            assert!(c.starts_with("<b>") && c.ends_with("</b>"));
        }
    }

    #[test]
    fn oversize_language_code_fence_keeps_code_tag_per_chunk() {
        let body = "x".repeat(10_000);
        let md = format!("```json\n{}\n```", body);
        let chunks = render_telegram(&md);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() <= 4096);
            assert!(
                c.starts_with("<pre><code class=\"language-json\">"),
                "chunk must reopen the code tag"
            );
            assert!(c.ends_with("</code></pre>"), "chunk must close code+pre");
        }
    }

    #[test]
    fn inline_link_with_parens_in_url() {
        assert_eq!(
            render_inline("[w](https://e.org/wiki/Rust_(programming))"),
            "<a href=\"https://e.org/wiki/Rust_(programming)\">w</a>"
        );
    }

    #[test]
    fn deeply_nested_emphasis_does_not_overflow() {
        let mut s = "x".to_string();
        for _ in 0..500 {
            s = format!("**{}**", s);
        }
        let out = render_inline(&s);
        assert!(!out.is_empty());
    }

    #[test]
    fn narrow_table_stays_compact_grid_without_separator() {
        let md = "| Qty | Item |\n| --- | --- |\n| 3 | Bolts |\n| 12 | Nuts |";
        let out = render_blocks(md);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], "<pre>Qty | Item\n3   | Bolts\n12  | Nuts</pre>");
    }

    #[test]
    fn wide_table_renders_as_vertical_records() {
        let md = "| Name | Role | Status |\n| --- | --- | --- |\n| Alice | Engineering Lead | On vacation |\n| Bob | Designer | Away |";
        let out = render_blocks(md);
        assert_eq!(out.len(), 1, "vertical table is one block");
        let block = &out[0];
        assert!(block.contains("<b>Alice</b>"), "missing Alice title: {block}");
        assert!(block.contains("  Role: Engineering Lead"), "missing role line: {block}");
        assert!(block.contains("  Status: On vacation"), "missing status line: {block}");
        assert!(block.contains("<b>Bob</b>"));
        assert!(!block.contains("<pre>"), "wide table must not use <pre>: {block}");
    }
}
