<script setup lang="ts">
import { Codemirror } from 'vue-codemirror'
import { markdown } from '@codemirror/lang-markdown'
import { oneDark } from '@codemirror/theme-one-dark'
import type { FileEntry } from '~/types/connection'

const {
  state, readFile, writeFile, deleteFile, listDir,
} = useConnection()

const editorExtensions = [oneDark, markdown()]

// --- Soul ---
const soulContent = ref('')
const soulOriginal = ref('')
const soulLoading = ref(false)
const soulSaving = ref(false)
const soulError = ref<string | null>(null)
const soulMsg = ref<string | null>(null)
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
  soulMsg.value = null
  try {
    await writeFile('/data/SOUL.md', soulContent.value)
    soulOriginal.value = soulContent.value
    soulMsg.value = 'Saved'
  } catch (e: any) {
    soulError.value = `Save failed: ${e.message}`
  }
  soulSaving.value = false
}

// --- Memory ---
const memoryContent = ref('')
const memoryOriginal = ref('')
const memoryLoading = ref(false)
const memorySaving = ref(false)
const memoryError = ref<string | null>(null)
const memoryMsg = ref<string | null>(null)
const memoryDirty = computed(() => memoryContent.value !== memoryOriginal.value)

const memoryFiles = ref<FileEntry[]>([])
const memoryFilesLoading = ref(false)

const selectedMemFile = ref<string | null>(null)
const memFileContent = ref('')
const memFileOriginal = ref('')
const memFileLoading = ref(false)
const memFileSaving = ref(false)
const memFileMsg = ref<string | null>(null)
const memFileDirty = computed(() => memFileContent.value !== memFileOriginal.value)

// Delete confirmation
const deleteTarget = ref<FileEntry | null>(null)
const deleteOpen = ref(false)
const deleting = ref(false)

async function loadMemory() {
  memoryLoading.value = true
  memoryError.value = null
  try {
    const result = await readFile('/data/MEMORY.md')
    memoryContent.value = result.content
    memoryOriginal.value = result.content
  } catch (e: any) {
    if (e.status === 404) {
      memoryContent.value = ''
      memoryOriginal.value = ''
    } else {
      memoryError.value = e.message
    }
  }
  memoryLoading.value = false
}

async function saveMemory() {
  memorySaving.value = true
  memoryError.value = null
  memoryMsg.value = null
  try {
    await writeFile('/data/MEMORY.md', memoryContent.value)
    memoryOriginal.value = memoryContent.value
    memoryMsg.value = 'Saved'
  } catch (e: any) {
    memoryError.value = `Save failed: ${e.message}`
  }
  memorySaving.value = false
}

async function loadMemoryFiles() {
  memoryFilesLoading.value = true
  try {
    const result = await listDir('/data/memory')
    memoryFiles.value = result.entries
      .filter(e => !e.isDir && e.name.endsWith('.md'))
      .sort((a, b) => b.name.localeCompare(a.name))
  } catch {
    memoryFiles.value = []
  }
  memoryFilesLoading.value = false
}

async function openMemFile(entry: FileEntry) {
  memFileLoading.value = true
  memFileMsg.value = null
  try {
    const result = await readFile(entry.path)
    selectedMemFile.value = entry.path
    memFileContent.value = result.content
    memFileOriginal.value = result.content
  } catch (e: any) {
    memoryError.value = `Cannot read: ${e.message}`
  }
  memFileLoading.value = false
}

async function saveMemFile() {
  if (!selectedMemFile.value) return
  memFileSaving.value = true
  memFileMsg.value = null
  try {
    await writeFile(selectedMemFile.value, memFileContent.value)
    memFileOriginal.value = memFileContent.value
    memFileMsg.value = 'Saved'
  } catch (e: any) {
    memoryError.value = `Save failed: ${e.message}`
  }
  memFileSaving.value = false
}

function confirmDelete(entry: FileEntry) {
  deleteTarget.value = entry
  deleteOpen.value = true
}

async function doDelete() {
  if (!deleteTarget.value) return
  deleting.value = true
  try {
    await deleteFile(deleteTarget.value.path)
    if (selectedMemFile.value === deleteTarget.value.path) {
      selectedMemFile.value = null
      memFileContent.value = ''
      memFileOriginal.value = ''
    }
    await loadMemoryFiles()
  } catch (e: any) {
    memoryError.value = `Delete failed: ${e.message}`
  }
  deleting.value = false
  deleteOpen.value = false
  deleteTarget.value = null
}

function formatSize(size: number | null): string {
  if (size == null) return ''
  if (size < 1024) return `${size} B`
  return `${(size / 1024).toFixed(1)} KB`
}

// Load everything on mount
async function loadAll() {
  await Promise.all([loadSoul(), loadMemory(), loadMemoryFiles()])
}

onMounted(() => {
  if (state.networkConnected) loadAll()
})

watch(() => state.networkConnected, (connected) => {
  if (connected) loadAll()
})
</script>

<template>
  <div class="space-y-6">
    <h1 class="text-2xl font-bold">Soul & Memory</h1>
    <p class="text-sm text-dimmed">
      The soul defines your agent's core personality and behavior. Memories are what the agent remembers across conversations.
    </p>

    <template v-if="state.networkConnected">
      <!-- Soul -->
      <UCard>
        <template #header>
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2">
              <UIcon name="i-lucide-sparkles" class="text-primary" />
              <span class="font-semibold">Soul</span>
              <span class="text-xs text-dimmed">SOUL.md</span>
            </div>
            <div class="flex items-center gap-2">
              <span v-if="soulMsg" class="text-xs text-green-400">{{ soulMsg }}</span>
              <UButton
                icon="i-lucide-refresh-cw"
                variant="ghost"
                size="xs"
                :disabled="soulLoading"
                @click="loadSoul(); soulMsg = null"
              />
              <UButton
                label="Save"
                size="xs"
                :disabled="!soulDirty || soulSaving"
                @click="saveSoul"
              >
                <template #leading>
                  <UIcon v-if="soulSaving" name="i-lucide-loader-circle" class="size-4 animate-spin" />
                  <UIcon v-else name="i-lucide-save" class="size-4" />
                </template>
              </UButton>
            </div>
          </div>
        </template>

        <UAlert v-if="soulError" icon="i-lucide-circle-x" color="error" variant="subtle" :description="soulError" class="mb-3" />

        <div v-if="soulLoading" class="flex h-48 items-center justify-center">
          <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
        </div>
        <Codemirror
          v-else
          v-model="soulContent"
          :extensions="editorExtensions"
          :style="{ minHeight: '12rem' }"
          placeholder="Define your agent's personality and behavior..."
          @update:model-value="soulMsg = null"
        />
      </UCard>

      <!-- Memory -->
      <UAlert v-if="memoryError" icon="i-lucide-circle-x" color="error" variant="subtle" :description="memoryError" />

      <!-- MEMORY.md -->
      <UCard>
        <template #header>
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2">
              <UIcon name="i-lucide-brain" class="text-primary" />
              <span class="font-semibold">Memory</span>
              <span class="text-xs text-dimmed">MEMORY.md</span>
            </div>
            <div class="flex items-center gap-2">
              <span v-if="memoryMsg" class="text-xs text-green-400">{{ memoryMsg }}</span>
              <UButton
                icon="i-lucide-refresh-cw"
                variant="ghost"
                size="xs"
                :disabled="memoryLoading"
                @click="loadMemory(); loadMemoryFiles(); memoryMsg = null"
              />
              <UButton
                label="Save"
                size="xs"
                :disabled="!memoryDirty || memorySaving"
                @click="saveMemory"
              >
                <template #leading>
                  <UIcon v-if="memorySaving" name="i-lucide-loader-circle" class="size-4 animate-spin" />
                  <UIcon v-else name="i-lucide-save" class="size-4" />
                </template>
              </UButton>
            </div>
          </div>
        </template>

        <div v-if="memoryLoading" class="flex h-48 items-center justify-center">
          <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
        </div>
        <Codemirror
          v-else
          v-model="memoryContent"
          :extensions="editorExtensions"
          :style="{ minHeight: '12rem' }"
          placeholder="No memories yet. The agent will accumulate memories during conversations."
          @update:model-value="memoryMsg = null"
        />
      </UCard>

      <!-- Daily memory files -->
      <UCard v-if="memoryFiles.length > 0 || memoryFilesLoading">
        <template #header>
          <div class="flex items-center gap-2">
            <UIcon name="i-lucide-calendar" class="text-dimmed" />
            <span class="font-semibold">Memory Files</span>
            <span class="text-xs text-dimmed">{{ memoryFiles.length }} files</span>
          </div>
        </template>

        <div v-if="memoryFilesLoading" class="flex h-24 items-center justify-center">
          <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
        </div>
        <template v-else>
          <div class="grid gap-4" style="grid-template-columns: 280px 1fr">
            <!-- File list -->
            <ul class="divide-y divide-default">
              <li
                v-for="entry in memoryFiles"
                :key="entry.path"
                class="flex cursor-pointer items-center justify-between px-2 py-1.5 hover:bg-accented rounded"
                :class="{ 'bg-accented': selectedMemFile === entry.path }"
                @click="openMemFile(entry)"
              >
                <div class="flex items-center gap-2 min-w-0">
                  <UIcon name="i-lucide-file-text" class="text-dimmed shrink-0" />
                  <span class="truncate text-sm">{{ entry.name }}</span>
                </div>
                <div class="flex items-center gap-2 shrink-0">
                  <span class="text-xs text-dimmed">{{ formatSize(entry.size) }}</span>
                  <UButton
                    icon="i-lucide-trash-2"
                    variant="ghost"
                    color="error"
                    size="xs"
                    @click.stop="confirmDelete(entry)"
                  />
                </div>
              </li>
            </ul>

            <!-- File editor -->
            <div>
              <div v-if="memFileLoading" class="flex h-48 items-center justify-center">
                <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
              </div>
              <div v-else-if="!selectedMemFile" class="flex h-48 items-center justify-center text-dimmed text-sm">
                Select a memory file to view or edit
              </div>
              <div v-else>
                <div class="flex items-center justify-between mb-2">
                  <span class="text-sm text-muted truncate">{{ selectedMemFile }}</span>
                  <div class="flex items-center gap-2">
                    <span v-if="memFileMsg" class="text-xs text-green-400">{{ memFileMsg }}</span>
                    <UButton
                      label="Save"
                      size="xs"
                      :disabled="!memFileDirty || memFileSaving"
                      @click="saveMemFile"
                    >
                      <template #leading>
                        <UIcon v-if="memFileSaving" name="i-lucide-loader-circle" class="size-4 animate-spin" />
                        <UIcon v-else name="i-lucide-save" class="size-4" />
                      </template>
                    </UButton>
                  </div>
                </div>
                <Codemirror
                  v-model="memFileContent"
                  :extensions="editorExtensions"
                  :style="{ minHeight: '12rem' }"
                  @update:model-value="memFileMsg = null"
                />
              </div>
            </div>
          </div>
        </template>
      </UCard>
    </template>

    <!-- Delete confirmation modal -->
    <UModal v-model:open="deleteOpen">
      <template #content>
        <div class="space-y-4 p-4">
          <h3 class="text-lg font-semibold">Delete Memory File</h3>
          <p class="text-sm text-muted">
            Are you sure you want to delete <strong>{{ deleteTarget?.name }}</strong>? This cannot be undone.
          </p>
          <div class="flex justify-end gap-2">
            <UButton label="Cancel" variant="ghost" color="neutral" @click="deleteOpen = false" />
            <UButton :label="deleting ? 'Deleting...' : 'Delete'" color="error" :disabled="deleting" @click="doDelete">
              <template v-if="deleting" #leading>
                <UIcon name="i-lucide-loader-circle" class="size-5 animate-spin" />
              </template>
            </UButton>
          </div>
        </div>
      </template>
    </UModal>
  </div>
</template>
