# Security Policy

## Reporting a vulnerability

Please report security issues **privately** — do not open a public issue for
anything exploitable.

Use GitHub's [private vulnerability reporting](https://github.com/bennyzen/zenclaw/security/advisories/new)
(Security tab → "Report a vulnerability"). Expect an initial acknowledgement
within about a week. There is no bounty program; ZenClaw is a hobbyist
open-source project.

## Supported versions

Only the `main` branch is supported. Fixes land on `main`; there are no
backports to tagged releases at this time.

## Trust model — please read before deploying

ZenClaw is designed as a **local-network appliance**. The on-device HTTP and
WebSocket API (port 80) is **unauthenticated by design** in the current
release. Anyone who can reach the device on the network can, without any
credential:

- **Read every stored secret in cleartext** via `GET /api/config` — this
  includes your LLM API keys, Telegram bot token, and S3/R2
  `secret_access_key`. The response is **not** redacted.
- **Read and write the device filesystem** via `/api/files*` (access is jailed
  to `/data` and, where present, `/sdcard`).
- **Reboot the device** via `POST /api/restart`.
- **Mint presigned GET/PUT/DELETE URLs** for your configured cloud bucket via
  `/api/cloud/sign`, granting read/write/delete access to that bucket.

In addition, responses are sent with `Access-Control-Allow-Origin: *`, so any
website you visit in a browser on the same network can script these requests
against the device.

**Therefore:**

- Only run ZenClaw on a **trusted network segment** you control.
- **Never** port-forward it, expose it to the public internet, or place it on
  an untrusted / guest VLAN.
- Treat the device as holding your provider and cloud credentials in plaintext.

Optional authentication (shared secret / token) and secret redaction on
`GET /api/config` are planned hardening items. Until then, the protections
above are the boundary of the security model. If your threat model requires
authentication today, place the device behind a reverse proxy that enforces it,
on an isolated network.
