# Model presets fast-dial Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a "fast dial" of saved provider+model+endpoint preset cards to the config page so the user can switch between preconfigured LLM providers in one click (with reboot confirmation).

**Architecture:** UI-only change to `web/app/pages/config.vue`. The Rust agent's `ProvidersConfig` (`agent/src/config.rs`) already supports an arbitrary map of `providers.{slug}` entries with `default` naming the active one — we just expose it. Each preset is keyed by a slug `{providerName}__{slugifiedModel}` derived from the form values. Saving overwrites the same slug; switching repurposes `/api/config` PUT (existing reboot path).

**Tech Stack:** Nuxt 4 + Vue 3 (`<script setup>`), `@nuxt/ui` v4 (UModal, UCard, UButton, UFormField), Tailwind. Existing `useConnection().saveConfig(config)` posts to `/api/config`.

**Spec:** `docs/superpowers/specs/2026-05-02-model-presets-fast-dial-design.md`.

**Testing strategy:** This codebase has no unit tests for Vue pages. Each task ends with a manual verification step against the dev server (`cd web && npm run dev`, browser at `http://localhost:3000/config`) connected to a real device or to the desktop agent (`cargo run --features desktop`).

---

## Task 1: Slug-derived save key

Modify `save()` so the active provider entry is written under a slug derived from the form values (`{providerName}__{slugifiedModel}`) instead of overwriting `providers[providerName]`. This is the foundation: once saving uses slugs, multiple presets can coexist in `providers`.

**Files:**
- Modify: `web/app/pages/config.vue` — add `slugify()` helper in `<script setup>`, change `save()` body around line 230-245.

- [ ] **Step 1: Add `slugify` helper inside `<script setup>` of `config.vue`**

Insert this function after the `BASE_URLS` constant (around line 45, before `// --- Form fields ---`):

```ts
function slugify(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
}
```

- [ ] **Step 2: Change the provider-section of `save()` to use slug keys**

Locate the provider block in `save()` (currently lines 237-245):

```ts
  const prov = config.providers || {}
  const key = isCustomProvider.value ? 'custom' : providerName.value
  prov.default = key
  const providerConfig = prov[key] || {}
  providerConfig.api_key = apiKey.value
  providerConfig.model = model.value
  providerConfig.base_url = baseUrl.value
  prov[key] = providerConfig
  config.providers = prov
```

Replace with:

```ts
  const prov = config.providers || {}
  const providerKey = isCustomProvider.value ? 'custom' : providerName.value
  // Slug-derived key lets multiple presets coexist (openai__gpt-4o-mini,
  // openai__gpt-5, etc.). Falls back to the bare provider name when the
  // model is empty so the form still saves something usable.
  const slug = model.value ? `${providerKey}__${slugify(model.value)}` : providerKey
  prov.default = slug
  const providerConfig = prov[slug] || {}
  providerConfig.api_key = apiKey.value
  providerConfig.model = model.value
  providerConfig.base_url = baseUrl.value
  prov[slug] = providerConfig
  config.providers = prov
```

- [ ] **Step 3: Update the `load()` provider matching to handle slugs**

`load()` (currently around lines 148-161) maps `providers.default` back to the `BASE_URLS`/dropdown identity. With slugs like `openai__gpt-4o-mini`, the existing `BASE_URLS[defaultProvider]` lookup misses. Update the matching block to strip the slug suffix first.

Locate (around line 148):

```ts
    const defaultProvider = config.providers?.default || 'google'
    const provider = config.providers?.[defaultProvider] || {}
    apiKey.value = provider.api_key || ''
    model.value = provider.model || ''
    baseUrl.value = provider.base_url || ''

    // Match to a known provider or fall back to custom
    // Check both by name and by base_url
    if (BASE_URLS[defaultProvider]) {
      providerName.value = defaultProvider
    } else {
      const match = Object.entries(BASE_URLS).find(([, url]) => baseUrl.value.includes(url))
      providerName.value = match ? match[0] : 'custom'
    }
```

Replace with:

```ts
    const defaultSlug = config.providers?.default || 'google'
    const provider = config.providers?.[defaultSlug] || {}
    apiKey.value = provider.api_key || ''
    model.value = provider.model || ''
    baseUrl.value = provider.base_url || ''

    // Slugs look like "openai__gpt-4o-mini" — strip the suffix to recover
    // the provider identity. Legacy keys ("openai") have no suffix and
    // pass through unchanged.
    const providerKey = defaultSlug.split('__')[0] || 'google'
    if (BASE_URLS[providerKey]) {
      providerName.value = providerKey
    } else {
      const match = Object.entries(BASE_URLS).find(([, url]) => baseUrl.value.includes(url))
      providerName.value = match ? match[0] : 'custom'
    }
```

- [ ] **Step 4: Manual verify (dev server)**

Run the web dev server and a backing agent:

```bash
# Terminal 1
cd web && npm run dev

# Terminal 2 (desktop agent for fast iteration; ESP32 also works)
cd agent && cargo run --features desktop
```

In the browser at `http://localhost:3000/config`, on the LLM Provider tab:
1. Pick provider = `openai`, model = `gpt-4o-mini`, base URL auto-fills, api_key = `test-key-1`.
2. Click **Save Config**.
3. Reload the page after the device comes back.
4. Open the Files page (or `curl http://localhost:8080/api/config | python3 -m json.tool`) and verify the persisted JSON contains:
   - `providers.default == "openai__gpt-4o-mini"`
   - `providers.openai__gpt-4o-mini == { api_key: "test-key-1", model: "gpt-4o-mini", base_url: "https://api.openai.com/v1" }`
5. Reload the config page and confirm the form fields repopulate correctly (provider=openai, model=gpt-4o-mini).

Expected: the `providers.{slug}` entry exists, `default` points at the slug, the form round-trips through `load()`.

- [ ] **Step 5: Commit**

```bash
git add web/app/pages/config.vue
git commit -m "feat(web): slug-derived keys for provider config saves

Save Config now writes the active provider under
providers.{providerName}__{slug(model)} instead of overwriting
providers[providerName]. Foundation for the model-presets fast-dial.

Spec: docs/superpowers/specs/2026-05-02-model-presets-fast-dial-design.md"
```

---

## Task 2: Render preset cards (display-only)

Render the saved presets from `rawConfig.providers` as cards inside the LLM Provider tab, below the existing form fields. No interactivity yet — just display, with the active card visually distinguished.

**Files:**
- Modify: `web/app/pages/config.vue` — add `presets` computed in `<script setup>`, add card markup at the end of the `<template #provider>` block (currently lines 376-424).

- [ ] **Step 1: Add `presets` computed**

Insert after the existing `isCustomProvider` computed (around line 123):

```ts
interface Preset {
  slug: string
  provider: string
  model: string
  baseUrl: string
  isActive: boolean
}

const presets = computed<Preset[]>(() => {
  const provs = rawConfig.value?.providers || {}
  const defaultSlug = provs.default
  return Object.entries(provs)
    .filter(([k]) => k !== 'default')
    .map(([slug, entry]: [string, any]) => ({
      slug,
      provider: slug.split('__')[0] || slug,
      model: entry?.model || '',
      baseUrl: entry?.base_url || '',
      isActive: slug === defaultSlug,
    }))
    .sort((a, b) => a.slug.localeCompare(b.slug))
})

function stripScheme(url: string): string {
  return url.replace(/^https?:\/\//, '')
}
```

- [ ] **Step 2: Add card markup at the end of the LLM Provider tab**

Locate the closing of `<template #provider>` (currently around line 423-424):

```html
              <UFormField label="API Key" class="w-full">
                <UInput v-model="apiKey" type="text" placeholder="API key" size="xl" class="w-full" />
              </UFormField>
            </div>
          </template>
```

Replace with (adds the preset section *inside* the same `<div class="space-y-4 pt-4">`, after the API Key field):

```html
              <UFormField label="API Key" class="w-full">
                <UInput v-model="apiKey" type="text" placeholder="API key" size="xl" class="w-full" />
              </UFormField>

              <div v-if="presets.length" class="pt-6 border-t border-default">
                <div class="flex items-center justify-between mb-3">
                  <h3 class="text-sm font-medium">Saved Models</h3>
                  <span class="text-xs text-dimmed">Click a card to switch</span>
                </div>
                <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <button
                    v-for="p in presets"
                    :key="p.slug"
                    type="button"
                    class="text-left rounded-lg border p-3 transition-colors"
                    :class="p.isActive
                      ? 'border-primary bg-primary/5 cursor-default'
                      : 'border-default hover:border-primary/50 hover:bg-elevated cursor-pointer'"
                  >
                    <div class="flex items-start justify-between gap-2">
                      <div class="min-w-0 flex-1">
                        <div class="flex items-center gap-1.5 mb-1">
                          <span v-if="p.isActive" class="size-1.5 rounded-full bg-primary" />
                          <span class="text-sm font-medium truncate">{{ p.provider }}</span>
                        </div>
                        <p class="text-xs text-muted truncate">{{ p.model || '—' }}</p>
                        <p class="text-xs text-dimmed truncate font-mono">{{ stripScheme(p.baseUrl) || '—' }}</p>
                      </div>
                    </div>
                  </button>
                </div>
              </div>
            </div>
          </template>
```

- [ ] **Step 3: Manual verify (dev server)**

With the dev server still running from Task 1:
1. Manually edit the device config (via `curl -X POST http://localhost:8080/api/config -H 'Content-Type: application/json' -d @<file>`) to seed two providers, e.g.:

```json
{
  "providers": {
    "default": "openai__gpt-4o-mini",
    "openai__gpt-4o-mini": {"api_key":"k1","model":"gpt-4o-mini","base_url":"https://api.openai.com/v1"},
    "anthropic__claude-haiku": {"api_key":"k2","model":"claude-haiku-4-5","base_url":"https://api.anthropic.com"}
  },
  "agent_name": "ZenClaw"
}
```

2. Reload `http://localhost:3000/config` → LLM Provider tab.
3. Confirm two cards render below the API Key field.
4. The `openai` card has the active dot and primary-colored border; the `anthropic` card looks neutral and shows hover affordance on mouseover.
5. Each card shows three lines (provider, model, endpoint without `https://`).
6. Clicking a card does nothing yet (no handler attached).

Expected: cards render correctly with active highlighting; no interactivity.

- [ ] **Step 4: Commit**

```bash
git add web/app/pages/config.vue
git commit -m "feat(web): render saved provider presets as cards on config page

Cards listed below the LLM Provider form fields; active preset gets a
dot + primary border. Display-only — switching is added in the next
commit."
```

---

## Task 3: Switch flow with confirmation modal

Wire card clicks to a confirmation modal that, on confirm, PUTs `/api/config` with `providers.default` set to the clicked slug. The existing reboot path runs.

**Files:**
- Modify: `web/app/pages/config.vue` — add modal state, click handler, `confirmSwitch()`, UModal markup.

- [ ] **Step 1: Add modal state and switch functions**

Insert after the `presets` computed (added in Task 2):

```ts
const switchOpen = ref(false)
const switchTarget = ref<Preset | null>(null)
const switching = ref(false)

function openSwitch(p: Preset) {
  if (p.isActive) return
  switchTarget.value = p
  switchOpen.value = true
}

async function confirmSwitch() {
  const target = switchTarget.value
  if (!target) return
  switching.value = true
  error.value = null
  successMsg.value = null
  try {
    const config = {
      ...rawConfig.value,
      providers: { ...(rawConfig.value.providers || {}), default: target.slug },
    }
    await saveConfig(config)
    rawConfig.value = config
    successMsg.value = `Switching to ${target.provider} / ${target.model}. Device is rebooting…`
    switchOpen.value = false
  } catch (e: any) {
    error.value = `Switch failed: ${e.message}`
  }
  switching.value = false
}
```

- [ ] **Step 2: Wire the click handler on the card button**

Locate the card `<button>` element added in Task 2. Modify it to add `@click="openSwitch(p)"`:

```html
                  <button
                    v-for="p in presets"
                    :key="p.slug"
                    type="button"
                    class="text-left rounded-lg border p-3 transition-colors"
                    :class="p.isActive
                      ? 'border-primary bg-primary/5 cursor-default'
                      : 'border-default hover:border-primary/50 hover:bg-elevated cursor-pointer'"
                    @click="openSwitch(p)"
                  >
```

- [ ] **Step 3: Add the confirmation modal**

Inside the outermost `<template>` of the file, just before the final closing `</div>` (currently around line 667), add the UModal. Locate this near the bottom of the template:

```html
        <!-- Actions -->
        <div class="flex justify-end gap-3">
          ...
        </div>
      </template>
    </template>
  </div>
</template>
```

Insert the modal between the actions row and the closing `</template>` / `</div>`:

```html
        <!-- Actions -->
        <div class="flex justify-end gap-3">
          ...
        </div>

        <UModal v-model:open="switchOpen" title="Switch active model?">
          <template #body>
            <p class="text-sm text-default">
              Switch the active model to
              <span class="font-medium">{{ switchTarget?.provider }} / {{ switchTarget?.model }}</span>?
            </p>
            <p class="mt-3 text-sm text-muted">
              The device will reboot to apply the change (~12 seconds offline).
            </p>
          </template>
          <template #footer>
            <div class="flex justify-end gap-2 w-full">
              <UButton
                label="Cancel"
                variant="ghost"
                color="neutral"
                :disabled="switching"
                @click="switchOpen = false"
              />
              <UButton
                label="Switch & Reboot"
                color="primary"
                :loading="switching"
                :disabled="switching"
                @click="confirmSwitch"
              />
            </div>
          </template>
        </UModal>
      </template>
    </template>
  </div>
</template>
```

(Leave the existing `...` actions row contents intact — only insert the `<UModal>` block immediately after it.)

- [ ] **Step 4: Manual verify (dev server)**

1. Reload `http://localhost:3000/config` with the two-preset seed config from Task 2.
2. Click the **anthropic** card → modal opens with "Switch active model? anthropic / claude-haiku-4-5" and the reboot warning.
3. Click **Cancel** → modal closes, no save.
4. Click the card again → modal reopens.
5. Click **Switch & Reboot** → button shows loading; the call to `/api/config` returns; modal closes; success banner shows "Switching to … Device is rebooting…".
6. Wait for the device to come back (desktop: instant; ESP32: ~12s and the `ConnectionBanner` should flap to disconnected then reconnected).
7. Reload the config page → form now reflects `claude-haiku-4-5` from the now-active anthropic preset; the **anthropic** card has the active dot.
8. Click the active card (anthropic) → nothing happens (early-return in `openSwitch`).

Expected: confirm modal works, switch persists, page reloads with new active preset.

- [ ] **Step 5: Commit**

```bash
git add web/app/pages/config.vue
git commit -m "feat(web): one-click switch between saved provider presets

Clicking a non-active preset card opens a confirmation modal; on confirm
the change is persisted via /api/config (existing reboot path) with
providers.default set to the clicked slug."
```

---

## Task 4: Delete preset

Add a `×` button on each card that removes the entry from local `rawConfig` state. Persistence happens on the next **Save Config**. The active card's `×` is disabled (would orphan `default`).

**Files:**
- Modify: `web/app/pages/config.vue` — add `deletePreset()` function, add `×` button to card, click stops propagation.

- [ ] **Step 1: Add `deletePreset` function**

Insert right after `confirmSwitch` (added in Task 3):

```ts
function deletePreset(p: Preset, ev: Event) {
  ev.stopPropagation()
  if (p.isActive) return
  const provs = { ...(rawConfig.value.providers || {}) }
  delete provs[p.slug]
  rawConfig.value = { ...rawConfig.value, providers: provs }
}
```

- [ ] **Step 2: Add the × button inside each card**

Locate the card markup added in Task 2. Inside the `<button>` element, after the existing `<div class="min-w-0 flex-1">` block, add a delete button. The card should look like:

```html
                  <button
                    v-for="p in presets"
                    :key="p.slug"
                    type="button"
                    class="text-left rounded-lg border p-3 transition-colors"
                    :class="p.isActive
                      ? 'border-primary bg-primary/5 cursor-default'
                      : 'border-default hover:border-primary/50 hover:bg-elevated cursor-pointer'"
                    @click="openSwitch(p)"
                  >
                    <div class="flex items-start justify-between gap-2">
                      <div class="min-w-0 flex-1">
                        <div class="flex items-center gap-1.5 mb-1">
                          <span v-if="p.isActive" class="size-1.5 rounded-full bg-primary" />
                          <span class="text-sm font-medium truncate">{{ p.provider }}</span>
                        </div>
                        <p class="text-xs text-muted truncate">{{ p.model || '—' }}</p>
                        <p class="text-xs text-dimmed truncate font-mono">{{ stripScheme(p.baseUrl) || '—' }}</p>
                      </div>
                      <UButton
                        icon="i-lucide-x"
                        variant="ghost"
                        color="neutral"
                        size="xs"
                        :disabled="p.isActive"
                        :title="p.isActive ? 'Cannot delete the active preset' : 'Remove preset'"
                        @click="(ev: Event) => deletePreset(p, ev)"
                      />
                    </div>
                  </button>
```

- [ ] **Step 3: Manual verify (dev server)**

1. Reload `http://localhost:3000/config` with at least two presets.
2. The active card's `×` is greyed-out / disabled and not clickable.
3. Click the `×` on the non-active card → it disappears from the grid immediately (no modal, no reboot).
4. The form fields are unchanged; the page does not reload.
5. The success banner does not appear (delete is a local operation).
6. Now click **Save Config** at the bottom of the page → device reboots with the deleted preset truly gone from `providers`.
7. After reload, the deleted card stays gone.
8. Alternative: instead of saving, hit browser reload — the deleted card comes back from `getConfig()` (delete was local-only).

Expected: delete is local + free, persists only via Save Config; active preset's × is disabled.

- [ ] **Step 4: Commit**

```bash
git add web/app/pages/config.vue
git commit -m "feat(web): allow removing saved provider presets

× button on each card removes the entry from local state; persistence
happens on the next Save Config. The active preset's × is disabled
(would orphan providers.default)."
```

---

## Self-review checklist (run after final task)

- [ ] All four spec sections (storage model, UI, switch flow, delete flow) have a corresponding task. ✓
- [ ] No `TODO` / `TBD` / placeholder strings in any task. ✓
- [ ] `slugify`, `presets`, `Preset`, `openSwitch`, `confirmSwitch`, `deletePreset`, `stripScheme` names match across tasks. ✓
- [ ] All four commits add up to a working feature; the branch is shippable after Task 4.
- [ ] Manual verification covers: save round-trip (T1), card render + active highlight (T2), switch + reboot (T3), local delete + Save Config persistence (T4).

---

## Out of scope (documented in spec)

- Hot-swap without reboot
- Per-chat provider override
- Renaming / labels
- Multiple keys per fingerprint

These are additive on top of this implementation and require separate specs.
