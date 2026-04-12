# Large File Support Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable ZenClaw to work with files up to 10MB+ in cloud (S3) storage via chunked I/O and Gemini File API integration, without exhausting ESP32 RAM.

**Architecture:** Add `head()` and `get_range()` to the S3 client for chunked reads. New storage tools (`storage_info`, `storage_read_chunk`, `storage_grep`) operate on byte ranges. A Gemini File API bridge pipes S3 content directly to Google's upload endpoint for full-file analysis. Tool result size limit raised from 8KB to 50KB.

**Tech Stack:** MicroPython, S3 HTTP Range requests, Gemini File API (`/upload/v1beta/files`)

**Design doc:** `docs/plans/2026-04-10-large-file-support-design.md`

---

### Task 1: Add `head()` method to S3Client

**Files:**
- Modify: `firmware/lib/s3.py`

**Step 1: Add `head()` method**

Add after the `list()` method in `S3Client` (after line 221):

```python
    async def head(self, key):
        path = self._path(key)
        headers = self._sign('HEAD', path)

        from lib.httpclient import request
        resp = await request(self._url(key), method='HEAD', headers=headers, timeout=10000)
        status = resp.get('status', 0)
        if status < 200 or status >= 300:
            if status == 404:
                return {'ok': False, 'status': status, 'error': 'Not found'}
            return {'ok': False, 'status': status, 'error': resp.get('body', b'').decode()}
        resp_headers = resp.get('headers', {})
        size = resp_headers.get('Content-Length', resp_headers.get('content-length', '0'))
        ct = resp_headers.get('Content-Type', resp_headers.get('content-type', ''))
        lm = resp_headers.get('Last-Modified', resp_headers.get('last-modified', ''))
        return {
            'ok': True, 'status': status,
            'size': int(size) if isinstance(size, str) else size,
            'content_type': ct,
            'last_modified': lm,
        }
```

**Step 2: Verify syntax**

Run: `cd firmware && python3 -c "import sys; sys.path.insert(0,'lib'); exec(open('lib/s3.py').read())"`
Expected: no syntax error (the imports will fail but syntax should be clean)

**Step 3: Commit**

```bash
git add firmware/lib/s3.py
git commit -m "feat(s3): add head() method for file metadata without download"
```

---

### Task 2: Add `get_range()` method to S3Client

**Files:**
- Modify: `firmware/lib/s3.py`

**Step 1: Add `get_range()` method**

Add after the `head()` method:

```python
    async def get_range(self, key, offset=0, length=51200):
        path = self._path(key)
        headers = self._sign('GET', path)
        headers['Range'] = 'bytes={}-{}'.format(offset, offset + length - 1)

        from lib.httpclient import get as http_get
        resp = await http_get(self._url(key), headers=headers, timeout=30000)
        status = resp.get('status', 0)
        if status < 200 or status >= 300:
            if status == 404:
                return {'ok': False, 'status': status, 'error': 'Not found'}
            if status == 416:
                return {'ok': False, 'status': status, 'error': 'Range not satisfiable'}
            return {'ok': False, 'status': status, 'error': resp.get('body', b'').decode()}
        body = resp.get('body', b'')
        if isinstance(body, (bytes, bytearray)):
            try:
                body = body.decode()
            except:
                pass
        cr = resp.get('headers', {}).get('Content-Range', resp.get('headers', {}).get('content-range', ''))
        total_size = 0
        if cr and '/' in cr:
            total_size = int(cr.split('/')[1])
        return {'ok': True, 'status': status, 'body': body, 'total_size': total_size, 'chunk_size': len(body)}
```

**Important note about httpclient:** The current `_do_request` in `lib/httpclient.py` returns `headers: {}` (empty dict on line 46). For `get_range()` and `head()` to work fully, `httpclient.py` needs to capture response headers. See Task 3.

**Step 2: Commit**

```bash
git add firmware/lib/s3.py
git commit -m "feat(s3): add get_range() for byte-range reads"
```

---

### Task 3: Capture response headers in httpclient

**Files:**
- Modify: `firmware/lib/httpclient.py`

**Step 1: Update `_do_request` to capture response headers**

In `_do_request()`, the `requests` library response object has `r.headers`. Update the return block (around line 43-47):

Change the return inside the `try` block from:

```python
        return {
            'status': r.status_code,
            'body': body,
            'headers': {}
        }
```

To:

```python
        resp_headers = {}
        try:
            for k, v in r.headers.items():
                resp_headers[k] = v
        except:
            pass
        return {
            'status': r.status_code,
            'body': body,
            'headers': resp_headers
        }
```

**Step 2: Verify no regressions**

Run: `cd firmware && python3 -c "import sys; sys.path.insert(0,'lib'); from httpclient import get; print('ok')"`
Expected: `ok`

**Step 3: Commit**

```bash
git add firmware/lib/httpclient.py
git commit -m "fix(httpclient): capture response headers for S3 range/head support"
```

---

### Task 4: Raise `MAX_TOOL_RESULT_CHARS` to 51200

**Files:**
- Modify: `firmware/agent/context_pruning.py`

**Step 1: Change the constant**

Line 7, change:

```python
MAX_TOOL_RESULT_CHARS = 8000
```

To:

```python
MAX_TOOL_RESULT_CHARS = 51200
```

**Step 2: Commit**

```bash
git add firmware/agent/context_pruning.py
git commit -m "feat: raise tool result limit to 50KB for large file support"
```

---

### Task 5: Add `storage_info` tool

**Files:**
- Modify: `firmware/agent/tools/storage_tools.py`

**Step 1: Add the `_storage_info` tool function**

Inside `create_storage_tools()`, after `_storage_list`, add:

```python
    async def _storage_info(args):
        key = args.get('key', '')
        if not key:
            return 'Error: key is required'

        result = await client.head(key)
        if not result['ok']:
            if result.get('status') == 404:
                return 'Not found: storage:{}'.format(key)
            return 'Error: {} {}'.format(result['status'], result.get('error', ''))

        size = result.get('size', 0)
        if size >= 1048576:
            size_str = '{:.1f} MB'.format(size / 1048576)
        elif size >= 1024:
            size_str = '{:.1f} KB'.format(size / 1024)
        else:
            size_str = '{} B'.format(size)

        lines = ['storage:{}'.format(key)]
        lines.append('  Size: {} ({} bytes)'.format(size_str, size))
        if result.get('content_type'):
            lines.append('  Type: {}'.format(result['content_type']))
        if result.get('last_modified'):
            lines.append('  Modified: {}'.format(result['last_modified']))
        return '\n'.join(lines)
```

**Step 2: Register in the returned dict**

Add to the return dict (before the closing `}`):

```python
        'storage_info': {
            'description': 'Get metadata for a cloud storage file (size, type, modified date) without downloading it.',
            'parameters': {
                'type': 'object',
                'properties': {
                    'key': {
                        'type': 'string',
                        'description': 'Object key / path to inspect',
                    },
                },
                'required': ['key'],
            },
            'execute': _storage_info,
        },
```

**Step 3: Verify tool registration**

Run: `cd firmware && python3 -c "
import sys, json
sys.path.insert(0,'.'); sys.path.insert(0,'lib'); sys.path.insert(0,'stubs')
config = json.load(open('config.json'))
from agent.tools.storage_tools import create_storage_tools
tools = create_storage_tools(config)
print('storage_info' in tools)
"`
Expected: `True` (if storage is configured) or check that the function is syntactically valid

**Step 4: Commit**

```bash
git add firmware/agent/tools/storage_tools.py
git commit -m "feat(storage): add storage_info tool for file metadata via HEAD"
```

---

### Task 6: Add `storage_read_chunk` tool

**Files:**
- Modify: `firmware/agent/tools/storage_tools.py`

**Step 1: Add the `_storage_read_chunk` tool function**

Inside `create_storage_tools()`, after `_storage_info`, add:

```python
    async def _storage_read_chunk(args):
        key = args.get('key', '')
        offset = args.get('offset', 0)
        limit = args.get('limit', 51200)
        if not key:
            return 'Error: key is required'

        result = await client.get_range(key, offset=offset, length=limit)
        if not result['ok']:
            if result.get('status') == 404:
                return 'Not found: storage:{}'.format(key)
            if result.get('status') == 416:
                return 'Offset {} is beyond end of file'.format(offset)
            return 'Error: {} {}'.format(result['status'], result.get('error', ''))

        body = result['body']
        total = result.get('total_size', 0)
        header = '[bytes {}-{}, {} bytes read'
        if total:
            header += ' of {} total]'.format(total)
        else:
            header += ']'.format()
        header = header.format(offset, offset + result['chunk_size'] - 1, result['chunk_size'], total)

        if len(body) > 51200:
            body = body[:51200] + '\n...[truncated at 50KB]'
        return header + '\n' + body
```

Note: the `.format()` call needs fixing for the total branch. Use:

```python
    async def _storage_read_chunk(args):
        key = args.get('key', '')
        offset = args.get('offset', 0)
        limit = args.get('limit', 51200)
        if not key:
            return 'Error: key is required'

        result = await client.get_range(key, offset=offset, length=limit)
        if not result['ok']:
            if result.get('status') == 404:
                return 'Not found: storage:{}'.format(key)
            if result.get('status') == 416:
                return 'Offset {} is beyond end of file'.format(offset)
            return 'Error: {} {}'.format(result['status'], result.get('error', ''))

        body = result['body']
        total = result.get('total_size', 0)
        chunk_len = result['chunk_size']
        end_byte = offset + chunk_len - 1
        if total:
            header = '[bytes {}-{}, {} bytes read of {} total]'.format(offset, end_byte, chunk_len, total)
        else:
            header = '[bytes {}-{}, {} bytes read]'.format(offset, end_byte, chunk_len)

        if len(body) > 51200:
            body = body[:51200] + '\n...[truncated at 50KB]'
        return header + '\n' + body
```

**Step 2: Register in the returned dict**

```python
        'storage_read_chunk': {
            'description': 'Read a byte range from a cloud storage file. Use for large files that should not be read entirely at once. Returns up to 50KB per call. Use storage_info first to get file size, then iterate with offset.',
            'parameters': {
                'type': 'object',
                'properties': {
                    'key': {
                        'type': 'string',
                        'description': 'Object key / path to read',
                    },
                    'offset': {
                        'type': 'integer',
                        'description': 'Byte offset to start reading from (default: 0)',
                    },
                    'limit': {
                        'type': 'integer',
                        'description': 'Maximum bytes to read (default: 51200, max: 51200)',
                    },
                },
                'required': ['key'],
            },
            'execute': _storage_read_chunk,
        },
```

**Step 3: Commit**

```bash
git add firmware/agent/tools/storage_tools.py
git commit -m "feat(storage): add storage_read_chunk for byte-range file reads"
```

---

### Task 7: Add `storage_grep` tool

**Files:**
- Modify: `firmware/agent/tools/storage_tools.py`

**Step 1: Add the `_storage_grep` tool function**

Inside `create_storage_tools()`, after `_storage_read_chunk`, add:

```python
    async def _storage_grep(args):
        key = args.get('key', '')
        pattern = args.get('pattern', '')
        context_lines = args.get('context_lines', 0)
        max_results = args.get('max_results', 50)
        if not key:
            return 'Error: key is required'
        if not pattern:
            return 'Error: pattern is required'

        info = await client.head(key)
        if not info['ok']:
            if info.get('status') == 404:
                return 'Not found: storage:{}'.format(key)
            return 'Error: {} {}'.format(info['status'], info.get('error', ''))

        total_size = info.get('size', 0)
        if total_size == 0:
            return 'File is empty: storage:{}'.format(key)

        chunk_size = 51200
        overlap = len(pattern) + max(0, context_lines) * 256
        matches = []
        offset = 0
        bytes_scanned = 0

        while offset < total_size and len(matches) < max_results:
            read_len = chunk_size + (overlap if offset > 0 else 0)
            result = await client.get_range(key, offset=offset, length=read_len)
            if not result['ok']:
                break

            chunk = result['body']
            if not isinstance(chunk, str):
                try:
                    chunk = chunk.decode('utf-8', 'ignore')
                except:
                    break

            lines = chunk.split('\n')
            for i, line in enumerate(lines):
                if len(matches) >= max_results:
                    break
                if pattern in line:
                    start = max(0, i - context_lines)
                    end = min(len(lines), i + context_lines + 1)
                    for j in range(start, end):
                        prefix = '>> ' if j == i else '   '
                        matches.append('{}{}'.format(prefix, lines[j]))
                    if context_lines > 0:
                        matches.append('---')

            advance = chunk_size
            offset += advance
            bytes_scanned += len(chunk)

            if len(chunk) < read_len:
                break

        if not matches:
            return 'No matches for "{}" in storage:{} (scanned {} of {} bytes)'.format(
                pattern, key, bytes_scanned, total_size)

        output = '\n'.join(matches)
        if len(output) > 51200:
            output = output[:51200] + '\n...[truncated at 50KB, {} matches found]'.format(len(matches))

        return 'Found {} matches in storage:{} (scanned {}/{} bytes):\n{}'.format(
            len(matches), key, bytes_scanned, total_size, output)
```

**Step 2: Register in the returned dict**

```python
        'storage_grep': {
            'description': 'Search for a text pattern in a cloud storage file. Scans in chunks for memory safety. Returns matching lines with optional context.',
            'parameters': {
                'type': 'object',
                'properties': {
                    'key': {
                        'type': 'string',
                        'description': 'Object key / path to search',
                    },
                    'pattern': {
                        'type': 'string',
                        'description': 'Text pattern to search for',
                    },
                    'context_lines': {
                        'type': 'integer',
                        'description': 'Number of context lines before/after each match (default: 0)',
                    },
                    'max_results': {
                        'type': 'integer',
                        'description': 'Maximum number of matches to return (default: 50)',
                    },
                },
                'required': ['key', 'pattern'],
            },
            'execute': _storage_grep,
        },
```

**Step 3: Commit**

```bash
git add firmware/agent/tools/storage_tools.py
git commit -m "feat(storage): add storage_grep for chunked pattern search in large files"
```

---

### Task 8: Create Gemini File API upload bridge

**Files:**
- Create: `firmware/agent/providers/gemini_upload.py`

**Step 1: Create the module**

```python
"""Gemini File API upload bridge — streams S3 content to Google's upload endpoint."""

import json
from lib.sys.log import log

GEMINI_UPLOAD_INIT = 'https://generativelanguage.googleapis.com/upload/v1beta/files'
CHUNK_SIZE = 32768


async def upload_from_s3(s3_client, key, api_key, mime_type='text/plain'):
    """Stream an S3 object to Gemini's File API. Returns file_uri on success."""
    from lib.httpclient import post, request

    info = await s3_client.head(key)
    if not info['ok']:
        return {'ok': False, 'error': 'S3 head failed: {}'.format(info.get('error', ''))}

    total_size = info.get('size', 0)
    file_size_header = str(total_size)

    headers = {
        'X-Goog-Upload-Protocol': 'resumable',
        'X-Goog-Upload-Command': 'start',
        'X-Goog-Upload-Header-Content-Length': file_size_header,
        'X-Goog-Upload-Header-Content-Type': mime_type,
        'Content-Type': 'application/json',
    }
    metadata = json.dumps({'file': {'display_name': key}}).encode()

    init_url = '{}?key={}'.format(GEMINI_UPLOAD_INIT, api_key)
    resp = await post(init_url, data=metadata, headers=headers, timeout=30000)

    upload_url = resp.get('headers', {}).get('X-Goog-Upload-URL',
                   resp.get('headers', {}).get('x-goog-upload-url', ''))
    if not upload_url:
        return {'ok': False, 'error': 'No upload URL in response. Headers: {}'.format(resp.get('headers', {}))}

    log('info', 'GEMINI-UPLOAD: streaming {} ({} bytes) to upload URL'.format(key, total_size), source='zenclaw')

    offset = 0
    while offset < total_size:
        read_len = min(CHUNK_SIZE, total_size - offset)
        range_result = await s3_client.get_range(key, offset=offset, length=read_len)
        if not range_result['ok']:
            return {'ok': False, 'error': 'S3 range read failed at offset {}: {}'.format(offset, range_result.get('error', ''))}

        chunk_data = range_result['body']
        if isinstance(chunk_data, str):
            chunk_data = chunk_data.encode('utf-8')

        is_final = (offset + read_len) >= total_size
        cmd = 'upload, finalize' if is_final else 'upload'

        chunk_headers = {
            'Content-Length': str(len(chunk_data)),
            'X-Goog-Upload-Offset': str(offset),
            'X-Goog-Upload-Command': cmd,
        }

        resp = await request(upload_url, method='POST', data=chunk_data, headers=chunk_headers, timeout=60000)

        offset += read_len

        if is_final:
            body = resp.get('body', b'')
            if isinstance(body, (bytes, bytearray)):
                body = body.decode('utf-8', 'replace')
            try:
                file_data = json.loads(body)
                file_uri = file_data.get('file', {}).get('uri', '')
                file_id = file_data.get('file', {}).get('name', '')
                mime = file_data.get('file', {}).get('mimeType', mime_type)
                if not file_uri:
                    return {'ok': False, 'error': 'No file_uri in upload response: {}'.format(body[:500])}
                return {'ok': True, 'file_uri': file_uri, 'file_id': file_id, 'mime_type': mime, 'size': total_size}
            except Exception as e:
                return {'ok': False, 'error': 'Failed to parse upload response: {} - {}'.format(e, body[:300])}

    return {'ok': False, 'error': 'Upload loop exited without finalizing'}
```

**Step 2: Commit**

```bash
git add firmware/agent/providers/gemini_upload.py
git commit -m "feat(providers): add Gemini File API upload bridge for streaming S3->Google"
```

---

### Task 9: Add `fileData` support to Gemini message builder

**Files:**
- Modify: `firmware/agent/providers/__init__.py`

**Step 1: Handle `_file_data` entries in `_build_gemini_messages()`**

In `_build_gemini_messages()`, after the `isinstance(content, list)` block (around line 169), add handling for messages that carry `_file_data`:

Inside the main `for msg in messages:` loop, before the `if isinstance(content, list):` check (line 169), add:

```python
        if msg.get('_file_data'):
            parts = []
            if content:
                parts.append({'text': content})
            for fd in msg['_file_data']:
                parts.append({'fileData': {'fileUri': fd['file_uri'], 'mimeType': fd.get('mime_type', 'text/plain')}})
            contents.append({'role': gemini_role, 'parts': parts})
            continue
```

This means messages can carry `_file_data` entries (list of `{file_uri, mime_type}` dicts) alongside text content, and they'll be rendered as Gemini `fileData` parts.

**Step 2: Commit**

```bash
git add firmware/agent/providers/__init__.py
git commit -m "feat(providers): support fileData parts in Gemini message builder"
```

---

### Task 10: Add `storage_analyze` tool (Gemini upload orchestration)

**Files:**
- Modify: `firmware/agent/tools/storage_tools.py`

**Step 1: Add the `_storage_analyze` tool function**

The tool needs access to the Gemini API key. The `create_storage_tools` function receives `config`, which contains `providers.google.api_key`. Add inside `create_storage_tools()`:

```python
    gemini_key = config.get('providers', {}).get('google', {}).get('api_key', '')

    async def _storage_analyze(args):
        key = args.get('key', '')
        if not key:
            return 'Error: key is required'
        if not gemini_key:
            return 'Error: storage_analyze requires a Gemini API key (providers.google.api_key)'

        from .providers.gemini_upload import upload_from_s3

        info = await client.head(key)
        if not info['ok']:
            return 'Error: {}'.format(info.get('error', 'File not found'))

        ct = info.get('content_type', 'text/plain')
        if not ct or ct == 'application/octet-stream':
            if key.endswith('.csv'):
                ct = 'text/csv'
            elif key.endswith('.json'):
                ct = 'application/json'
            elif key.endswith('.md'):
                ct = 'text/markdown'
            elif key.endswith('.pdf'):
                ct = 'application/pdf'
            elif key.endswith('.txt'):
                ct = 'text/plain'
            else:
                ct = 'text/plain'

        result = await upload_from_s3(client, key, gemini_key, mime_type=ct)
        if not result['ok']:
            return 'Upload failed: {}'.format(result['error'])

        return 'File uploaded for analysis.\nKey: storage:{}\nSize: {} bytes\nFile URI: {}\n\nYou can now ask questions about this file. It has been loaded into context.'.format(
            key, result['size'], result['file_uri'])
```

Wait — the import path is wrong. `storage_tools.py` is in `agent/tools/`, the provider is in `agent/providers/`. The correct relative import would be `from ..providers.gemini_upload import upload_from_s3`. But the CLAUDE.md says tools use relative imports for sibling modules. Let me check...

Actually, `storage_tools.py` is in `agent/tools/` and `gemini_upload.py` is in `agent/providers/`. The relative import from tools to providers is `from ..providers.gemini_upload import ...`. However, `create_storage_tools` is a factory function and the import can be done lazily inside the function to avoid circular imports. Let me use absolute import:

```python
    async def _storage_analyze(args):
        key = args.get('key', '')
        if not key:
            return 'Error: key is required'
        if not gemini_key:
            return 'Error: storage_analyze requires a Gemini API key (providers.google.api_key)'

        try:
            from agent.providers.gemini_upload import upload_from_s3
        except ImportError:
            return 'Error: gemini_upload module not available'

        info = await client.head(key)
        if not info['ok']:
            return 'Error: {}'.format(info.get('error', 'File not found'))

        ct = info.get('content_type', 'text/plain')
        if not ct or ct == 'application/octet-stream':
            if key.endswith('.csv'):
                ct = 'text/csv'
            elif key.endswith('.json'):
                ct = 'application/json'
            elif key.endswith('.md'):
                ct = 'text/markdown'
            elif key.endswith('.pdf'):
                ct = 'application/pdf'
            else:
                ct = 'text/plain'

        result = await upload_from_s3(client, key, gemini_key, mime_type=ct)
        if not result['ok']:
            return 'Upload failed: {}'.format(result['error'])

        return 'File uploaded for analysis.\nKey: storage:{}\nSize: {} bytes\nFile URI: {}\n\nYou can now ask questions about this file. It has been loaded into context.'.format(
            key, result['size'], result['file_uri'])
```

**But there's a problem:** The `storage_analyze` tool returns a string, but the uploaded `file_uri` needs to be injected into subsequent messages as a `fileData` part. This requires coordination with the agent loop. Two options:

**Option A (simple):** The tool returns the `file_uri` in the result string, and we add a `_file_data` field to the tool result message in the agent loop. This requires the tool to return structured data, not just a string.

**Option B (tool returns dict):** Change the tool to return a dict with special keys that the agent loop recognizes. For example, `{'_file_data': [...], '_result_text': '...'}`.

**Go with Option B** — it's cleaner. Update `agent_loop.py` to detect `_file_data` in tool results:

In `agent_loop.py`, `_execute_tool_calls()`, after the result is obtained (around line 148), add:

```python
            # Check for file_data injection
            file_data = None
            if isinstance(result, dict):
                file_data = result.get('_file_data')
                result = result.get('_result_text', str(result))
```

Then when building `tool_msg`:

```python
        tool_msg = {'role': 'tool', 'tool_call_id': tc['id'], 'content': result}
        if file_data:
            tool_msg['_file_data'] = file_data
```

And the `_storage_analyze` tool returns:

```python
        return {
            '_result_text': 'File uploaded for analysis.\nKey: storage:{}\nSize: {} bytes\n\nYou can now ask questions about this file.'.format(key, result['size']),
            '_file_data': [{'file_uri': result['file_uri'], 'mime_type': result['mime_type']}],
        }
```

**Step 2: Update `agent/tools/__init__.py` execute method**

In `ZenClawTools.execute()`, line 116, change:

```python
            result_str = str(result) if result else '(empty)'
```

To handle dict results that have `_result_text`:

```python
            if isinstance(result, dict) and '_result_text' in result:
                result_str = result['_result_text']
            else:
                result_str = str(result) if result else '(empty)'
```

**Step 3: Register the tool in the returned dict**

```python
        'storage_analyze': {
            'description': 'Upload a cloud storage file to Gemini for full analysis. Streams the file from S3 to Google without loading it entirely into memory. After upload, the file is available for questions in the conversation. Gemini only.',
            'parameters': {
                'type': 'object',
                'properties': {
                    'key': {
                        'type': 'string',
                        'description': 'Object key / path to analyze',
                    },
                },
                'required': ['key'],
            },
            'execute': _storage_analyze,
        },
```

**Step 4: Commit**

```bash
git add firmware/agent/tools/storage_tools.py firmware/agent/tools/__init__.py firmware/agent/agent_loop.py
git commit -m "feat(storage): add storage_analyze tool with Gemini File API upload"
```

---

### Task 11: Update SOUL.md with new tool descriptions

**Files:**
- Modify: `firmware/data/SOUL.md` (if it contains tool guidance)

**Step 1: Check if SOUL.md mentions storage tools**

Read `firmware/data/SOUL.md` and add guidance about the new tools if appropriate. This is optional — the tool descriptions are self-documenting.

---

### Task 12: Integration test with smoke tests

**Files:**
- Modify: `firmware/test_tools.py`

**Step 1: Add smoke test entries**

The existing smoke tests only run if the tool is registered (storage tools require config). Add conditional tests at the end of the TESTS list:

```python
    # ===== Storage tools (require S3 config) =====
    ('storage_list', {'prefix': ''},
     'storage list works',
     lambda r: r is not None),

    ('storage_write', {'key': '_test/hello.txt', 'content': 'hello from zenclaw test'},
     'storage write test file',
     'Wrote'),

    ('storage_info', {'key': '_test/hello.txt'},
     'storage info returns metadata',
     'Size:', 'bytes'),

    ('storage_read_chunk', {'key': '_test/hello.txt', 'offset': 0, 'limit': 1024},
     'storage read chunk returns content',
     'hello from zenclaw'),

    ('storage_grep', {'key': '_test/hello.txt', 'pattern': 'zenclaw'},
     'storage grep finds pattern',
     'zenclaw', 'match'),

    ('storage_delete', {'key': '_test/hello.txt'},
     'storage cleanup test file',
     'Deleted'),
```

**Step 2: Run smoke tests**

Run: `cd firmware && micropython -X heapsize=4m test_tools.py`
Expected: Storage tests PASS if S3 is configured, SKIP if not.

**Step 3: Commit**

```bash
git add firmware/test_tools.py
git commit -m "test: add smoke tests for storage_info, storage_read_chunk, storage_grep"
```

---

### Task 13: End-to-end verification

**Step 1: Run full tool smoke test suite**

Run: `cd firmware && micropython -X heapsize=4m test_tools.py`
Expected: All existing tests still pass, new storage tests pass (or skip gracefully).

**Step 2: Verify context_pruning change doesn't break existing behavior**

Run: `cd firmware && python3 -c "
import sys; sys.path.insert(0,'.'); sys.path.insert(0,'agent')
from context_pruning import soft_trim_tool_result
msg = {'role': 'tool', 'tool_call_id': 'test', 'content': 'x' * 60000}
trimmed = soft_trim_tool_result(msg)
print('Original: {} chars'.format(len(msg['content'])))
print('Trimmed: {} chars'.format(len(trimmed['content'])))
assert len(trimmed['content']) == 51200 + len('...[truncated]'), 'Unexpected trim length'
print('PASS')
"`
Expected: PASS

**Step 3: Verify S3 head/get_range with Python stdlib (desktop)**

This requires S3 credentials configured in config.json. Manual test:

```bash
cd firmware && python3 -c "
import sys, json, asyncio
sys.path.insert(0,'.'); sys.path.insert(0,'lib'); sys.path.insert(0,'stubs')
config = json.load(open('config.json'))
from lib.s3 import S3Client
storage = config.get('storage', {})
if storage.get('access_key_id'):
    c = S3Client(endpoint=storage['endpoint'], access_key=storage['access_key_id'],
                 secret_key=storage['secret_access_key'], bucket=storage['bucket'], region=storage.get('region','auto'))
    async def test():
        info = await c.head('nonexistent-test-key')
        print('HEAD result:', info)
    asyncio.run(test())
else:
    print('No storage configured, skip')
"
```

---

## Summary

| Task | Component | Est. Time |
|------|-----------|-----------|
| 1 | S3 `head()` | 5 min |
| 2 | S3 `get_range()` | 5 min |
| 3 | httpclient header capture | 5 min |
| 4 | Raise MAX_TOOL_RESULT_CHARS | 2 min |
| 5 | `storage_info` tool | 5 min |
| 6 | `storage_read_chunk` tool | 10 min |
| 7 | `storage_grep` tool | 15 min |
| 8 | Gemini upload bridge | 20 min |
| 9 | fileData in message builder | 10 min |
| 10 | `storage_analyze` tool + agent loop changes | 15 min |
| 11 | SOUL.md update | 5 min |
| 12 | Smoke tests | 10 min |
| 13 | E2E verification | 10 min |

**Total: ~2 hours**
