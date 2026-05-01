<script setup lang="ts">
import { Codemirror } from 'vue-codemirror'
import { markdown } from '@codemirror/lang-markdown'
import { oneDark } from '@codemirror/theme-one-dark'
import type { MemoryBlock } from '~/utils/memory'
import {
  MAX_BYTES,
  MAX_ENTRIES,
  capacityInfo,
  newMemoryId,
  nowTimestamp,
  parseMemoryFile,
  serializeMemoryFile,
  tagFrequencies,
} from '~/utils/memory'

const { state, readFile, writeFile } = useConnection()
const toast = useToast()

const editorExtensions = [oneDark, markdown()]

// --- Soul ---
const soulContent = ref('')
const soulOriginal = ref('')
const soulLoading = ref(false)
const soulSaving = ref(false)
const soulError = ref<string | null>(null)
const soulDirty = computed(() => soulContent.value !== soulOriginal.value)

async function loadSoul() {
  soulLoading.value = true
  soulError.value = null
  try {
    const result = await readFile('/data/SOUL.md')
    soulContent.value = result.content
    soulOriginal.value = result.content
  } catch (e: any) {
    if (e.status === 404) {
      soulContent.value = ''
      soulOriginal.value = ''
    } else {
      soulError.value = e.message
    }
  }
  soulLoading.value = false
}

async function saveSoul() {
  soulSaving.value = true
  soulError.value = null
  try {
    await writeFile('/data/SOUL.md', soulContent.value)
    soulOriginal.value = soulContent.value
    toast.add({ title: 'Soul saved', color: 'success', icon: 'i-lucide-check' })
  } catch (e: any) {
    soulError.value = `Save failed: ${e.message}`
  }
  soulSaving.value = false
}

// --- Memories (single MEMORY.md) ---
const memoriesLoading = ref(false)
const memoriesError = ref<string | null>(null)
const blocks = ref<MemoryBlock[]>([])

async function loadMemories() {
  memoriesLoading.value = true
  memoriesError.value = null
  try {
    const result = await readFile('/data/MEMORY.md')
    blocks.value = parseMemoryFile(result.content)
  } catch (e: any) {
    if (e.status === 404) {
      blocks.value = []
    } else {
      memoriesError.value = e.message ?? String(e)
      blocks.value = []
    }
  }
  memoriesLoading.value = false
}

async function persistBlocks(next: MemoryBlock[]): Promise<void> {
  const serialized = serializeMemoryFile(next)
  await writeFile('/data/MEMORY.md', serialized)
  blocks.value = next
}

const capacity = computed(() =>
  capacityInfo(blocks.value, serializeMemoryFile(blocks.value)))

// --- Search & tag filter ---
const searchQuery = ref('')
const activeTag = ref<string | null>(null)

const tagCounts = computed(() => tagFrequencies(blocks.value))
const sortedTags = computed(() =>
  [...tagCounts.value.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])))

const filteredBlocks = computed(() => {
  const q = searchQuery.value.trim().toLowerCase()
  return blocks.value.filter((b) => {
    if (activeTag.value && !b.tags.some(t => t.toLowerCase() === activeTag.value)) return false
    if (!q) return true
    const haystack = `${b.id}\n${b.title}\n${b.content}\n${b.tags.join(' ')}`.toLowerCase()
    return haystack.includes(q)
  })
})

// Newest first — `## [<id>] <timestamp>` blocks store ISO timestamps that
// sort lexicographically.
const sortedBlocks = computed(() =>
  [...filteredBlocks.value].sort((a, b) => b.timestamp.localeCompare(a.timestamp)))

const totalCount = computed(() => blocks.value.length)
const filteredCount = computed(() => filteredBlocks.value.length)

// --- Editor slideover ---
const editorOpen = ref(false)
const editorSaving = ref(false)
const editingBlock = ref<MemoryBlock | null>(null)

function openCreate() {
  editingBlock.value = null
  editorOpen.value = true
}

function openEdit(b: MemoryBlock) {
  editingBlock.value = b
  editorOpen.value = true
}

async function handleSave(payload: { title: string; content: string; tags: string[] }) {
  editorSaving.value = true
  try {
    let next: MemoryBlock[]
    if (editingBlock.value) {
      const targetId = editingBlock.value.id
      next = blocks.value.map(b =>
        b.id === targetId
          ? { ...b, title: payload.title, content: payload.content, tags: payload.tags }
          : b)
    } else {
      if (blocks.value.length >= MAX_ENTRIES) {
        toast.add({
          title: `Memory full (${MAX_ENTRIES} entries)`,
          description: 'Delete or merge entries before adding more.',
          color: 'error',
          icon: 'i-lucide-circle-x',
        })
        editorSaving.value = false
        return
      }
      const newBlock: MemoryBlock = {
        id: newMemoryId(),
        timestamp: nowTimestamp(),
        title: payload.title,
        tags: payload.tags,
        content: payload.content,
      }
      next = [...blocks.value, newBlock]
    }

    const serialized = serializeMemoryFile(next)
    if (new TextEncoder().encode(serialized).length > MAX_BYTES) {
      toast.add({
        title: 'Memory full (size cap)',
        description: `Would exceed ${MAX_BYTES / 1024} KB. Trim or delete other entries first.`,
        color: 'error',
        icon: 'i-lucide-circle-x',
      })
      editorSaving.value = false
      return
    }

    await persistBlocks(next)
    toast.add({
      title: editingBlock.value ? 'Memory saved' : 'Memory created',
      color: 'success',
      icon: 'i-lucide-check',
    })
    editorOpen.value = false
  } catch (e: any) {
    toast.add({
      title: 'Save failed',
      description: e.message ?? String(e),
      color: 'error',
      icon: 'i-lucide-circle-x',
    })
  }
  editorSaving.value = false
}

// --- Delete confirmation ---
const deleteTarget = ref<MemoryBlock | null>(null)
const deleteOpen = ref(false)
const deleting = ref(false)

function confirmDelete(b: MemoryBlock) {
  deleteTarget.value = b
  deleteOpen.value = true
}

async function doDelete() {
  if (!deleteTarget.value) return
  deleting.value = true
  try {
    const targetId = deleteTarget.value.id
    const next = blocks.value.filter(b => b.id !== targetId)
    await persistBlocks(next)
    toast.add({
      title: 'Memory deleted',
      description: targetId,
      color: 'success',
      icon: 'i-lucide-check',
    })
    deleteOpen.value = false
    deleteTarget.value = null
  } catch (e: any) {
    toast.add({
      title: 'Delete failed',
      description: e.message ?? String(e),
      color: 'error',
      icon: 'i-lucide-circle-x',
    })
  }
  deleting.value = false
}

// --- Advanced: raw MEMORY.md ---
const showAdvanced = ref(false)
const rawContent = ref('')
const rawOriginal = ref('')
const rawLoading = ref(false)
const rawSaving = ref(false)
const rawDirty = computed(() => rawContent.value !== rawOriginal.value)

async function loadRaw() {
  rawLoading.value = true
  try {
    const result = await readFile('/data/MEMORY.md')
    rawContent.value = result.content
    rawOriginal.value = result.content
  } catch (e: any) {
    if (e.status === 404) {
      rawContent.value = ''
      rawOriginal.value = ''
    } else {
      toast.add({ title: 'Could not load MEMORY.md', description: e.message, color: 'error' })
    }
  }
  rawLoading.value = false
}

async function saveRaw() {
  rawSaving.value = true
  try {
    await writeFile('/data/MEMORY.md', rawContent.value)
    rawOriginal.value = rawContent.value
    blocks.value = parseMemoryFile(rawContent.value)
    toast.add({ title: 'MEMORY.md saved', color: 'success', icon: 'i-lucide-check' })
  } catch (e: any) {
    toast.add({ title: 'Save failed', description: e.message, color: 'error' })
  }
  rawSaving.value = false
}

watch(showAdvanced, (v) => { if (v && !rawOriginal.value) loadRaw() })

// --- Lifecycle ---
async function loadAll() {
  await Promise.all([loadSoul(), loadMemories()])
}

onMounted(() => { if (state.networkConnected) loadAll() })
watch(() => state.networkConnected, (connected) => { if (connected) loadAll() })
</script>

<template>
  <div class="space-y-6">
    <div class="flex items-end justify-between gap-4 flex-wrap">
      <div>
        <h1 class="text-2xl font-bold">Soul & Memory</h1>
        <p class="text-sm text-dimmed mt-1">
          The soul defines your agent's core personality. Memories are facts the agent
          recalls across conversations, stored in a single MEMORY.md file (capped at
          {{ MAX_ENTRIES }} entries / {{ MAX_BYTES / 1024 }} KB).
        </p>
      </div>
    </div>

    <template v-if="state.networkConnected">
      <!-- Soul -->
      <UCard>
        <template #header>
          <div class="flex items-center justify-between gap-2">
            <div class="flex items-center gap-2">
              <UIcon name="i-lucide-sparkles" class="text-primary size-5" />
              <span class="font-semibold">Soul</span>
              <span class="text-xs text-dimmed font-mono">SOUL.md</span>
            </div>
            <div class="flex items-center gap-2">
              <UButton icon="i-lucide-refresh-cw" variant="ghost" size="xs" :loading="soulLoading" @click="loadSoul" />
              <UButton label="Save" size="xs" icon="i-lucide-save" :disabled="!soulDirty || soulSaving" :loading="soulSaving" @click="saveSoul" />
            </div>
          </div>
        </template>

        <UAlert v-if="soulError" icon="i-lucide-circle-x" color="error" variant="subtle" :description="soulError" class="mb-3" />

        <div v-if="soulLoading" class="flex h-32 items-center justify-center">
          <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
        </div>
        <div v-else class="rounded-md border border-default overflow-hidden">
          <Codemirror
            v-model="soulContent"
            :extensions="editorExtensions"
            :style="{ minHeight: '12rem' }"
            placeholder="Define your agent's personality and behavior..."
          />
        </div>
      </UCard>

      <!-- Memories toolbar + capacity -->
      <div class="rounded-xl border border-default bg-elevated/40 p-4 space-y-4">
        <div class="flex items-center justify-between gap-3 flex-wrap">
          <div class="flex items-center gap-2">
            <UIcon name="i-lucide-brain" class="text-primary size-5" />
            <h2 class="font-semibold text-lg">Memories</h2>
            <UBadge :label="`${totalCount}`" variant="soft" color="neutral" size="xs" />
          </div>

          <div class="flex items-center gap-2">
            <UButton icon="i-lucide-refresh-cw" variant="ghost" size="sm" :loading="memoriesLoading" @click="loadMemories" />
            <UButton label="New memory" icon="i-lucide-plus" size="sm" :disabled="capacity.full" @click="openCreate" />
          </div>
        </div>

        <!-- Capacity -->
        <div class="space-y-1">
          <div class="flex items-baseline justify-between text-xs font-mono">
            <span :class="{ 'text-warning': capacity.near && !capacity.full, 'text-error': capacity.full, 'text-dimmed': !capacity.near }">
              {{ capacity.pct }}% capacity
            </span>
            <span class="text-dimmed">
              {{ capacity.count }}/{{ MAX_ENTRIES }} entries · {{ (capacity.bytes / 1024).toFixed(1) }}KB / {{ MAX_BYTES / 1024 }}KB
            </span>
          </div>
          <div class="h-1.5 w-full rounded-full bg-default/40 overflow-hidden">
            <div
              class="h-full rounded-full transition-[width] duration-200"
              :class="capacity.full ? 'bg-error' : capacity.near ? 'bg-warning' : 'bg-primary'"
              :style="{ width: `${capacity.pct}%` }"
            />
          </div>
          <p v-if="capacity.near" class="text-xs" :class="capacity.full ? 'text-error' : 'text-warning'">
            <UIcon name="i-lucide-triangle-alert" class="size-3.5 inline -mt-0.5" />
            {{ capacity.full ? 'Memory is full. The agent cannot save new entries until you delete or merge some.' : 'Memory near capacity. Consider deleting or merging stale entries before adding more.' }}
          </p>
        </div>

        <!-- Search + tag pills -->
        <div class="flex items-center gap-3 flex-wrap">
          <UInput
            v-model="searchQuery"
            placeholder="Search by content, id, or tag…"
            icon="i-lucide-search"
            size="sm"
            class="flex-1 min-w-[14rem]"
            :ui="{ root: 'w-full' }"
          />
        </div>

        <div v-if="sortedTags.length > 0" class="flex items-center gap-1.5 flex-wrap">
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-full px-2.5 py-0.5 text-xs font-medium border transition-colors"
            :class="activeTag === null
              ? 'bg-primary/10 border-primary/40 text-primary'
              : 'bg-transparent border-default text-dimmed hover:opacity-100'"
            @click="activeTag = null"
          >
            All
            <span class="text-dimmed">{{ totalCount }}</span>
          </button>
          <button
            v-for="[tag, count] in sortedTags"
            :key="tag"
            type="button"
            class="inline-flex items-center gap-1 rounded-full px-2.5 py-0.5 text-xs font-medium border transition-colors"
            :class="activeTag === tag
              ? 'bg-primary/10 border-primary/40 text-primary'
              : 'bg-transparent border-default text-dimmed hover:opacity-100'"
            @click="activeTag = activeTag === tag ? null : tag"
          >
            {{ tag }}
            <span class="text-dimmed">{{ count }}</span>
          </button>
        </div>

        <p v-if="(searchQuery || activeTag) && filteredCount !== totalCount" class="text-xs text-dimmed">
          Showing {{ filteredCount }} of {{ totalCount }}
        </p>
      </div>

      <UAlert v-if="memoriesError" icon="i-lucide-circle-x" color="error" variant="subtle" :description="memoriesError" />

      <!-- Loading -->
      <div v-if="memoriesLoading && blocks.length === 0" class="flex h-48 items-center justify-center">
        <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
      </div>

      <!-- Empty: no memories -->
      <div
        v-else-if="!memoriesLoading && blocks.length === 0 && !memoriesError"
        class="flex flex-col items-center justify-center gap-3 py-16 text-center"
      >
        <UIcon name="i-lucide-brain" class="text-dimmed size-12" />
        <div>
          <p class="font-medium">No memories yet</p>
          <p class="text-sm text-dimmed mt-1 max-w-md">
            Memories accumulate as the agent learns about you. You can also seed one manually.
          </p>
        </div>
        <UButton label="Create the first memory" icon="i-lucide-plus" @click="openCreate" />
      </div>

      <!-- Empty: filter to nothing -->
      <div
        v-else-if="filteredCount === 0"
        class="flex flex-col items-center justify-center gap-2 py-12 text-center text-sm text-dimmed"
      >
        <UIcon name="i-lucide-search-x" class="size-8" />
        <p>No memories match your filters.</p>
        <UButton label="Clear filters" variant="ghost" size="xs" @click="searchQuery = ''; activeTag = null" />
      </div>

      <!-- Cards -->
      <div v-else class="grid gap-3 grid-cols-1 lg:grid-cols-2">
        <MemoryCard
          v-for="b in sortedBlocks"
          :key="b.id"
          :block="b"
          @edit="openEdit(b)"
          @delete="confirmDelete(b)"
        />
      </div>

      <!-- Advanced: raw MEMORY.md -->
      <UCollapsible v-model:open="showAdvanced" class="rounded-xl border border-default">
        <UButton
          variant="ghost"
          color="neutral"
          class="w-full justify-between"
          :ui="{ base: 'rounded-xl' }"
          :trailing-icon="showAdvanced ? 'i-lucide-chevron-up' : 'i-lucide-chevron-down'"
        >
          <span class="flex items-center gap-2">
            <UIcon name="i-lucide-file-cog" class="size-4 text-dimmed" />
            Advanced — edit raw MEMORY.md
          </span>
        </UButton>
        <template #content>
          <div class="p-4 space-y-3 border-t border-default">
            <p class="text-xs text-dimmed">
              Raw editor for MEMORY.md. Use this to bulk-edit, fix malformed entries, or seed the file.
              The card view above re-parses on save.
            </p>
            <div v-if="rawLoading" class="flex h-32 items-center justify-center">
              <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
            </div>
            <div v-else class="rounded-md border border-default overflow-hidden">
              <Codemirror
                v-model="rawContent"
                :extensions="editorExtensions"
                :style="{ minHeight: '14rem' }"
                placeholder="## [mem_xxxx] 2026-05-01T10:30:00Z (tags: preference)\nbody"
              />
            </div>
            <div class="flex items-center justify-end gap-2">
              <UButton icon="i-lucide-refresh-cw" variant="ghost" size="xs" :loading="rawLoading" @click="loadRaw" />
              <UButton
                label="Save MEMORY.md"
                icon="i-lucide-save"
                size="xs"
                :disabled="!rawDirty || rawSaving"
                :loading="rawSaving"
                @click="saveRaw"
              />
            </div>
          </div>
        </template>
      </UCollapsible>

      <!-- Editor slideover -->
      <MemoryEditor
        v-model:open="editorOpen"
        :block="editingBlock"
        :saving="editorSaving"
        @save="handleSave"
      />

      <!-- Delete confirmation -->
      <UModal v-model:open="deleteOpen" title="Delete memory?">
        <template #body>
          <p class="text-sm text-muted">
            This permanently deletes <span class="font-mono text-default">{{ deleteTarget?.id }}</span>.
            The agent won't be able to recall it in future conversations.
          </p>
          <p v-if="deleteTarget?.content" class="mt-3 text-xs text-dimmed border-l-2 border-default pl-3 italic line-clamp-3">
            {{ deleteTarget.content }}
          </p>
        </template>
        <template #footer>
          <div class="flex justify-end gap-2 w-full">
            <UButton label="Cancel" variant="ghost" color="neutral" @click="deleteOpen = false" />
            <UButton label="Delete" color="error" :loading="deleting" :disabled="deleting" @click="doDelete" />
          </div>
        </template>
      </UModal>
    </template>

    <UAlert
      v-else
      icon="i-lucide-wifi-off"
      color="warning"
      variant="subtle"
      title="Not connected"
      description="Connect to a device on the Dashboard to view and edit its soul and memories."
    />
  </div>
</template>
