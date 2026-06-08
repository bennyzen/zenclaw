# Telegram Markdown Rendering — Design

**Date:** 2026-06-08
**Status:** Approved (design), pending implementation plan
**Area:** `agent/src/core/channels/telegram.rs` + new `agent/src/core/channels/markdown_html.rs`

## Problem

The installable PWA is the intended mobile UX for ZenClaw, but on Android it is
structurally unreachable for a local device:

1. **mDNS** — Chrome on Android does not resolve `*.local` names.
2. **Mixed content** — the PWA is served from `https://bennyzen.github.io`
   (GitHub Pages, HTTPS); an HTTPS page fetching `http://<device-ip>` is blocked
   active mixed content (only `http://localhost` is exempt), and the device
   speaks plain HTTP.
3. **PWA installability** — an installable PWA requires a secure origin, so the
   device's own `http://<ip>` UI cannot be the installed app.

Rather than re-architect device reach (native wrapper / cloud relay), we lean on
the **Telegram channel**, which already works on Android and bypasses all three
walls. The only gap: Telegram replies are sent as raw text (`parse_mode: None`),
so LLM markdown (`**bold**`, `### headings`, code fences, bullet lists, pipe
tables) renders literally as asterisks, hashes, and pipes. Rendering it makes
Telegram a first-class mobile UX and mitigates the PWA problem with the current
feature set.

## Goals

- Render LLM markdown so Telegram displays it formatted.
- **Formatting must never cause message loss** — the property that lets us turn
  `parse_mode` on at all (see the warning at `telegram.rs:7`).
- Render markdown **tables** legibly (the user's explicit ask).
- Fix the adjacent silent-loss bug: Telegram caps a message at 4096 chars;
  longer sends currently 400 and vanish. Chunk long replies.

## Non-Goals (YAGNI)

- Telegram **entity/offset** rendering mode (UTF-16 `MessageEntity` offsets).
  More bulletproof but heavy to compute in Rust on a size-constrained target;
  HTML parse_mode handles our needs including tables.
- Code-block-as-file-attachment, emoji heading prefixes, MarkdownV2 target.
- Solving Android PWA reach (separate, deferred decision).

## Background: Telegram formatting constraints

Telegram's HTML subset supports only:
`<b> <strong> <i> <em> <u> <ins> <s> <strike> <del> <a> <code> <pre> <tg-spoiler> <blockquote>`.

There is **no `<table>`, no `<br>`** (use `\n`), no native headings, no native
lists. We target **`parse_mode=HTML`** (chosen over MarkdownV2: 3 escape chars
`& < >` vs. 18, and the tag model cleanly absorbs the downconversions Telegram
can't do natively). Reference for the table strategy:
[`telegramify-markdown`](https://github.com/sudoskys/telegramify-markdown)
renders tables as monospace `<pre>` blocks.

## Architecture

### New module: `core/channels/markdown_html.rs`

Shared (compiles on ESP32 + desktop), pure-string, no I/O, fully unit-testable
on desktop. Single public entry point:

```rust
/// Convert LLM CommonMark-ish markdown to Telegram-HTML chunks,
/// each guaranteed ≤ 4096 UTF-8-safe chars.
pub fn render_telegram(md: &str) -> Vec<String>;
```

Pipeline: parse markdown into a light block/inline model (hand-rolled for the LLM
subset), render each block to Telegram HTML, then chunk.

### Conversion rules

| Markdown | Telegram HTML |
|---|---|
| `**b**` / `__b__` | `<b>b</b>` |
| `*i*` / `_i_` | `<i>i</i>` |
| `` `code` `` | `<code>code</code>` |
| ` ```lang\n…\n``` ` | `<pre><code class="language-lang">…</code></pre>` |
| `~~s~~` | `<s>s</s>` |
| `[t](url)` | `<a href="url">t</a>` |
| `#`..`######` heading | `<b>…</b>`; H1–H2 also wrapped in `<u>` |
| `- ` / `* ` / `+ ` list | `• ` lines; nested → indented |
| `1.` ordered list | `1. ` lines |
| `> quote` | `<blockquote>…</blockquote>` |
| GFM pipe table | one `<pre>` block, columns width-padded (see below) |
| `&` `<` `>` in text spans | escaped to `&amp;` `&lt;` `&gt;` |

Escaping applies to text content only, never to generated tags or `href`
attribute values (attribute values escape `&`, `<`, `>`, `"`).

### Table rendering

1. Detect a GFM pipe table: a header row, a separator row
   (`|---|:--:|`), and ≥0 body rows.
2. Strip alignment colons; compute each column's max display width across header
   + body cells.
3. Pad every cell to its column width, join with ` | `, emit the header, a
   dashed separator line, and the body rows.
4. Wrap the whole grid in a single `<pre>…</pre>` (monospace preserves columns).

**Known caveat (documented, accepted):** Telegram mobile wraps `<pre>` at ~40
monospace chars; very wide tables wrap awkwardly. We render the grid correctly
and accept that wide tables are wide — there is no Telegram mechanism to avoid
this.

### Chunking (4096-char cap)

- Accumulate rendered blocks; start a new chunk before exceeding 4096 chars.
- Never split inside a tag or a `<pre>` block.
- If a single `<pre>` (large code block or table) alone exceeds 4096, hard-split
  it at line boundaries, closing `</pre>` and re-opening `<pre>` across the
  boundary so each chunk is independently valid HTML.
- Count by UTF-8 chars conservatively to stay under Telegram's limit.

### Delivery + safety net (`telegram.rs` `deliver`)

- Route delivery text through `render_telegram()`, producing 1..N chunks.
- Send each chunk with `parse_mode=HTML`.
- **On any 4xx** from Telegram for a chunk (malformed entities), automatically
  **re-send that chunk as plain text** (omit `parse_mode`) and log the fallback.
  A message is never lost to a formatting defect.
- Send chunks in order.

### Wiring

- Build the production `TelegramChannel` in HTML mode and route sends through the
  converter. The existing `with_parse_mode()` builder (`telegram.rs:165`) is
  retained for tests/override.
- Update the doc comment at `telegram.rs:7` to reflect that parse_mode is now on
  by default with a plain-text fallback.

## Error handling

| Failure | Behavior |
|---|---|
| Malformed/edge markdown the parser can't model | Falls through as escaped plain text inside the chunk; never panics. |
| Telegram 4xx on an HTML chunk | Retry that chunk as plain text; log. |
| Telegram 5xx / network error | Propagate existing error (unchanged from today). |
| Reply > 4096 chars | Chunked into multiple ordered messages. |
| `<pre>`/table > 4096 chars alone | Hard-split at line boundaries with re-opened `<pre>`. |

## Testing (desktop, `#[cfg(all(test, feature = "desktop"))]`)

Pure-string logic — no hardware. Cover:

- Each inline construct: bold, italic, inline code, strikethrough, links, escaping.
- Headings → bold (+underline for H1–H2).
- Bullet and ordered lists, nested lists.
- Blockquote.
- Fenced code block with and without language.
- **Tables:** column-width padding, alignment-colon stripping, `<pre>` wrapping,
  cells containing `<`/`&`.
- Mixed-construct document.
- Chunking: clean block-boundary split at ~4096; oversize `<pre>` hard-split with
  re-opened tags; never split mid-tag.
- Delivery: `parse_mode=HTML` present on success; plain-text retry on simulated
  4xx (via `MockHttpClient`).

## Files

- **New:** `agent/src/core/channels/markdown_html.rs` — converter + chunker + tests.
- **Modified:** `agent/src/core/channels/telegram.rs` — route `deliver` through
  the converter, HTML mode, plain-text fallback, updated doc comment.
- **Modified:** `agent/src/core/channels/mod.rs` — module export if needed.
- **Modified (wiring):** the two production `TelegramChannel` construction sites —
  `agent/src/main.rs:316` (ESP32) and `agent/src/desktop/run.rs:190` (desktop) —
  enable HTML mode. (If `deliver` routes through the converter internally, these
  may need no change beyond confirming default behavior.)
