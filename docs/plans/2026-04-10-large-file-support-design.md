# Large File Support for ZenClaw Cloud Storage

**Date:** 2026-04-10
**Status:** Approved

## Problem

ZenClaw needs to work with files up to 10MB+ stored in cloud (S3-compatible) storage. The ESP32-S3 has limited RAM (512KB SRAM + 2-8MB PSRAM), so files cannot be fully buffered. Neither Gemini nor OpenAI can fetch external URLs — content must flow through the ESP32.

Current state: `storage_read` downloads the entire file into RAM, then `context_pruning.py` truncates to 8K chars. A 10MB file would be fully downloaded (crash risk) then 99.9% discarded.

## Design

### Chunked S3 I/O

Add `head()` and `get_range()` to `S3Client` (`lib/s3.py`). S3 natively supports HTTP `Range: bytes=N-M` headers and HEAD requests.

Chunk size: **50KB** per tool result (~12K tokens). The ESP32 can safely buffer 200KB from S3, but 50KB balances memory safety with LLM token efficiency and fewer round-trips.

Raise `MAX_TOOL_RESULT_CHARS` from 8000 to 51200 in `context_pruning.py`.

### New Storage Tools

| Tool | Purpose |
|------|---------|
| `storage_info(key)` | File metadata via HEAD — size, content type, last modified. Zero download. |
| `storage_read_chunk(key, offset, limit)` | Read byte range. Default limit 50KB. Memory-safe for any file size. |
| `storage_grep(key, pattern, context_lines)` | Search pattern in large file. Scans in 50KB chunks. Returns matching lines up to 50KB. |
| `storage_analyze(key)` | Gemini-only. Pipes S3 content to Gemini File API upload, returns file_uri for full-context analysis. |
| `storage_edit(key, old_text, new_text)` | Chunked scan for match, read surrounding region, replace, write back. Phase 2. |
| `storage_transform(source_key, dest_key, transform_type)` | Streaming chunked read → transform → write. Phase 2. |

### Gemini File API Bridge

New module: `agent/providers/gemini_upload.py`

Flow:
1. `storage_analyze` tool initiates resumable upload via `POST /upload/v1beta/files`
2. Streams S3 content to Gemini in 32K chunks (read from S3 → send to upload URL → repeat)
3. Gets back `file_uri`
4. Subsequent messages include `fileData` part referencing the URI
5. Gemini processes full file with 1M token context

Requires modifying `_build_gemini_messages()` in `providers/__init__.py` to emit `fileData` parts.

### Data Flow Examples

**Full-file analysis (Gemini):**
```
storage_info("data/report.csv") → {size: 8MB}
storage_analyze("data/report.csv") → uploads to Gemini via streaming pipe
LLM receives fileData → full analysis in one call
```

**Chunked analysis (OpenAI-compatible):**
```
storage_info("data/report.csv") → {size: 8MB}
storage_read_chunk("data/report.csv", offset=0, limit=51200) → first 50KB
LLM reads, decides if more needed, iterates
```

**Edit:**
```
storage_edit("config/prod.yaml", old_text="v1", new_text="v2")
  → scan chunks until match found
  → read surrounding region, replace, write back
```

**Search:**
```
storage_grep("logs/app.log", pattern="ERROR")
  → scan 50KB chunks, collect matches, return up to 50KB
```

## Implementation Phases

### Phase 1 (Core)

1. `lib/s3.py` — add `head()`, `get_range()` methods
2. `agent/context_pruning.py` — raise `MAX_TOOL_RESULT_CHARS` to 51200
3. `agent/tools/storage_tools.py` — add `storage_info`, `storage_read_chunk`, `storage_grep` tools
4. `agent/providers/gemini_upload.py` — new Gemini File API upload bridge
5. `agent/providers/__init__.py` — `fileData` support in `_build_gemini_messages()`
6. `agent/tools/storage_tools.py` — add `storage_analyze` tool (Gemini upload orchestration)

### Phase 2 (Edits + Transforms)

1. `storage_edit` tool with chunked scan-and-replace
2. `storage_transform` tool with streaming pipeline (grep, head, tail, filter)

## Files Changed

| File | Change |
|------|--------|
| `lib/s3.py` | Add `head()`, `get_range()` |
| `agent/context_pruning.py` | `MAX_TOOL_RESULT_CHARS` → 51200 |
| `agent/tools/storage_tools.py` | 4-5 new tools |
| `agent/providers/gemini_upload.py` | New — Gemini File API streaming upload |
| `agent/providers/__init__.py` | `fileData` parts in `_build_gemini_messages()` |
| `agent/gateway.py` | Pass provider info to storage tools for Gemini detection |
