# Promoting ZenClaw

Working notes on getting the project known. The hook is unusually strong —
**a real LLM agent (tool use, persistent memory, multi-turn) running *on* a
~$10 microcontroller, flashable from the browser, no cloud agent** — so the
strategy is to lead with the "wow" and not waste first impressions.

## 1. The demo is the marketing

Nothing converts like seeing it run. Produce **one 30–60s screen capture**:

1. Open the web provisioning wizard → flash a board from the browser (Web Serial)
2. Talk to the device over Telegram / the web chat
3. Watch it call a tool (web fetch, memory, etc.) and answer

Put that GIF/video at the top of the README and reuse it in every post. The
**browser provisioning flow** (`bennyzen.github.io/zenclaw`) is the single best
asset: *"flash an AI agent onto your ESP32 from your browser in 60 seconds"* is
simultaneously a tweet, a Show HN title, and a Hackaday tip.

## 2. Prerequisites before any launch (one day of polish)

You get one front-page shot per channel, so do these first:

- **README hero**: demo GIF, one-line value prop, 3-step quickstart, a short
  "what's novel" section.
- **Social preview image**: Repo → Settings → Social preview (controls the link card).
- **Description + topics + About website link** pointing at the provisioning page.
  (Proposed topics: `ai-agent`, `llm`, `rust`, `esp32`, `esp32-p4`,
  `embedded-rust`, `esp-idf`, `edge-ai`, `on-device-ai`, `openai-compatible`, …)
- **A tagged release** with prebuilt binaries so "try it now" has zero build friction.

## 3. Channels, ranked

- **Show HN** — e.g. *"Show HN: An LLM agent that runs on a $10 ESP32 (Rust)"*.
  This niche overperforms on HN. Launch Tue–Thu morning US time; be present in comments.
- **Espressif / esp-rs community** — the esp-rs team (e.g. MabezDev) and Espressif
  DevRel amplify cool esp-rs projects. We're already in their orbit via
  [esp-rs/rust#277](https://github.com/esp-rs/rust/issues/277) — confirm 1.96.0.0
  fixes it there, and share in the esp-rs Matrix/Discord. An Espressif repost is
  worth thousands of impressions.
- **Reddit**: r/esp32, r/rust, r/embedded, r/LocalLLaMA (loves the no-cloud angle),
  r/selfhosted. Tailor the title per sub.
- **Hackaday** (submit a tip — they love this) and a **Hackster.io** writeup.
- **lobste.rs** (rust + hardware tags), **Mastodon/fosstodon**, **Bluesky**, **X**.
- **awesome-esp-rust** — open a PR to get listed; it's a real discovery channel.

## 4. Technical depth posts (sustained interest, not just a spike)

Short devlogs on the *hard* parts pull the serious audience and age well as HN /
lobste.rs follow-ups:

- "Running an LLM agent on a microcontroller: the OpenAI-compat pivot"
- "Getting the ESP32-P4 SD card working (the on-chip LDO_VO4 gotcha)"
- "On-device agent memory in 64 KB of text"

## 5. Messaging

Lead with the surprising, concrete claim — not the category:

> "A full AI agent — tools, memory, Telegram — running *on* an ESP32, flashed from
> your browser. No companion app, no cloud agent."

Save "framework for…" framing for the README body, not the headline.

## Suggested launch sequence

1. Polish README + demo GIF + tagged release (day 1–2)
2. Warm up: esp-rs community + awesome-esp-rust PR
3. Show HN + Reddit the same morning
4. Hackaday tip
5. Devlog post ~a week later for a second wave
