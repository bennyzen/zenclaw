//! Persistent memory — a single action-dispatched tool backed by
//! `data/MEMORY.md`. No vectors, no embeddings, no hidden background
//! work. Every save/edit/delete is a visible tool call, and the
//! capacity signal in tool results nudges the agent to ask the user
//! before grooming memory unilaterally.
//!
//! On-disk format. The current format puts the title on the `##`
//! markdown heading and pushes id/timestamp/tags to a metadata line
//! below it:
//!
//! ```text
//! ## Prefers explicit error handling
//! [mem_a3f2c1d8] 2026-05-01T10:30:00Z (tags: preference, code-style)
//!
//! Hates unwrap() outside tests, prefers Result<T, E>.
//! ```
//!
//! The legacy format (no title; metadata fused into the heading) is
//! still readable and is preserved on round-trip until the entry is
//! edited:
//!
//! ```text
//! ## [97de55fb-532e-4f3f-82fb-004f5f5eb4b6] 2026-04-15T10:00:00Z (tags: legacy)
//! Body content.
//! ```
//!
//! Caps: 64 KB total + 200 entries. Hard fail at either; warning footer
//! at >= 70%. The agent surfaces near-cap conditions to the user and
//! proposes a compaction plan rather than deleting silently.
//!
//! Title cap: 80 chars (commit-subject convention).
//!
//! Limitation: a memory body containing a line that looks like the
//! current header (`## ` followed on the next non-blank line by a
//! `[<id>] <timestamp>` metadata line) would confuse the parser. This
//! is unlikely in practice and the model should not produce such
//! content. If it ever bites, escape on serialize.

use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

/// Hard byte cap for the entire MEMORY.md file.
const MAX_BYTES: usize = 64 * 1024;
/// Hard entry-count cap.
const MAX_ENTRIES: usize = 200;
/// Capacity at which the tool result starts nudging the agent to compact.
const WARN_THRESHOLD_PCT: usize = 70;
/// Top-K cutoff for memory search.
const SEARCH_TOP_K: usize = 10;
/// Title cap (chars). Commit-subject convention.
const MAX_TITLE_CHARS: usize = 80;
/// Fallback "title" length for legacy (titleless) entries in list output.
const LEGACY_TITLE_PREVIEW_CHARS: usize = 60;

pub struct MemoryTool;

#[async_trait]
impl Tool for MemoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "memory".to_string(),
            description: "Persistent memory across chats — survives reboots. \
                Actions:\n\
                - save: persist a fact, preference, decision, or constraint the user wants remembered. \
                  Provide a short title (≤80 chars, commit-subject style). Don't say \"I'll remember\" without saving.\n\
                - search: keyword search over saved memory; optional tag filter. Use whenever the user \
                  references something they may have told you before.\n\
                - list: browse entries. Without a tag returns one line per entry; with a tag returns \
                  full content for matching entries.\n\
                - get: retrieve one entry by id.\n\
                - edit: update an entry's title/content/tags. id and timestamp are preserved.\n\
                - delete: permanently remove an entry. User-initiated deletes (\"forget X\") run directly; \
                  agent-initiated compaction must be approved by the user first.\n\
                save/edit/delete return a capacity footer — at >=70% surface this to the user and propose \
                a compaction plan rather than grooming silently.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["save", "search", "list", "get", "edit", "delete"],
                        "description": "Operation to perform"
                    },
                    "id":      { "type": "string", "description": "Memory ID (e.g. mem_a3f2c1d8). Required for get/edit/delete." },
                    "title":   { "type": "string", "description": "Short label (≤80 chars). Required for save; optional for edit (replaces existing)." },
                    "content": { "type": "string", "description": "Body text. Used by save/edit. Pass \"\" on edit to clear." },
                    "tags":    { "type": "string", "description": "Comma-separated tags. Used by save/edit. Pass \"\" on edit to clear." },
                    "query":   { "type": "string", "description": "Keywords for search." },
                    "tag":     { "type": "string", "description": "Tag filter (case-insensitive). Used by search and list." }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");
        match action {
            "save"   => do_save(&args, ctx),
            "search" => do_search(&args, ctx),
            "list"   => do_list(&args, ctx),
            "get"    => do_get(&args, ctx),
            "edit"   => do_edit(&args, ctx),
            "delete" => do_delete(&args, ctx),
            "" => ToolResult::Error("memory: 'action' is required".into()),
            other => ToolResult::Error(format!("memory: unknown action '{}'", other)),
        }
    }
}

// --- per-action implementations ---

fn do_save(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let title = match args["title"].as_str() {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => return ToolResult::Error("memory(save): 'title' is required and must be non-empty".into()),
    };
    if title.chars().count() > MAX_TITLE_CHARS {
        return ToolResult::Error(format!(
            "memory(save): title must be ≤{} chars (got {}). Shorten it; put detail in 'content'.",
            MAX_TITLE_CHARS,
            title.chars().count(),
        ));
    }
    let content = args["content"]
        .as_str()
        .map(|c| c.trim().to_string())
        .unwrap_or_default();
    let tags = parse_tags(args["tags"].as_str().unwrap_or(""));

    let mut blocks = match read_memory_blocks(ctx) {
        Ok(b) => b,
        Err(e) => return ToolResult::Error(format!("Failed to read memory: {}", e)),
    };

    if blocks.len() >= MAX_ENTRIES {
        return ToolResult::Error(format!(
            "Memory full: {}/{} entries. Tell the user and propose a compaction plan, then call memory(action=delete) or memory(action=edit) before retrying.",
            blocks.len(),
            MAX_ENTRIES,
        ));
    }

    let id = format!("mem_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    blocks.push(MemoryBlock { id: id.clone(), timestamp, title, tags, content });

    let serialized = serialize_blocks(&blocks);
    if serialized.len() > MAX_BYTES {
        return ToolResult::Error(format!(
            "Memory full: would be {}B (max {}B). Tell the user and propose a compaction plan first.",
            serialized.len(),
            MAX_BYTES,
        ));
    }

    if let Err(e) = write_memory_file(ctx, &serialized) {
        return ToolResult::Error(format!("Failed to write memory: {}", e));
    }

    let footer = capacity_footer(serialized.len(), blocks.len());
    ToolResult::Text(format!("Saved {}.\n{}", id, footer))
}

fn do_search(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let query = match args["query"].as_str() {
        Some(q) if !q.trim().is_empty() => q.trim().to_string(),
        _ => return ToolResult::Error("memory(search): 'query' is required".into()),
    };
    let tag_filter = args["tag"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());

    let blocks = match read_memory_blocks(ctx) {
        Ok(b) => b,
        Err(e) => return ToolResult::Error(format!("Failed to read memory: {}", e)),
    };
    if blocks.is_empty() {
        return ToolResult::Text("No memories saved yet.".into());
    }

    let ranked = rank_search(&blocks, &query, tag_filter);
    if ranked.is_empty() {
        return ToolResult::Text(format!("No matches for '{}'.", query));
    }

    let rendered: Vec<String> = ranked
        .into_iter()
        .take(SEARCH_TOP_K)
        .map(|(_, b)| format_block(b))
        .collect();
    ToolResult::Text(rendered.join("\n\n"))
}

fn do_list(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let tag_filter = args["tag"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());

    let blocks = match read_memory_blocks(ctx) {
        Ok(b) => b,
        Err(e) => return ToolResult::Error(format!("Failed to read memory: {}", e)),
    };
    if blocks.is_empty() {
        return ToolResult::Text("No memories saved yet.".into());
    }

    let filtered: Vec<&MemoryBlock> = blocks
        .iter()
        .filter(|b| match tag_filter {
            None => true,
            Some(tag) => b.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)),
        })
        .collect();

    if filtered.is_empty() {
        return ToolResult::Text(format!("No memories with tag '{}'.", tag_filter.unwrap_or("")));
    }

    let lines: Vec<String> = if tag_filter.is_some() {
        filtered.iter().map(|b| format_block(b)).collect()
    } else {
        filtered.iter().map(|b| format_list_entry(b)).collect()
    };

    ToolResult::Text(format!("{} memories:\n{}", filtered.len(), lines.join("\n")))
}

fn do_get(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let id = match args["id"].as_str() {
        Some(i) if !i.trim().is_empty() => i.trim().to_string(),
        _ => return ToolResult::Error("memory(get): 'id' is required".into()),
    };

    let blocks = match read_memory_blocks(ctx) {
        Ok(b) => b,
        Err(e) => return ToolResult::Error(format!("Failed to read memory: {}", e)),
    };

    match blocks.iter().find(|b| b.id == id) {
        Some(b) => ToolResult::Text(format_block(b)),
        None => ToolResult::Error(format!("Memory '{}' not found.", id)),
    }
}

fn do_edit(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let id = match args["id"].as_str() {
        Some(i) if !i.trim().is_empty() => i.trim().to_string(),
        _ => return ToolResult::Error("memory(edit): 'id' is required".into()),
    };
    let new_title = args["title"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(t) = &new_title {
        if t.chars().count() > MAX_TITLE_CHARS {
            return ToolResult::Error(format!(
                "memory(edit): title must be ≤{} chars (got {}).",
                MAX_TITLE_CHARS,
                t.chars().count(),
            ));
        }
    }
    // Distinguish "not provided" from "provided as empty string" — empty
    // string is a valid clear-the-field signal.
    let new_content = args.get("content").and_then(|v| v.as_str()).map(|s| s.trim().to_string());
    let new_tags = args.get("tags").and_then(|v| v.as_str()).map(parse_tags);

    if new_title.is_none() && new_content.is_none() && new_tags.is_none() {
        return ToolResult::Error("memory(edit): must provide 'title', 'content', and/or 'tags'".into());
    }

    let mut blocks = match read_memory_blocks(ctx) {
        Ok(b) => b,
        Err(e) => return ToolResult::Error(format!("Failed to read memory: {}", e)),
    };

    let block = match blocks.iter_mut().find(|b| b.id == id) {
        Some(b) => b,
        None => return ToolResult::Error(format!("Memory '{}' not found.", id)),
    };

    if let Some(t) = new_title {
        block.title = t;
    }
    if let Some(c) = new_content {
        block.content = c;
    }
    if let Some(t) = new_tags {
        block.tags = t;
    }

    let serialized = serialize_blocks(&blocks);
    if serialized.len() > MAX_BYTES {
        return ToolResult::Error(format!(
            "Edit would exceed memory cap: {}B > {}B. Trim or delete other entries first.",
            serialized.len(),
            MAX_BYTES,
        ));
    }

    if let Err(e) = write_memory_file(ctx, &serialized) {
        return ToolResult::Error(format!("Failed to write memory: {}", e));
    }

    let footer = capacity_footer(serialized.len(), blocks.len());
    ToolResult::Text(format!("Edited {}.\n{}", id, footer))
}

fn do_delete(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let id = match args["id"].as_str() {
        Some(i) if !i.trim().is_empty() => i.trim().to_string(),
        _ => return ToolResult::Error("memory(delete): 'id' is required".into()),
    };

    let mut blocks = match read_memory_blocks(ctx) {
        Ok(b) => b,
        Err(e) => return ToolResult::Error(format!("Failed to read memory: {}", e)),
    };

    let original_len = blocks.len();
    blocks.retain(|b| b.id != id);
    if blocks.len() == original_len {
        return ToolResult::Error(format!("Memory '{}' not found.", id));
    }

    let serialized = serialize_blocks(&blocks);
    if let Err(e) = write_memory_file(ctx, &serialized) {
        return ToolResult::Error(format!("Failed to write memory: {}", e));
    }

    let footer = capacity_footer(serialized.len(), blocks.len());
    ToolResult::Text(format!("Deleted {}.\n{}", id, footer))
}

// --- shared types & helpers ---

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemoryBlock {
    id: String,
    timestamp: String,
    /// Short label, ≤ MAX_TITLE_CHARS. Empty for legacy entries; on display we
    /// fall back to a derived snippet of `content`.
    title: String,
    tags: Vec<String>,
    content: String,
}

struct ParsedMetadata {
    id: String,
    timestamp: String,
    tags: Vec<String>,
}

fn memory_path(ctx: &ToolContext) -> String {
    format!("{}/MEMORY.md", ctx.data_dir)
}

fn parse_tags(s: &str) -> Vec<String> {
    s.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Cloud key for the agent's MEMORY.md file. Derived once so callers
/// can pass the same key to read + write paths.
const MEMORY_CLOUD_KEY: &str = "sys/MEMORY.md";

fn read_memory_blocks(ctx: &ToolContext) -> std::io::Result<Vec<MemoryBlock>> {
    // Cloud mode: cache is the source of truth (boot_restore populated
    // it; subsequent writes update it before strict_put). Fall back to
    // local FS only when the cache hasn't been seeded yet (post-boot
    // before any read or write).
    if let Some(cloud) = &ctx.cloud {
        if let Some(bytes) = cloud.cache.get(MEMORY_CLOUD_KEY) {
            let s = String::from_utf8_lossy(&bytes);
            return Ok(parse_blocks(&s));
        }
    }
    let path = memory_path(ctx);
    match std::fs::read_to_string(&path) {
        Ok(c) => Ok(parse_blocks(&c)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Write `content` to MEMORY.md. In cloud mode: update the cache, then
/// `strict_put` to S3 (block on confirmation, retry up to retry_max,
/// surface the error to the caller on exhaustion). Always also write
/// to the local FS as a snapshot fallback — if cloud machinery breaks
/// in a future boot, the user's MEMORY.md is still readable on flash.
fn write_memory_file(ctx: &ToolContext, content: &str) -> std::io::Result<()> {
    if let Some(cloud) = &ctx.cloud {
        cloud
            .cache
            .put(MEMORY_CLOUD_KEY, content.as_bytes().to_vec());
        crate::core::cloud::strict::strict_put(
            &cloud.store,
            MEMORY_CLOUD_KEY,
            content.as_bytes(),
            cloud.retry_max,
            cloud.backoff_cap_secs,
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    }
    let path = memory_path(ctx);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, content)
}

/// Parse the on-disk MEMORY.md format. Accepts both the current shape
/// (`## Title\n[id] ts (tags)\n\nbody`) and the legacy shape
/// (`## [id] ts (tags)\nbody`).
fn parse_blocks(content: &str) -> Vec<MemoryBlock> {
    let lines: Vec<&str> = content.lines().collect();
    let mut blocks: Vec<MemoryBlock> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        match try_parse_header_at(&lines, i) {
            Some((block_header, body_start)) => {
                let body_end = next_block_start(&lines, body_start);
                let mut block = block_header;
                block.content = lines[body_start..body_end].join("\n").trim().to_string();
                blocks.push(block);
                i = body_end;
            }
            None => i += 1,
        }
    }
    blocks
}

/// If `lines[i]` starts a memory header (current or legacy), return the
/// partially-built block (without `content`) plus the index where its body
/// begins. Otherwise `None`.
fn try_parse_header_at(lines: &[&str], i: usize) -> Option<(MemoryBlock, usize)> {
    let rest = lines[i].trim_start().strip_prefix("## ")?.trim();

    // Legacy: heading-line carries the metadata directly.
    if rest.starts_with('[') {
        let meta = parse_metadata(rest)?;
        return Some((
            MemoryBlock {
                id: meta.id,
                timestamp: meta.timestamp,
                title: String::new(),
                tags: meta.tags,
                content: String::new(),
            },
            i + 1,
        ));
    }

    // Current: heading is the title, metadata follows on the next non-blank line.
    let mut j = i + 1;
    while j < lines.len() && lines[j].trim().is_empty() {
        j += 1;
    }
    if j >= lines.len() {
        return None;
    }
    let meta = parse_metadata(lines[j].trim())?;
    Some((
        MemoryBlock {
            id: meta.id,
            timestamp: meta.timestamp,
            title: rest.to_string(),
            tags: meta.tags,
            content: String::new(),
        },
        j + 1,
    ))
}

/// Find the next index `>= start` that begins a new memory header, or `lines.len()`.
fn next_block_start(lines: &[&str], start: usize) -> usize {
    let mut i = start;
    while i < lines.len() {
        if try_parse_header_at(lines, i).is_some() {
            return i;
        }
        i += 1;
    }
    lines.len()
}

/// Parse a metadata fragment: `[<id>] <timestamp>` or
/// `[<id>] <timestamp> (tags: <a, b, c>)`.
fn parse_metadata(s: &str) -> Option<ParsedMetadata> {
    let s = s.trim().strip_prefix('[')?;
    let (id, rest) = s.split_once(']')?;
    let rest = rest.trim_start();

    let (timestamp, tags) = match rest.find(" (tags:") {
        Some(idx) => {
            let ts = rest[..idx].trim().to_string();
            let tags_part = rest[idx + " (tags:".len()..].trim_end_matches(')').trim();
            let tags = tags_part
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            (ts, tags)
        }
        None => (rest.trim().to_string(), Vec::new()),
    };

    Some(ParsedMetadata { id: id.trim().to_string(), timestamp, tags })
}

/// Serialize blocks. Entries with a title use the current format; entries
/// without one (legacy, never edited) are written back in legacy format so
/// the on-disk shape doesn't churn for untouched data.
fn serialize_blocks(blocks: &[MemoryBlock]) -> String {
    let mut out = String::new();
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if b.title.is_empty() {
            out.push_str("## ");
            out.push_str(&metadata_line(b));
            out.push('\n');
            out.push_str(&b.content);
            out.push('\n');
        } else {
            out.push_str("## ");
            out.push_str(&b.title);
            out.push('\n');
            out.push_str(&metadata_line(b));
            out.push_str("\n\n");
            out.push_str(&b.content);
            out.push('\n');
        }
    }
    out
}

fn metadata_line(b: &MemoryBlock) -> String {
    if b.tags.is_empty() {
        format!("[{}] {}", b.id, b.timestamp)
    } else {
        format!("[{}] {} (tags: {})", b.id, b.timestamp, b.tags.join(", "))
    }
}

/// Full-block render for memory(get) / memory(search) results. Mirrors
/// the on-disk layout so the agent sees exactly what's stored.
fn format_block(b: &MemoryBlock) -> String {
    if b.title.is_empty() {
        format!("## {}\n{}", metadata_line(b), b.content)
    } else {
        format!("## {}\n{}\n\n{}", b.title, metadata_line(b), b.content)
    }
}

/// One-line entry for `memory(list)` (no tag filter). The id stays full
/// so the agent can pass it to memory(get|edit|delete), but the
/// timestamp drops to its date part. Untitled (legacy) entries fall
/// back to a derived snippet of content so the list still has signal.
fn format_list_entry(b: &MemoryBlock) -> String {
    let date = b.timestamp.split('T').next().unwrap_or(&b.timestamp);
    let label = display_title(b);
    if b.tags.is_empty() {
        format!("[{}] {} — {}", b.id, label, date)
    } else {
        format!("[{}] {} — {} (tags: {})", b.id, label, date, b.tags.join(", "))
    }
}

/// What to show as the title in list views. Prefers the explicit title;
/// for legacy entries derives a quoted snippet from the first line of
/// content so the agent (or user) can tell it's not a curated title.
fn display_title(b: &MemoryBlock) -> String {
    if !b.title.is_empty() {
        return b.title.clone();
    }
    let first_line = b.content.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return "(empty)".to_string();
    }
    let chars: Vec<char> = first_line.chars().collect();
    let snippet: String = if chars.len() <= LEGACY_TITLE_PREVIEW_CHARS {
        first_line.to_string()
    } else {
        let trimmed: String = chars.iter().take(LEGACY_TITLE_PREVIEW_CHARS).collect();
        format!("{}…", trimmed.trim_end())
    };
    format!("\"{}\"", snippet)
}

/// Tag bonus + body term frequency. For a corpus of <=200 short entries
/// this is microseconds; no need for a real BM25 with idf precompute.
fn rank_search<'a>(
    blocks: &'a [MemoryBlock],
    query: &str,
    tag_filter: Option<&str>,
) -> Vec<(f32, &'a MemoryBlock)> {
    let query_lower = query.to_lowercase();
    let query_terms: Vec<&str> = query_lower
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    if query_terms.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(f32, &MemoryBlock)> = blocks
        .iter()
        .filter(|b| match tag_filter {
            None => true,
            Some(tag) => b.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)),
        })
        .filter_map(|b| {
            let s = score_block(b, &query_terms);
            if s > 0.0 {
                Some((s, b))
            } else {
                None
            }
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

fn score_block(b: &MemoryBlock, query_terms: &[&str]) -> f32 {
    let title_lower = b.title.to_lowercase();
    let body_lower = b.content.to_lowercase();
    let body_word_count = b.content.split_whitespace().count().max(1) as f32;
    let tags_lower: Vec<String> = b.tags.iter().map(|t| t.to_lowercase()).collect();

    let mut score = 0.0;
    for term in query_terms {
        // Title hit: highest signal — the user-curated label IS the gist.
        if !title_lower.is_empty() && title_lower.contains(term) {
            score += 5.0;
        }
        // Tag exact match: strong signal.
        if tags_lower.iter().any(|t| t == term) {
            score += 3.0;
        }
        // Body term frequency, normalized by body length so a one-line
        // memory matching once beats a paragraph matching once.
        let count = body_lower.matches(term).count() as f32;
        if count > 0.0 {
            score += 1.0 + count / body_word_count;
        }
    }
    score
}

fn capacity_footer(bytes: usize, count: usize) -> String {
    let bytes_pct = (bytes * 100 / MAX_BYTES).min(100);
    let count_pct = (count * 100 / MAX_ENTRIES).min(100);
    let pct = bytes_pct.max(count_pct);
    let base = format!(
        "(memory: {}% — {}/{} entries, {:.1}KB/{}KB)",
        pct,
        count,
        MAX_ENTRIES,
        bytes as f32 / 1024.0,
        MAX_BYTES / 1024,
    );
    if pct >= WARN_THRESHOLD_PCT {
        format!(
            "{}\nMemory near capacity — surface this to the user and propose a compaction plan (entries to merge or delete) before saving more. Wait for approval before calling memory(action=delete) or memory(action=edit) on agent-initiated changes.",
            base
        )
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, ts: &str, title: &str, tags: &[&str], content: &str) -> MemoryBlock {
        MemoryBlock {
            id: id.into(),
            timestamp: ts.into(),
            title: title.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            content: content.into(),
        }
    }

    #[test]
    fn parse_serialize_roundtrip_titled_and_legacy() {
        let blocks = vec![
            block("mem_aaa", "2026-05-01T10:00:00Z", "Prefers explicit errors",
                  &["preference", "rust"], "Hates unwrap() outside tests."),
            block("mem_bbb", "2026-05-01T11:00:00Z", "", &[], "Lives in Berlin."),
        ];
        let serialized = serialize_blocks(&blocks);
        let parsed = parse_blocks(&serialized);
        assert_eq!(parsed, blocks);
    }

    #[test]
    fn parse_handles_preamble() {
        let content = "preamble line\nmore preamble\n\n## [mem_x] 2026-05-01T00:00:00Z\nbody";
        let blocks = parse_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, "mem_x");
        assert_eq!(blocks[0].title, "");
        assert_eq!(blocks[0].content, "body");
    }

    #[test]
    fn parse_multiline_body() {
        let content = "## [mem_x] 2026-05-01T00:00:00Z\nline 1\nline 2\nline 3";
        let blocks = parse_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, "line 1\nline 2\nline 3");
    }

    #[test]
    fn parse_handles_tags() {
        let content = "## [mem_x] 2026-05-01T00:00:00Z (tags: a, b, c)\nbody";
        let blocks = parse_blocks(content);
        assert_eq!(blocks[0].tags, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_titled_format() {
        let content = "## My title\n[mem_x] 2026-05-01T00:00:00Z (tags: foo)\n\nbody line\nsecond line";
        let blocks = parse_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, "mem_x");
        assert_eq!(blocks[0].title, "My title");
        assert_eq!(blocks[0].tags, vec!["foo"]);
        assert_eq!(blocks[0].content, "body line\nsecond line");
    }

    #[test]
    fn parse_mixed_titled_and_legacy() {
        let content = "\
## Titled one
[mem_a] 2026-05-01T00:00:00Z

body a

## [mem_b] 2026-05-01T01:00:00Z (tags: legacy)
body b
";
        let blocks = parse_blocks(content);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].title, "Titled one");
        assert_eq!(blocks[0].content, "body a");
        assert_eq!(blocks[1].title, "");
        assert_eq!(blocks[1].id, "mem_b");
        assert_eq!(blocks[1].content, "body b");
    }

    #[test]
    fn parse_does_not_split_on_body_hash_headers() {
        // A `## ` line in the body, *not* followed by a metadata line, must
        // be treated as body content, not a memory boundary.
        let content = "\
## Real title
[mem_x] 2026-05-01T00:00:00Z

intro line
## Looks like a header but is body
trailing body
";
        let blocks = parse_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].content.contains("Looks like a header but is body"));
    }

    #[test]
    fn parse_tags_helper_trims_and_drops_empty() {
        assert_eq!(parse_tags(""), Vec::<String>::new());
        assert_eq!(parse_tags("a"), vec!["a"]);
        assert_eq!(parse_tags(" a ,  b ,, c "), vec!["a", "b", "c"]);
    }

    #[test]
    fn ranks_title_match_above_tag_above_body() {
        let blocks = vec![
            block("mem_t", "t", "Loves rust",   &[],         "favorite language"),
            block("mem_g", "t", "",             &["rust"],   "favorite language"),
            block("mem_b", "t", "",             &[],         "i talk about rust often"),
        ];
        let ranked = rank_search(&blocks, "rust", None);
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].1.id, "mem_t");
        assert_eq!(ranked[1].1.id, "mem_g");
        assert_eq!(ranked[2].1.id, "mem_b");
    }

    #[test]
    fn search_filters_by_tag() {
        let blocks = vec![
            block("mem_a", "t", "", &["rust"], "favorite"),
            block("mem_b", "t", "", &["python"], "favorite"),
        ];
        let ranked = rank_search(&blocks, "favorite", Some("python"));
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].1.id, "mem_b");
    }

    #[test]
    fn display_title_uses_title_when_set() {
        let b = block("mem_x", "t", "Real title", &[], "body");
        assert_eq!(display_title(&b), "Real title");
    }

    #[test]
    fn display_title_quotes_first_line_when_legacy() {
        let b = block("mem_x", "t", "", &[], "first line content\nmore body");
        assert_eq!(display_title(&b), "\"first line content\"");
    }

    #[test]
    fn display_title_truncates_long_legacy_first_line() {
        let long = "a".repeat(200);
        let b = block("mem_x", "t", "", &[], &long);
        let dt = display_title(&b);
        assert!(dt.starts_with('"') && dt.ends_with("…\""));
        assert!(dt.chars().count() <= LEGACY_TITLE_PREVIEW_CHARS + 4); // quotes + ellipsis
    }

    #[test]
    fn capacity_footer_warns_at_70_percent() {
        let footer = capacity_footer(50 * 1024, 100);
        assert!(footer.contains("78%"), "expected 78% in: {}", footer);
        assert!(footer.contains("near capacity"));
    }

    #[test]
    fn capacity_footer_picks_max_dimension() {
        // 10KB but 180 entries — count dominates
        let footer = capacity_footer(10 * 1024, 180);
        assert!(footer.contains("90%"), "expected 90% (count-driven) in: {}", footer);
    }

    #[test]
    fn capacity_footer_silent_below_threshold() {
        let footer = capacity_footer(5 * 1024, 10);
        assert!(!footer.contains("near capacity"));
    }

    #[cfg(feature = "desktop")]
    mod tool_tests {
        use super::*;
        use crate::config::Config;
        use crate::core::sessions::SessionManager;
        use std::sync::Arc;

        fn ctx(tmp: &tempfile::TempDir) -> ToolContext {
            let config: Config = serde_json::from_str("{}").expect("default config");
            ToolContext {
                chat_id: "test".into(),
                prompt_source: None,
                config: Arc::new(config),
                sessions: Arc::new(SessionManager::new(&format!("{}/sessions", tmp.path().display()))),
                data_dir: tmp.path().display().to_string(),
                cloud: None,
            }
        }

        fn unwrap_text(r: ToolResult) -> String {
            match r {
                ToolResult::Text(s) => s,
                ToolResult::Error(e) => panic!("expected Text, got Error: {}", e),
                ToolResult::Json(v) => panic!("expected Text, got Json: {}", v),
            }
        }

        fn unwrap_error(r: ToolResult) -> String {
            match r {
                ToolResult::Error(s) => s,
                other => panic!("expected Error, got {:?}", other),
            }
        }

        fn id_from_save(saved: &str) -> &str {
            saved.lines().next().unwrap().trim_start_matches("Saved ").trim_end_matches('.')
        }

        async fn save(c: &ToolContext, args: serde_json::Value) -> ToolResult {
            let mut a = args;
            a["action"] = json!("save");
            MemoryTool.execute(a, c).await
        }

        async fn search(c: &ToolContext, args: serde_json::Value) -> ToolResult {
            let mut a = args;
            a["action"] = json!("search");
            MemoryTool.execute(a, c).await
        }

        async fn list(c: &ToolContext, args: serde_json::Value) -> ToolResult {
            let mut a = args;
            a["action"] = json!("list");
            MemoryTool.execute(a, c).await
        }

        async fn get(c: &ToolContext, args: serde_json::Value) -> ToolResult {
            let mut a = args;
            a["action"] = json!("get");
            MemoryTool.execute(a, c).await
        }

        async fn edit(c: &ToolContext, args: serde_json::Value) -> ToolResult {
            let mut a = args;
            a["action"] = json!("edit");
            MemoryTool.execute(a, c).await
        }

        async fn delete(c: &ToolContext, args: serde_json::Value) -> ToolResult {
            let mut a = args;
            a["action"] = json!("delete");
            MemoryTool.execute(a, c).await
        }

        #[tokio::test]
        async fn save_then_get_roundtrip() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);

            let saved = unwrap_text(save(&c, json!({
                "title":   "Loves Rust",
                "content": "Especially the borrow checker.",
                "tags":    "preference"
            })).await);
            let id = id_from_save(&saved);

            let got = unwrap_text(get(&c, json!({"id": id})).await);
            assert!(got.contains("Loves Rust"), "expected title in get: {}", got);
            assert!(got.contains("borrow checker"));
            assert!(got.contains("preference"));
        }

        #[tokio::test]
        async fn save_requires_title() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);

            let err = unwrap_error(save(&c, json!({"content": "fact only"})).await);
            assert!(err.contains("title"), "expected title-required error: {}", err);
        }

        #[tokio::test]
        async fn save_rejects_overlong_title() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            let long_title = "x".repeat(MAX_TITLE_CHARS + 1);
            let err = unwrap_error(save(&c, json!({"title": long_title})).await);
            assert!(err.contains("≤80"), "expected length error: {}", err);
        }

        #[tokio::test]
        async fn save_allows_title_only() {
            // One-liner: title only, no body.
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            let saved = unwrap_text(save(&c, json!({"title": "A bare fact"})).await);
            let id = id_from_save(&saved);
            let got = unwrap_text(get(&c, json!({"id": id})).await);
            assert!(got.contains("A bare fact"));
        }

        #[tokio::test]
        async fn search_finds_by_title_and_tag_and_body() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);

            save(&c, json!({"title": "Lives in Berlin",     "tags": "profile"})).await;
            save(&c, json!({"title": "Drinks black coffee", "tags": "preference"})).await;

            let by_title = unwrap_text(search(&c, json!({"query": "berlin"})).await);
            assert!(by_title.contains("Berlin"));

            let by_tag = unwrap_text(search(&c, json!({"query": "drinks", "tag": "preference"})).await);
            assert!(by_tag.contains("coffee"));

            let no_match = unwrap_text(search(&c, json!({"query": "xyzzy"})).await);
            assert!(no_match.contains("No matches"));
        }

        #[tokio::test]
        async fn list_shows_titles_without_tag() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            save(&c, json!({"title": "Fact One"})).await;
            save(&c, json!({"title": "Fact Two", "tags": "x"})).await;

            let listed = unwrap_text(list(&c, json!({})).await);
            assert!(listed.contains("2 memories"));
            assert!(listed.contains("Fact One"), "list must show title: {}", listed);
            assert!(listed.contains("Fact Two"), "list must show title: {}", listed);
        }

        #[tokio::test]
        async fn list_falls_back_to_quoted_first_line_for_legacy() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            // Hand-write a legacy entry directly to disk.
            std::fs::write(
                format!("{}/MEMORY.md", tmp.path().display()),
                "## [mem_legacy] 2026-04-01T00:00:00Z\nLegacy first line.\n",
            ).unwrap();

            let listed = unwrap_text(list(&c, json!({})).await);
            assert!(listed.contains("\"Legacy first line.\""), "expected quoted fallback: {}", listed);
        }

        #[tokio::test]
        async fn list_full_content_with_tag_filter() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            save(&c, json!({"title": "Fact One", "content": "body alpha", "tags": "x"})).await;
            save(&c, json!({"title": "Fact Two", "content": "body beta",  "tags": "y"})).await;

            let listed = unwrap_text(list(&c, json!({"tag": "x"})).await);
            assert!(listed.contains("body alpha"));
            assert!(!listed.contains("body beta"));
        }

        #[tokio::test]
        async fn edit_replaces_title_content_and_tags() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            let saved = unwrap_text(save(&c, json!({
                "title":   "Wrong title",
                "content": "wrong body",
                "tags":    "old",
            })).await);
            let id = id_from_save(&saved);

            unwrap_text(edit(&c, json!({
                "id":      id,
                "title":   "Right title",
                "content": "right body",
                "tags":    "new",
            })).await);
            let got = unwrap_text(get(&c, json!({"id": id})).await);
            assert!(got.contains("Right title"));
            assert!(got.contains("right body"));
            assert!(got.contains("new"));
            assert!(!got.contains("Wrong"));
            assert!(!got.contains("wrong body"));
            assert!(!got.contains("old"));
        }

        #[tokio::test]
        async fn edit_can_add_title_to_legacy_entry() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            std::fs::write(
                format!("{}/MEMORY.md", tmp.path().display()),
                "## [mem_legacy] 2026-04-01T00:00:00Z\nbody\n",
            ).unwrap();

            unwrap_text(edit(&c, json!({
                "id":    "mem_legacy",
                "title": "Now titled",
            })).await);

            let got = unwrap_text(get(&c, json!({"id": "mem_legacy"})).await);
            assert!(got.contains("Now titled"), "title not applied: {}", got);
        }

        #[tokio::test]
        async fn delete_removes_entry() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            let saved = unwrap_text(save(&c, json!({"title": "Doomed"})).await);
            let id = id_from_save(&saved);

            unwrap_text(delete(&c, json!({"id": id})).await);
            let err = unwrap_error(get(&c, json!({"id": id})).await);
            assert!(err.contains("not found"));
        }

        #[tokio::test]
        async fn save_fails_loudly_at_entry_cap() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            // Hand-craft MEMORY.md at exactly MAX_ENTRIES (legacy-shape entries are fine).
            let blocks: Vec<MemoryBlock> = (0..MAX_ENTRIES)
                .map(|i| block(&format!("mem_{:08x}", i), "2026-01-01T00:00:00Z", "", &[], "x"))
                .collect();
            std::fs::write(format!("{}/MEMORY.md", tmp.path().display()), serialize_blocks(&blocks)).unwrap();

            let err = unwrap_error(save(&c, json!({"title": "one more"})).await);
            assert!(err.contains("Memory full"));
            assert!(err.contains("compaction"));
        }

        #[tokio::test]
        async fn delete_unknown_id_errors() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            let err = unwrap_error(delete(&c, json!({"id": "mem_nope"})).await);
            assert!(err.contains("not found"));
        }

        #[tokio::test]
        async fn edit_requires_at_least_one_field() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            let saved = unwrap_text(save(&c, json!({"title": "x"})).await);
            let id = id_from_save(&saved);
            let err = unwrap_error(edit(&c, json!({"id": id})).await);
            assert!(err.contains("must provide"));
        }

        #[tokio::test]
        async fn unknown_action_errors() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ctx(&tmp);
            let err = unwrap_error(MemoryTool.execute(json!({"action": "obliterate"}), &c).await);
            assert!(err.contains("unknown action"));
        }
    }
}
