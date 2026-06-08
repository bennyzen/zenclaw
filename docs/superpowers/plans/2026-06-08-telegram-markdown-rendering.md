# Telegram Markdown Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render LLM markdown as Telegram-flavored HTML (bold/italic/code/lists/blockquote/links + monospace tables), chunked to ≤4096 chars, with an automatic plain-text fallback so formatting can never lose a message.

**Architecture:** A new pure-string module `core/channels/markdown_html.rs` converts markdown → Telegram HTML and splits it into size-bounded chunks. `TelegramChannel::deliver` routes every send through it, posts each chunk with `parse_mode=HTML`, and on a Telegram **400** (malformed entities) re-sends that chunk as stripped plain text. All logic is host-testable on the `desktop` feature; no hardware needed.

**Tech Stack:** Rust (std, shared esp32 + desktop), `serde_json`, existing `HttpClient`/`Channel` abstractions, `MockHttpClient` test harness.

**Conventions:**
- All commands run from the `agent/` directory.
- Host test command (default feature is `esp32`, which can't build on host): `cargo test --no-default-features --features desktop <filter>`
- Branch: `feat/telegram-markdown-rendering` (already created).
- The converter module's tests are pure and sync — gate with `#[cfg(test)]` + `#[test]`. The `telegram.rs` delivery tests stay `#[cfg(all(test, feature = "desktop"))]` + `#[tokio::test]`.

**Spec:** `docs/superpowers/specs/2026-06-08-telegram-markdown-rendering-design.md`

**Refinement vs. spec:** The spec says "retry as plain text on any 4xx." We narrow this to **status 400** only. 400 is Telegram's "Bad Request" for malformed entities (the formatting failure we guard against); 403 (blocked) / 429 (rate limit) are not formatting problems and should propagate. This also preserves the existing `channel_deliver_non_200_errors` (403) test.

---

## File Structure

- **Create:** `agent/src/core/channels/markdown_html.rs` — markdown→Telegram-HTML converter + chunker. Single public fn `render_telegram(&str) -> Vec<String>`. All helpers private. Self-contained tests.
- **Modify:** `agent/src/core/channels/mod.rs` — add `pub mod markdown_html;`.
- **Modify:** `agent/src/core/channels/telegram.rs` — route `deliver` through the converter; add `post_send` + `send_one` helpers + `strip_tags`; update doc comment; replace two parse_mode tests; add fallback/chunking tests.

No changes needed at the construction sites (`main.rs:316`, `desktop/run.rs:190`): HTML mode is now the unconditional behavior of `deliver`, so they keep calling `TelegramChannel::new(...)` unchanged.

---

## Task 1: Module scaffold + HTML escape

**Files:**
- Create: `agent/src/core/channels/markdown_html.rs`
- Modify: `agent/src/core/channels/mod.rs:28` (add module declaration next to `pub mod telegram;`)

- [ ] **Step 1: Create the module with the escape fn and a test**

Create `agent/src/core/channels/markdown_html.rs`:

```rust
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

/// Escape a value destined for an HTML attribute (adds `"`).
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
```

- [ ] **Step 2: Declare the module**

In `agent/src/core/channels/mod.rs`, immediately after the line `pub mod telegram;` (line 28), add:

```rust
pub mod markdown_html;
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test --no-default-features --features desktop markdown_html`
Expected: PASS (2 tests: `escape_replaces_html_significant_chars`, `escape_attr_also_escapes_quotes`).

- [ ] **Step 4: Commit**

```bash
git add agent/src/core/channels/markdown_html.rs agent/src/core/channels/mod.rs
git commit -m "feat(telegram): scaffold markdown_html module with HTML escaping"
```

---

## Task 2: Inline rendering

Renders inline spans: code, links, bold, italic, strikethrough. Underscore emphasis is flank-guarded so `snake_case` is not mangled.

**Files:**
- Modify: `agent/src/core/channels/markdown_html.rs`

- [ ] **Step 1: Write the failing tests**

Add these test functions inside the existing `mod tests` block in `markdown_html.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --no-default-features --features desktop markdown_html`
Expected: FAIL to compile — `render_inline` is not defined.

- [ ] **Step 3: Implement the inline renderer**

Add these functions to `markdown_html.rs` (above the `#[cfg(test)]` block):

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --no-default-features --features desktop markdown_html`
Expected: PASS (all Task 1 + Task 2 tests).

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/channels/markdown_html.rs
git commit -m "feat(telegram): inline markdown -> HTML (bold/italic/code/links/strike)"
```

---

## Task 3: Block rendering (headings, lists, blockquote, code fences, paragraphs)

Produces a `Vec<String>` of HTML blocks. (Tables are handled in Task 4; here a table row falls through to a paragraph.)

**Files:**
- Modify: `agent/src/core/channels/markdown_html.rs`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --no-default-features --features desktop markdown_html`
Expected: FAIL to compile — `render_blocks` is not defined.

- [ ] **Step 3: Implement block rendering**

Add to `markdown_html.rs` (above the `#[cfg(test)]` block):

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --no-default-features --features desktop markdown_html`
Expected: PASS (Task 1–3 tests).

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/channels/markdown_html.rs
git commit -m "feat(telegram): block markdown -> HTML (headings/lists/quote/fences)"
```

---

## Task 4: Table rendering

Detects GFM pipe tables and renders them as a single width-padded monospace `<pre>` block.

**Files:**
- Modify: `agent/src/core/channels/markdown_html.rs`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
    #[test]
    fn table_renders_as_padded_pre() {
        let md = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 7 |";
        let out = render_blocks(md);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0],
            "<pre>Name  | Age\n------+----\nAlice | 30 \nBob   | 7  </pre>"
        );
    }

    #[test]
    fn table_escapes_cell_html() {
        let md = "| A |\n| --- |\n| x<y |";
        let out = render_blocks(md);
        assert_eq!(out, vec!["<pre>A  \n---\nx&lt;y</pre>"]);
    }

    #[test]
    fn table_requires_separator_row() {
        // A lone pipe line with no separator is just a paragraph.
        let out = render_blocks("a | b | c");
        assert_eq!(out, vec!["a | b | c"]);
    }
```

> Note: cells are right-padded to the column's max width; the header/body use
> `" | "` as the column join and the separator uses `"-+-"`, so columns line up.
> The last column is also padded (trailing spaces), which keeps every row the
> same visual width inside the monospace block.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --no-default-features --features desktop markdown_html`
Expected: FAIL — tables currently render as paragraphs, so assertions mismatch.

- [ ] **Step 3: Implement table rendering and wire it into `render_blocks`**

Add these functions to `markdown_html.rs` (above the `#[cfg(test)]` block):

```rust
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
/// Returns (html_block, lines_consumed).
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

    let mut grid = String::new();
    grid.push_str(&format_row(&rows[0], &widths));
    grid.push('\n');
    let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    grid.push_str(&sep.join("-+-"));
    for row in &rows[1..] {
        grid.push('\n');
        grid.push_str(&format_row(row, &widths));
    }

    (format!("<pre>{}</pre>", escape(&grid)), consumed)
}
```

Then wire detection into `render_blocks`, replacing the placeholder comment:

```rust
        // Table detection is added in Task 4 (here, a pipe row falls through
        // to a paragraph).
```

with:

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --no-default-features --features desktop markdown_html`
Expected: PASS (Task 1–4 tests).

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/channels/markdown_html.rs
git commit -m "feat(telegram): render markdown tables as monospace <pre> blocks"
```

---

## Task 5: Chunking + public `render_telegram`

Packs blocks into chunks ≤ `TELEGRAM_LIMIT`, hard-splitting any single oversize block (re-opening `<pre>` across the split).

**Files:**
- Modify: `agent/src/core/channels/markdown_html.rs`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --no-default-features --features desktop markdown_html`
Expected: FAIL to compile — `render_telegram` is not defined.

- [ ] **Step 3: Implement chunking + public entry**

Add to `markdown_html.rs` (above the `#[cfg(test)]` block):

```rust
/// Split one oversize block into pieces ≤ `limit`. A `<pre>…</pre>` block is
/// split at line boundaries with the `<pre>` tags re-opened on each piece so
/// every piece is independently valid HTML. Any other oversize block is
/// hard-split by character count.
fn split_block(block: &str, limit: usize) -> Vec<String> {
    const OPEN: &str = "<pre>";
    const CLOSE: &str = "</pre>";
    if block.starts_with(OPEN) && block.ends_with(CLOSE) {
        let inner = &block[OPEN.len()..block.len() - CLOSE.len()];
        let wrap = OPEN.chars().count() + CLOSE.chars().count();
        let mut out: Vec<String> = Vec::new();
        let mut cur = String::new();
        for piece in inner.split_inclusive('\n') {
            if !cur.is_empty()
                && cur.chars().count() + piece.chars().count() + wrap > limit
            {
                out.push(format!("{}{}{}", OPEN, cur, CLOSE));
                cur = String::new();
            }
            cur.push_str(piece);
        }
        if !cur.is_empty() {
            out.push(format!("{}{}{}", OPEN, cur, CLOSE));
        }
        out
    } else {
        let chars: Vec<char> = block.chars().collect();
        chars
            .chunks(limit)
            .map(|c| c.iter().collect::<String>())
            .collect()
    }
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --no-default-features --features desktop markdown_html`
Expected: PASS (all converter tests).

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/channels/markdown_html.rs
git commit -m "feat(telegram): chunk rendered HTML to 4096-char limit with pre-safe splits"
```

---

## Task 6: Route `deliver` through the converter + plain-text fallback

Make every Telegram send render markdown to HTML, post per chunk with `parse_mode=HTML`, and on a **400** re-send that chunk as stripped plain text.

**Files:**
- Modify: `agent/src/core/channels/telegram.rs` — doc comment (lines 5–10), `impl Channel for TelegramChannel` `deliver` (lines 200–242), add helpers, replace tests at lines 495–524, add new tests.

- [ ] **Step 1: Write/replace the failing tests**

In `telegram.rs`'s `mod tests`, **replace** the two tests `channel_deliver_includes_parse_mode_when_set` (lines 495–507) and `channel_deliver_omits_parse_mode_by_default` (lines 509–524) with the following four tests:

```rust
    #[tokio::test]
    async fn channel_deliver_uses_html_parse_mode_and_renders_markdown() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(200, r#"{"ok":true}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.deliver("1", "be **bold**").await.unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.len(), 1);
        let body = parse_body_json(&reqs[0]);
        assert_eq!(body["parse_mode"], "HTML");
        assert_eq!(body["text"], "be <b>bold</b>");
    }

    #[tokio::test]
    async fn channel_deliver_falls_back_to_plain_text_on_400() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(400, r#"{"ok":false,"description":"can't parse entities"}"#);
        http.push_response(200, r#"{"ok":true}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.deliver("1", "be **bold**").await.unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.len(), 2, "expected an HTML attempt then a plain retry");
        // First attempt: HTML.
        assert_eq!(parse_body_json(&reqs[0])["parse_mode"], "HTML");
        // Retry: no parse_mode, tags stripped.
        let retry = parse_body_json(&reqs[1]);
        assert!(retry.get("parse_mode").is_none(), "retry must be plain: {:?}", retry);
        assert_eq!(retry["text"], "be bold");
    }

    #[tokio::test]
    async fn channel_deliver_403_propagates_without_fallback() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(403, r#"{"ok":false,"description":"forbidden"}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        let result = ch.deliver("1", "hi").await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("403"));
        assert_eq!(http.requests().len(), 1, "403 must not trigger a retry");
    }

    #[tokio::test]
    async fn channel_deliver_long_message_is_chunked() {
        let http = Arc::new(MockHttpClient::new());
        // Enough 200s for however many chunks; extra canned responses are fine.
        for _ in 0..10 {
            http.push_response(200, r#"{"ok":true}"#);
        }
        let big: String = (0..500)
            .map(|n| format!("paragraph number {}", n))
            .collect::<Vec<_>>()
            .join("\n\n");

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.deliver("1", &big).await.unwrap();

        let reqs = http.requests();
        assert!(reqs.len() > 1, "long message should be split into multiple sends");
        for r in &reqs {
            let body = parse_body_json(r);
            let text = body["text"].as_str().unwrap();
            assert!(text.chars().count() <= 4096);
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --no-default-features --features desktop telegram`
Expected: FAIL to compile — `deliver` does not yet render/chunk; the new assertions (HTML parse_mode, fallback) don't hold.

- [ ] **Step 3: Replace `deliver` and add helpers**

In `telegram.rs`, replace the entire `impl Channel for TelegramChannel { ... }` block (lines 200–243) with:

```rust
#[async_trait]
impl Channel for TelegramChannel {
    async fn deliver(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for chunk in crate::core::channels::markdown_html::render_telegram(text) {
            self.send_one(chat_id, &chunk).await?;
        }
        Ok(())
    }
}

impl TelegramChannel {
    /// Send one already-rendered HTML chunk. Tries `parse_mode=HTML`; on a
    /// Telegram 400 (malformed entities) retries the same chunk as stripped
    /// plain text so a formatting defect can never lose a message.
    async fn send_one(
        &self,
        chat_id: &str,
        html: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let status = self.post_send(chat_id, html, Some("HTML")).await?;
        if (200..300).contains(&status) {
            return Ok(());
        }
        if status == 400 {
            let plain = strip_tags(html);
            let retry = self.post_send(chat_id, &plain, None).await?;
            if (200..300).contains(&retry) {
                return Ok(());
            }
            return Err(format!(
                "Telegram sendMessage HTTP {} (after plain-text fallback)",
                retry
            )
            .into());
        }
        Err(format!("Telegram sendMessage HTTP {}", status).into())
    }

    /// POST one sendMessage. Returns the HTTP status (transport errors are
    /// surfaced as `Err`).
    async fn post_send(
        &self,
        chat_id: &str,
        text: &str,
        parse_mode: Option<&str>,
    ) -> Result<u16, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.bot_token
        );
        let mut payload = serde_json::Map::new();
        payload.insert(
            "chat_id".to_string(),
            serde_json::Value::String(chat_id.to_string()),
        );
        payload.insert(
            "text".to_string(),
            serde_json::Value::String(text.to_string()),
        );
        if let Some(mode) = parse_mode {
            payload.insert(
                "parse_mode".to_string(),
                serde_json::Value::String(mode.to_string()),
            );
        }
        let body = serde_json::Value::Object(payload).to_string();

        let mut headers = Headers::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let resp = self.http.post(&url, &headers, body.as_bytes()).await?;
        Ok(resp.status)
    }
}

/// Remove HTML tags and unescape entities for the plain-text fallback path.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}
```

> The `parse_mode` field and `with_parse_mode` builder (lines 153, 165) are now
> unused by `deliver`. Leave them in place — they are harmless and still set by
> nothing in production. (If the compiler warns `field is never read`, that is
> addressed in Step 5.)

- [ ] **Step 4: Update the module doc comment**

In `telegram.rs`, replace the `Defaults:` bullet about parse_mode (lines 6–10):

```rust
//! - parse_mode: None. LLM replies aren't sanitized for Markdown special
//!   chars (`_`, `*`, `[`, `` ` ``); a stray underscore returns 400 from
//!   Telegram. Opt in via `with_parse_mode(Some("Markdown"))` if you
//!   know your replies are safe.
```

with:

```rust
//! - Formatting: `deliver` renders LLM markdown to Telegram HTML via
//!   `markdown_html::render_telegram` (bold/italic/code/lists/quote/links +
//!   monospace `<pre>` tables), chunks output to Telegram's 4096-char limit,
//!   and sends each chunk with `parse_mode=HTML`. On a Telegram 400 (malformed
//!   entities) the chunk is re-sent as stripped plain text, so a formatting
//!   defect can never lose a message.
```

- [ ] **Step 5: Silence the now-unused builder if needed**

Only if Step 3 produced a `field is never read` / `method is never used` warning for `parse_mode` / `with_parse_mode`, add `#[allow(dead_code)]` above the `with_parse_mode` method (line 165) and above the `parse_mode: Option<String>` field (line 153). If there is no warning, skip this step.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --no-default-features --features desktop telegram`
Expected: PASS — including `channel_deliver_posts_sendmessage_with_chat_and_text` (plain "hello" unchanged), the four new tests, and the unchanged `channel_deliver_non_200_errors` (403).

- [ ] **Step 7: Run the full converter + channel suite**

Run: `cargo test --no-default-features --features desktop channels`
Expected: PASS (all `markdown_html` + `telegram` + `channels` tests).

- [ ] **Step 8: Commit**

```bash
git add agent/src/core/channels/telegram.rs
git commit -m "feat(telegram): render markdown to HTML on send with plain-text fallback"
```

---

## Task 7: Verify ESP32 build still compiles

The converter is shared code; confirm it builds for the device target, not just host tests.

**Files:** none (build check only).

- [ ] **Step 1: Build the ESP32-S3 target**

Run: `just build devkitc`
Expected: builds successfully (the new `markdown_html` module uses only `std` String/Vec/format and compiles on esp-idf).

- [ ] **Step 2: If the build cache is stale, clean and rebuild**

Only if Step 1 fails with an unexpected error: `just clean devkitc && just build devkitc`.

- [ ] **Step 3: No commit** (no source change in this task).

---

## Self-Review Notes

- **Spec coverage:** HTML target (Tasks 2–4); hand-rolled converter (2–5); tables as `<pre>` (4); chunking with `<pre>`-safe re-open (5); plain-text fallback (6); doc-comment + wiring (6); desktop tests per construct (2–6); ESP32 build sanity (7). The spec's "any 4xx" is intentionally narrowed to **400** — documented at the top of this plan.
- **Type consistency:** `render_inline(&str)->String`, `render_blocks(&str)->Vec<String>`, `render_telegram(&str)->Vec<String>`, `post_send(...)->Result<u16,_>`, `send_one`, `strip_tags(&str)->String`, `Marker::{Unordered,Ordered(u64)}` — names are used identically across tasks.
- **No placeholders:** every code step contains complete, compilable code.
