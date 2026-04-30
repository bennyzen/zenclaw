#!/usr/bin/env python3
"""Drive the desktop ZenClaw agent through a 50-turn scripted session and
record per-turn metrics so we can see where prolonged sessions hurt.

Pre-req: a desktop agent running at the given --host (default
localhost:8080), seeded with a working config.json. Start it with:
  cd ~/zenclaw-desktop && /path/to/zenclaw-agent  2> /tmp/zenclaw-agent.log

For each turn:
  1. snapshot   /api/status + JSONL stats
  2. POST       /api/chat
  3. snapshot   /api/status + JSONL stats
  4. compute    rss_delta, session_bytes_delta, tool_calls_in_turn,
                total_session_messages, est_tokens_total

Output: a CSV trace + verbose unbuffered stdout so you can follow along.
The CSV is the artifact you eyeball after the run -- turn-by-turn deltas
are where session bloat shows up.
"""

import argparse
import csv
import json
import os
import sys
import time
import urllib.error
import urllib.request

# 50 prompts spanning every diversity dimension the optimization arc
# is meant to address (see project_handover_optimization_arc.md):
# context bloat, error feedback loops, cost runaway, state desync.
#
# Order matters in places:
#   - turns 6/10/15/20/27 grow scratch.log incrementally
#   - turn 20 stores a "secret phrase" memory; turn 34 asks for it back
#     (14-turn spaced-recall canary for the c42cf6e persistence fix)
#   - turns 29/30/31 hammer the same nonexistent file three times in a
#     row (loop-detector canary)
#   - turn 40 fetches RFC 9110 (~491 KB) -- the post-cap_or_refuse stress
#     case for web_fetch
DEFAULT_PROMPTS = [
    # 1-5: warm-up, files + memory
    "List the files in your data directory and summarize what each one is for.",
    "Read SOUL.md and tell me in two sentences what character it describes.",
    "Read AGENTS.md and tell me the top three constraints it places on you.",
    "Save a memory under the key 'synthetic_test' with the value 'session started 2026-04-30, 50 turns planned'.",
    "Search your memory for 'synthetic'. What did you find?",
    # 6-10: scratch.log incremental, web warm-up
    "Append the line 'turn 6: heartbeat' to a file called scratch.log (create it if missing).",
    "Read scratch.log and tell me what's in it.",
    "Fetch https://example.com and summarize the page in one sentence.",
    "Search the web for 'rust 2024 edition release notes' and tell me the top result.",
    "Append the line 'turn 10: still alive' to scratch.log, then read the whole file back.",
    # 11-15: agent self-inspection, more memory, list-diff
    "Use your session tool to inspect the current session status. How many messages are in it so far?",
    "Save a memory under 'synthetic_progress' with value 'reached turn 12 without errors yet'.",
    "Search your memory for 'progress'. List everything you find.",
    "List the files in your data directory again -- has anything changed since turn 1?",
    "Read scratch.log one more time and tell me how many lines it has.",
    # 16-20: web JSON, web search, lessons memory, SECRET PHRASE STORE
    "Fetch https://httpbin.org/json and tell me what fields the response object has.",
    "Search the web for 'esp32-p4 rust embedded' and summarize what you learn in two sentences.",
    "Save a memory under 'synthetic_lessons' with whatever you've learned so far in the session -- keep it brief.",
    "Search your memory for 'lessons'. What's there?",
    "Save a memory under the key 'secret_phrase' with the exact value 'periwinkle plumber'. I will ask you for this later.",
    # 21-23: cron lifecycle
    "Use the cron tool to schedule a task named 'ping_test' to run in one hour. Use whatever command/payload makes sense.",
    "List all currently scheduled cron tasks.",
    "Remove the 'ping_test' cron you just scheduled.",
    # 24-26: storage lifecycle
    "Use the storage tool to write a small object: key='note1', value='hello from turn 24'.",
    "List everything in storage.",
    "Read the storage value back for key 'note1'.",
    # 27-28: file edit, session history
    "Edit scratch.log: insert the line '== HEADER (inserted turn 27) ==' as the very first line of the file.",
    "Use the session tool to fetch the recent history of this conversation. Roughly how many entries does it contain?",
    # 29-31: ADVERSARIAL -- nonexistent file three times in a row
    "Try to read a file that does not exist: nonexistent_xyz.md. What happens?",
    "Try to read nonexistent_xyz.md again. Does anything realize you already failed once?",
    "Try to read nonexistent_xyz.md a third time. Be precise about whether the agent is now repeating itself or has changed strategy.",
    # 32-33: aftermath of adversarial, repeat fetch
    "Save a memory under 'synthetic_errors' that describes what just happened with the missing file across the last three turns.",
    "Fetch https://example.com one more time. Compare it to the first fetch on turn 8 -- same content?",
    # 34-35: SPACED RECALL -- the canary for the persistence fix
    "What was the value I asked you to store under the memory key 'secret_phrase' back on turn 20? Use the memory tool to retrieve it.",
    "Save a memory under 'recall_check' describing whether you recalled the secret phrase correctly on the previous turn.",
    # 36: COMPOUND multi-tool turn (single user prompt -> many tool calls)
    "Find every .md file in your data directory, read each one, and save a one-line summary of each as a separate memory entry (key like 'summary_<filename>'). Then list the keys you saved.",
    # 37: real-sized file read
    "Read AGENTS.md in full and report its byte length and how many distinct lines it contains.",
    # 38-41: web ramp -- medium then large then a follow-up that depends on prior turn's payload
    "Fetch https://www.rfc-editor.org/rfc/rfc9111.txt and tell me what RFC number this is and what its title is.",
    "Search the web for 'z.ai glm-5.1 pricing' and report what you find.",
    "Fetch https://www.rfc-editor.org/rfc/rfc9110.txt -- this is ~491 KB. Tell me the document title and the section names you can identify.",
    "Summarize what you just fetched on the previous turn in two sentences without re-fetching it.",
    # 42-43: session inspection mid-late, memory cross-search
    "Use the session tool again to check current session status. How has it grown since turn 11?",
    "Search your memory for 'synthetic'. List every key/value pair you find.",
    # 44: scratch.log final read
    "Read scratch.log and tell me the last three lines.",
    # 45: AMBIGUOUS prompt -- should clarify or refuse cleanly, not invent
    "Do the thing we discussed earlier.",
    # 46-47: more search, final lessons memory
    "Search the web for 'ggml quantization formats' and summarize the top result.",
    "Save a memory under 'synthetic_final_lessons' with what you've learned this session about the agent's tools, error recovery, and state.",
    # 48-49: closing inspection
    "List the files in your data directory one final time. What's there now that wasn't on turn 1?",
    "Use the session tool to give a final report on this session: total messages, total tool calls, anything notable.",
    # 50: meta-summary
    "Summarize this entire 50-turn session in three sentences: what tools you used most, what worked, what didn't.",
]

assert len(DEFAULT_PROMPTS) == 50, f"expected 50 prompts, got {len(DEFAULT_PROMPTS)}"


def http_get_json(url, timeout):
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return json.loads(r.read().decode("utf-8"))


def http_post_json(url, body, timeout):
    data = json.dumps(body).encode("utf-8")
    req = urllib.request.Request(
        url, data=data, headers={"Content-Type": "application/json"}, method="POST"
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode("utf-8"))


def status_snapshot(host, timeout):
    try:
        data = http_get_json(f"http://{host}/api/status", timeout)
        mem = data.get("memory") or {}
        return {
            "rss_kb": mem.get("used_kb"),
            "free_kb": mem.get("free_kb"),
            "rss_peak_kb": mem.get("rss_peak_kb"),
            "uptime_s": data.get("uptime_s"),
        }
    except Exception as e:
        return {"rss_kb": None, "free_kb": None, "rss_peak_kb": None, "uptime_s": None, "error": str(e)}


def session_stats(data_dir, chat_id):
    """Walk the JSONL once and return (file_size, total_messages,
    total_tool_messages, total_content_bytes). Returns Nones on missing file."""
    path = os.path.join(data_dir, "sessions", f"{chat_id}.jsonl")
    try:
        size = os.path.getsize(path)
    except OSError:
        return None, None, None, None

    n_msg = 0
    n_tool = 0
    content_bytes = 0
    try:
        with open(path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    e = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if e.get("type") != "message":
                    continue
                n_msg += 1
                if e.get("role") == "tool":
                    n_tool += 1
                content = e.get("content")
                if isinstance(content, str):
                    content_bytes += len(content)
                elif isinstance(content, list):
                    # MessageContent::Parts -- best-effort byte count
                    content_bytes += len(json.dumps(content))
                tc = e.get("tool_calls")
                if tc:
                    content_bytes += len(json.dumps(tc))
    except OSError:
        return size, None, None, None

    return size, n_msg, n_tool, content_bytes


def trim_for_log(s, n=100):
    """Single-line preview suitable for follow-along output."""
    if not s:
        return ""
    s = s.replace("\r", " ").replace("\n", " ")
    return s if len(s) <= n else s[: n - 1] + "…"


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--host", default="127.0.0.1:8080", help="agent host:port")
    ap.add_argument("--chat-id", default="synthetic", help="chat_id to use (sessions land in data/sessions/<id>.jsonl)")
    ap.add_argument("--data-dir", default=os.path.expanduser("~/zenclaw-desktop/data"), help="data dir for session-size tracking")
    ap.add_argument("--turns", type=int, default=len(DEFAULT_PROMPTS), help="how many prompts to fire (max len of prompt list)")
    ap.add_argument("--turn-timeout", type=float, default=180.0, help="per-turn HTTP timeout (seconds)")
    ap.add_argument("--output", default="session-trace.csv", help="CSV path to write the trace")
    ap.add_argument("--fresh", action="store_true", help="delete data/sessions/<chat_id>.jsonl before starting (clean baseline)")
    ap.add_argument("--quiet", action="store_true", help="suppress per-turn stdout chatter")
    args = ap.parse_args()

    prompts = DEFAULT_PROMPTS[: args.turns]

    if args.fresh:
        path = os.path.join(args.data_dir, "sessions", f"{args.chat_id}.jsonl")
        try:
            os.remove(path)
            print(f"Wiped {path}", flush=True)
        except FileNotFoundError:
            pass
        except OSError as e:
            print(f"WARN: could not wipe {path}: {e}", flush=True)

    # Sanity-check the agent is up before starting.
    boot = status_snapshot(args.host, timeout=5)
    if boot.get("error"):
        print(f"FAIL: agent not reachable at {args.host}: {boot['error']}", file=sys.stderr, flush=True)
        return 2

    if not args.quiet:
        print(
            f"Agent up. Boot RSS={boot['rss_kb']}KB free={boot['free_kb']}KB. "
            f"Running {len(prompts)} turns -> {args.output}",
            flush=True,
        )

    fields = [
        "turn", "prompt_len", "reply_len",
        "latency_ms",
        "rss_before_kb", "rss_after_kb", "rss_delta_kb",
        "free_before_kb", "free_after_kb",
        "session_bytes_before", "session_bytes_after", "session_delta_bytes",
        "total_session_messages", "tool_calls_in_turn", "est_tokens_total",
        "ok", "error",
    ]
    with open(args.output, "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()

        prev_tool_msgs = 0
        run_start = time.monotonic()

        for idx, prompt in enumerate(prompts, start=1):
            before_status = status_snapshot(args.host, timeout=5)
            sess_size_b, _, before_tool_msgs, _ = session_stats(args.data_dir, args.chat_id)
            if before_tool_msgs is not None:
                prev_tool_msgs = before_tool_msgs

            if not args.quiet:
                print(f"\n[turn {idx:2d}/{len(prompts)}] -> {trim_for_log(prompt)}", flush=True)

            t0 = time.monotonic()
            ok = True
            err = ""
            reply = ""
            try:
                resp = http_post_json(
                    f"http://{args.host}/api/chat",
                    {"message": prompt, "chat_id": args.chat_id},
                    timeout=args.turn_timeout,
                )
                reply = resp.get("reply") or ""
            except urllib.error.HTTPError as e:
                ok = False
                err = f"HTTP {e.code}: {e.read().decode('utf-8', 'replace')[:200]}"
            except Exception as e:
                ok = False
                err = str(e)
            latency_ms = int((time.monotonic() - t0) * 1000)

            after_status = status_snapshot(args.host, timeout=5)
            sess_size_a, total_msgs_a, tool_msgs_a, content_bytes_a = session_stats(args.data_dir, args.chat_id)

            # When auto-compaction fires, the JSONL is rewritten and the
            # post-turn tool_msgs count can be lower than pre-turn. Clamp at 0
            # so the per-turn metric stays interpretable; a negative value
            # would just mean "compaction discarded N tool messages this turn"
            # which we surface separately via session_delta_bytes anyway.
            tool_calls_this_turn = (
                max(0, tool_msgs_a - prev_tool_msgs)
                if (tool_msgs_a is not None) else None
            )
            est_tokens_total = (content_bytes_a // 4) if content_bytes_a is not None else None

            row = {
                "turn": idx,
                "prompt_len": len(prompt),
                "reply_len": len(reply),
                "latency_ms": latency_ms,
                "rss_before_kb": before_status["rss_kb"],
                "rss_after_kb": after_status["rss_kb"],
                "rss_delta_kb": (after_status["rss_kb"] - before_status["rss_kb"])
                    if (before_status["rss_kb"] is not None and after_status["rss_kb"] is not None) else None,
                "free_before_kb": before_status["free_kb"],
                "free_after_kb": after_status["free_kb"],
                "session_bytes_before": sess_size_b,
                "session_bytes_after": sess_size_a,
                "session_delta_bytes": (sess_size_a - sess_size_b)
                    if (sess_size_b is not None and sess_size_a is not None) else None,
                "total_session_messages": total_msgs_a,
                "tool_calls_in_turn": tool_calls_this_turn,
                "est_tokens_total": est_tokens_total,
                "ok": ok,
                "error": err,
            }
            writer.writerow(row)
            f.flush()

            if not args.quiet:
                tag = "OK  " if ok else "FAIL"
                rss_d = row["rss_delta_kb"]
                rss_d_s = f"{rss_d:+d}KB" if rss_d is not None else "?KB"
                sess_d = row["session_delta_bytes"]
                sess_d_s = f"{sess_d:+d}B" if sess_d is not None else "?B"
                tools_s = (
                    f"{tool_calls_this_turn}" if tool_calls_this_turn is not None else "?"
                )
                msgs_s = f"{total_msgs_a}" if total_msgs_a is not None else "?"
                tok_s = f"{est_tokens_total}" if est_tokens_total is not None else "?"
                print(
                    f"            <- [{tag}] {trim_for_log(reply)}",
                    flush=True,
                )
                print(
                    f"            ~~  latency={latency_ms}ms reply={len(reply)}B "
                    f"tools={tools_s} sess={sess_d_s} (total {sess_size_a}B) "
                    f"msgs={msgs_s} tok~={tok_s} rss={rss_d_s}"
                    + (f" err={trim_for_log(err, 120)}" if err else ""),
                    flush=True,
                )

            prev_tool_msgs = tool_msgs_a if tool_msgs_a is not None else prev_tool_msgs

    final = status_snapshot(args.host, timeout=5)
    drift_kb = (final["rss_kb"] - boot["rss_kb"]) if (final["rss_kb"] is not None and boot["rss_kb"] is not None) else None
    elapsed = int(time.monotonic() - run_start)
    print()
    print(
        f"Done. {len(prompts)} turns in {elapsed}s. "
        f"RSS drift: {drift_kb} KB. RSS peak: {final.get('rss_peak_kb')} KB. "
        f"Trace: {args.output}",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
