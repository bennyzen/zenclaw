# Model presets fast-dial — design

**Date:** 2026-05-02
**Status:** Approved, ready for plan
**Scope:** UI-only change to `web/app/pages/config.vue`

## Problem

The `providers` config block in ZenClaw has always been a map keyed by name
(`providers.google`, `providers.openai`, …) with a `default` field naming the
active one. The Rust resolver (`agent/src/config.rs:75`) and its MicroPython
predecessor both look up `providers[default]`, so the on-disk format trivially
supports several preconfigured provider+model+key combinations side by side.

The web UI has never exposed this. `web/app/pages/config.vue` loads only
`providers[providers.default]` into a single set of form fields, so swapping
between, say, Gemini Flash and GPT-4o-mini means re-entering the API key, model
name, and base URL by hand each time. There is no fast switch.

This spec adds a "fast dial" — saved provider+model+endpoint combinations
rendered as cards on the config page, one click to switch.

## Goals

- One-click switch between previously-saved provider configurations.
- Persistence on the device (the device needs the credentials to make calls).
- No backend changes — the existing `/api/config` PUT does the job.
- No new top-level pages or chrome on other pages.

## Non-goals

- Custom labels / renaming. Cards are identified by their fingerprint
  (`provider + model + endpoint`).
- Multiple presets with the same fingerprint but different API keys.
- Reordering cards. Alphabetical by slug is fine for the expected ~5 entries.
- Hot-swapping the active provider without a reboot. Switching reuses the
  existing `/api/config` PUT path, which writes NVS and reboots the device
  (~12s of downtime). Sub-second swap is a possible later evolution but is
  out of scope here.

## Storage model

Each preset is a `providers.{slug}` entry. The slug is derived from the form
values:

```
slug = "{providerName}__{slugifiedModel}"
```

`slugifiedModel` lowercases the model string and replaces any character not in
`[a-z0-9]` with `-`, collapsing runs and trimming leading/trailing hyphens.
Example: `openai__gpt-4o-mini`, `google__gemini-2-5-flash`.

`providers.default` continues to name the active slug. The Rust resolver
already does a hashmap lookup with arbitrary string keys, so it accepts both
legacy keys (`google`) and new slugs (`google__gemini-2-5-flash`) without
changes.

**Migration / backward compat.** Existing configs (`providers.google`,
`providers.default = "google"`) keep working. They render as a single card
labelled with the existing key. New saves use the slugged key. Old keys are
not rewritten — they age out naturally as the user updates models. No
migration code is needed.

## UI

Inside the existing **LLM Provider** tab of `config.vue`, below the form
fields and the page-level **Save Config** button, add a new section:

```
─── Saved Models ───────────────────────────────────
┌─────────────────────────────────┐  ┌────────────────────────────┐
│ ● openai                        │  │   anthropic                │
│   gpt-4o-mini                   │  │   claude-haiku-4-5         │
│   api.openai.com/v1          × │  │   api.anthropic.com    × │
└─────────────────────────────────┘  └────────────────────────────┘
```

- Cards render from `Object.entries(rawConfig.providers).filter(([k]) => k !== 'default')`.
- Each card displays three lines: **provider name**, **model**, **endpoint** (base_url, with leading `https://` stripped for compactness).
- The active card (`slug === providers.default`) gets a leading status dot and an accent border.
- API key is **not** shown on the card. Identity is provider+model+endpoint; the key is sensitive and orthogonal.
- The active card's `×` delete button is disabled (would orphan `default`).
- The section renders whenever `rawConfig.providers` has at least one entry beyond the `default` field — i.e. as soon as a config has been saved at least once. The active preset still appears as its own (non-deletable) card; this gives the user a visual mirror of current state. The section is only hidden on a freshly-loaded device with no provider configured at all.

## Save flow

The existing **Save Config** button is the only save path. Modify the provider
section of `save()` so the entry is written under the slug derived from the
form values, and `providers.default` is set to that slug:

```ts
const slug = `${providerName.value}__${slugify(model.value)}`
prov.default = slug
prov[slug] = { api_key: apiKey.value, model: model.value, base_url: baseUrl.value }
```

Existing entries under other slugs are preserved because `save()` already
spreads `rawConfig.value` first. Saving the same form values twice overwrites
the same slot (intended dedup behavior).

The existing reboot semantics of `/api/config` PUT apply unchanged.

## Switch flow

1. User clicks a non-active card body.
2. Confirmation modal opens:

   > **Switch to openai / gpt-4o-mini?**
   > The device will reboot to apply the new model (~12 seconds offline).
   >
   > [Cancel] [Switch & Reboot]

3. On confirm, PUT `/api/config` with the current `rawConfig` but
   `providers.default` set to the clicked slug. The form fields are not
   touched; the existing reboot path runs.
4. The existing `ConnectionBanner` handles the disconnect/reconnect cycle.
   The page does not need a custom "rebooting" overlay.

## Delete flow

Clicking `×` on a non-active card removes the entry from local `rawConfig`
state immediately. The change is **not** persisted until the user clicks
**Save Config** (which they may want to do alongside other edits, or not at
all if they undo by reloading the page).

The active card's `×` is disabled.

Rationale for not persisting on delete: a delete-triggered save would reboot
the device, which is too heavy a side-effect for an "I cleaned up an old
preset" gesture. Coupling the persistence to the existing Save Config keeps
all reboot-causing actions behind explicit user intent.

## Components touched

- `web/app/pages/config.vue` — all changes.
  - Add `slugify(model: string): string` helper.
  - Add `presets` computed (entries minus `default`).
  - Add card list markup with click handler.
  - Add confirmation modal (use `UModal` from `@nuxt/ui`).
  - Modify `save()` to use the slug-derived key.
  - Add `switchPreset(slug)` function — opens modal, on confirm calls a
    variant of save that only changes `providers.default`.
  - Add `deletePreset(slug)` — mutates local `rawConfig`.

No other files are touched. No tests are added (existing config.vue has no
unit tests; the change is well within manual-test territory).

## Risks and edge cases

- **Slug collision across providers.** Two distinct providers happening to
  have the same `(name, model)` is implausible (provider names are unique by
  convention) but not impossible. Mitigation: none; if it happens the user
  sees the second save overwrite the first, matching the documented
  fingerprint-overwrite behavior.
- **Legacy entries with capital letters or odd characters.** Existing
  `providers.google` etc. are lowercase ASCII; the slug function output
  matches that shape. No collision risk between legacy and slugged keys.
- **Save Config without a model.** `slugify("")` returns `""`, producing a
  slug like `openai__`. Guard: skip the slug write and fall back to the bare
  provider name as the key when the model field is empty. (This matches
  current behavior — saving with no model leaves `providers.openai = {…}`.)
- **Form values that don't match any saved preset.** Cards reflect what's in
  `rawConfig.providers`; the form is a separate edit surface. If the user
  edits the form without saving, no card highlights as active beyond the
  one corresponding to the on-disk `default`. This matches user expectation
  (cards = saved state, form = draft).

## Out of scope (revisitable later)

- Hot-swap without reboot (would be a new endpoint + in-memory mutex on
  Gateway).
- Per-chat provider override (passing `provider` in `/api/chat`).
- Renaming / labels.
- Multiple keys per fingerprint.

These are all additive on top of this design — none would require undoing
work done here.
