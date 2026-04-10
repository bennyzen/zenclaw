<script setup lang="ts">
import type { FileEntry } from '~/types/connection'
import { Codemirror } from 'vue-codemirror'
import { python } from '@codemirror/lang-python'
import { json as jsonLang } from '@codemirror/lang-json'
import { markdown } from '@codemirror/lang-markdown'
import { oneDark } from '@codemirror/theme-one-dark'

function langForFile(path: string) {
  if (path.endsWith('.py')) return python()
  if (path.endsWith('.json')) return jsonLang()
  if (path.endsWith('.md')) return markdown()
  return []
}

const editorExtensions = computed(() => [
  oneDark,
  ...(selectedFile.value ? [langForFile(selectedFile.value)].flat() : []),
])

const {
  state, listDir, readFile, writeFile, deleteFile, createDir, uploadFile,
  listCloudDir, readCloudFile, writeCloudFile, deleteCloudFile, uploadCloudFile,
} = useConnection()

// Storage mode
const storageMode = ref<'device' | 'cloud'>('device')
const isCloud = computed(() => storageMode.value === 'cloud')
const cloudConfigured = computed(() => state.lastStatus?.cloudStorage?.configured ?? false)

const currentPath = ref('.')
const entries = ref<FileEntry[]>([])
const loading = ref(false)
const error = ref<string | null>(null)

const toast = useToast()

// Editor state
const selectedFile = ref<string | null>(null)
const editorContent = ref('')
const editorDirty = ref(false)
const openingFile = ref(false)
const saving = ref(false)
const saveMsg = ref<string | null>(null)

// Modal state
const createOpen = ref(false)
const createType = ref<'file' | 'dir'>('file')
const createName = ref('')
const creating = ref(false)

// Delete confirmation
const deleteTarget = ref<FileEntry | null>(null)
const deleteOpen = ref(false)
const deleting = ref(false)

// Upload
const uploadInput = ref<HTMLInputElement | null>(null)

function switchMode(mode: 'device' | 'cloud') {
  if (storageMode.value === mode) return
  storageMode.value = mode
  selectedFile.value = null
  editorContent.value = ''
  editorDirty.value = false
  saveMsg.value = null
  error.value = null
  currentPath.value = mode === 'cloud' ? '' : '.'
  loadDir(currentPath.value)
}

async function loadDir(path: string) {
  loading.value = true
  error.value = null
  try {
    if (isCloud.value) {
      const result = await listCloudDir(path)
      currentPath.value = result.path
      entries.value = result.entries
    } else {
      const result = await listDir(path)
      currentPath.value = result.path
      entries.value = result.entries
    }
  } catch (e: any) {
    error.value = e.message
  }
  loading.value = false
}

function goUp() {
  if (isCloud.value) {
    // Cloud: remove last path segment from prefix
    const trimmed = currentPath.value.replace(/\/$/, '')
    const slash = trimmed.lastIndexOf('/')
    const parent = slash >= 0 ? trimmed.substring(0, slash + 1) : ''
    loadDir(parent)
  } else {
    const parts = currentPath.value.replace(/\\/g, '/').split('/')
    parts.pop()
    const parent = parts.length ? parts.join('/') : '.'
    loadDir(parent)
  }
}

async function openFile(entry: FileEntry) {
  if (entry.isDir) {
    loadDir(entry.path)
    return
  }
  error.value = null
  openingFile.value = true
  try {
    if (isCloud.value) {
      const result = await readCloudFile(entry.path)
      selectedFile.value = result.path
      editorContent.value = result.content
    } else {
      const result = await readFile(entry.path)
      selectedFile.value = result.path
      editorContent.value = result.content
    }
    editorDirty.value = false
    saveMsg.value = null
  } catch (e: any) {
    error.value = `Cannot read file: ${e.message}`
  }
  openingFile.value = false
}

async function save() {
  if (!selectedFile.value) return
  saving.value = true
  saveMsg.value = null
  try {
    if (isCloud.value) {
      await writeCloudFile(selectedFile.value, editorContent.value)
    } else {
      await writeFile(selectedFile.value, editorContent.value)
    }
    editorDirty.value = false
    saveMsg.value = 'Saved'
    await loadDir(currentPath.value)
  } catch (e: any) {
    error.value = `Save failed: ${e.message}`
  }
  saving.value = false
}

function confirmDelete(entry: FileEntry) {
  deleteTarget.value = entry
  deleteOpen.value = true
}

async function doDelete() {
  if (!deleteTarget.value) return
  deleting.value = true
  try {
    if (isCloud.value) {
      await deleteCloudFile(deleteTarget.value.path)
    } else {
      await deleteFile(deleteTarget.value.path)
    }
    if (selectedFile.value === deleteTarget.value.path) {
      selectedFile.value = null
      editorContent.value = ''
    }
    await loadDir(currentPath.value)
  } catch (e: any) {
    error.value = `Delete failed: ${e.message}`
  }
  deleting.value = false
  deleteOpen.value = false
  deleteTarget.value = null
}

function openCreateModal(type: 'file' | 'dir') {
  createType.value = type
  createName.value = ''
  createOpen.value = true
}

async function doCreate() {
  if (!createName.value.trim()) return
  creating.value = true
  const name = createName.value.trim()
  try {
    if (isCloud.value) {
      // Cloud: write an empty file at the key
      const key = currentPath.value + name
      await writeCloudFile(key, '')
    } else {
      const path = currentPath.value === '.'
        ? name
        : `${currentPath.value}/${name}`
      if (createType.value === 'dir') {
        await createDir(path)
      } else {
        await writeFile(path, '')
      }
    }
    await loadDir(currentPath.value)
    createOpen.value = false
  } catch (e: any) {
    error.value = `Create failed: ${e.message}`
  }
  creating.value = false
}

function triggerUpload() {
  uploadInput.value?.click()
}

async function handleUpload(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  try {
    const data = await file.arrayBuffer()
    if (isCloud.value) {
      const key = currentPath.value + file.name
      await uploadCloudFile(key, data)
    } else {
      const path = currentPath.value === '.'
        ? file.name
        : `${currentPath.value}/${file.name}`
      try {
        await uploadFile(path, data)
      } catch (e: any) {
        if (data.byteLength > 256 * 1024 || e.message?.includes('too large')) {
          const key = currentPath.value === '.'
            ? file.name
            : `${currentPath.value}/${file.name}`
          await uploadCloudFile(key, data)
          storageMode.value = 'cloud'
          currentPath.value = ''
          toast.add({ title: 'Uploaded to cloud storage', description: 'File too large for device storage — saved to cloud instead.', color: 'info', icon: 'i-lucide-cloud-upload' })
        } else {
          throw e
        }
      }
    }
    await loadDir(currentPath.value)
  } catch (e: any) {
    error.value = `Upload failed: ${e.message}`
  }
  input.value = ''
}

async function downloadFile(entry: FileEntry) {
  try {
    let content: string
    if (isCloud.value) {
      const result = await readCloudFile(entry.path)
      content = result.content
    } else {
      const result = await readFile(entry.path)
      content = result.content
    }
    const blob = new Blob([content], { type: 'text/plain' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = entry.name
    a.click()
    URL.revokeObjectURL(url)
  } catch (e: any) {
    error.value = `Download failed: ${e.message}`
  }
}

function formatSize(size: number | null): string {
  if (size == null) return ''
  if (size < 1024) return `${size} B`
  return `${(size / 1024).toFixed(1)} KB`
}

const canGoUp = computed(() => {
  if (isCloud.value) return currentPath.value !== ''
  return currentPath.value !== '.'
})

const displayPath = computed(() => {
  if (isCloud.value) return currentPath.value || '/'
  return currentPath.value
})

// Load root on mount
onMounted(() => {
  if (state.networkConnected) {
    loadDir('.')
  }
})

watch(() => state.networkConnected, (connected) => {
  if (connected) loadDir(storageMode.value === 'cloud' ? '' : '.')
})
</script>

<template>
  <div class="space-y-4">
        <div class="flex items-center justify-between">
      <h1 class="text-2xl font-bold">File Manager</h1>

      <div v-if="state.networkConnected" class="flex items-center gap-1">
        <UTooltip text="On-device storage. Best for small files and configs.">
          <UButton
            icon="i-lucide-hard-drive"
            :label="'Device'"
            size="xs"
            :variant="storageMode === 'device' ? 'soft' : 'ghost'"
            :color="storageMode === 'device' ? 'primary' : 'neutral'"
            @click="switchMode('device')"
          />
        </UTooltip>
        <UTooltip text="Cloud storage (R2). Large files go here automatically.">
          <UButton
            icon="i-lucide-cloud"
            :label="'Cloud'"
            size="xs"
            :variant="storageMode === 'cloud' ? 'soft' : 'ghost'"
            :color="storageMode === 'cloud' ? 'primary' : 'neutral'"
            :disabled="!cloudConfigured"
            @click="switchMode('cloud')"
          />
        </UTooltip>
      </div>
    </div>

    <p class="text-sm text-dimmed">
      Files up to 256 KB are stored on the device. Larger files are automatically uploaded to cloud storage and won't use device space. The agent can read and search cloud files of any size.
    </p>

    <template v-if="state.networkConnected">
      <UAlert v-if="error" icon="i-lucide-circle-x" color="error" variant="subtle" :description="error" />

      <div class="grid gap-4" style="grid-template-columns: 360px 1fr">
        <!-- Directory browser -->
        <UCard class="min-h-96">
          <template #header>
            <div class="flex items-center justify-between">
              <div class="flex items-center gap-2">
                <UButton
                  icon="i-lucide-arrow-up"
                  variant="ghost"
                  color="neutral"
                  size="xs"
                  :disabled="!canGoUp"
                  @click="goUp"
                />
                <span class="text-sm text-muted truncate">{{ displayPath }}</span>
              </div>
              <div class="flex gap-1">
                <UButton
                  icon="i-lucide-file-plus"
                  variant="ghost"
                  color="neutral"
                  size="xs"
                  @click="openCreateModal('file')"
                />
                <UButton
                  v-if="!isCloud"
                  icon="i-lucide-folder-plus"
                  variant="ghost"
                  color="neutral"
                  size="xs"
                  @click="openCreateModal('dir')"
                />
                <UButton
                  icon="i-lucide-upload"
                  variant="ghost"
                  color="neutral"
                  size="xs"
                  @click="triggerUpload"
                />
                <input
                  ref="uploadInput"
                  type="file"
                  class="hidden"
                  @change="handleUpload"
                >
              </div>
            </div>
          </template>

          <div v-if="loading" class="flex justify-center py-8">
            <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
          </div>

          <ul v-else class="divide-y divide-default">
            <li
              v-for="entry in entries"
              :key="entry.path"
              class="flex cursor-pointer items-center justify-between px-2 py-1.5 hover:bg-accented rounded"
              :class="{ 'bg-accented': selectedFile === entry.path }"
              @click="openFile(entry)"
            >
              <div class="flex items-center gap-2 min-w-0">
                <UIcon
                  :name="entry.isDir ? 'i-lucide-folder' : 'i-lucide-file'"
                  :class="entry.isDir ? 'text-yellow-500' : 'text-dimmed'"
                />
                <span class="truncate text-sm">{{ entry.name }}</span>
              </div>
              <div class="flex items-center gap-2 shrink-0">
                <span v-if="!entry.isDir" class="text-xs text-dimmed">
                  {{ formatSize(entry.size) }}
                </span>
                <UButton
                  v-if="!entry.isDir"
                  icon="i-lucide-download"
                  variant="ghost"
                  color="neutral"
                  size="xs"
                  @click.stop="downloadFile(entry)"
                />
                <UButton
                  icon="i-lucide-trash-2"
                  variant="ghost"
                  color="error"
                  size="xs"
                  @click.stop="confirmDelete(entry)"
                />
              </div>
            </li>
            <li v-if="!entries.length" class="py-4 text-center text-sm text-dimmed">
              {{ isCloud ? 'No objects' : 'Empty directory' }}
            </li>
          </ul>
        </UCard>

        <!-- Text editor -->
        <UCard class="min-h-96">
          <template #header>
            <div class="flex items-center justify-between">
              <span class="text-sm text-muted truncate">
                {{ selectedFile || 'No file selected' }}
              </span>
              <div v-if="selectedFile" class="flex items-center gap-2">
                <span v-if="saveMsg" class="text-xs text-green-400">{{ saveMsg }}</span>
                <UButton
                  :label="saving ? 'Saving...' : 'Save'"
                  size="xs"
                  :disabled="!editorDirty || saving"
                  @click="save"
                >
                  <template #leading>
                    <UIcon v-if="saving" name="i-lucide-loader-circle" class="size-4 animate-spin" />
                    <UIcon v-else name="i-lucide-save" class="size-4" />
                  </template>
                </UButton>
              </div>
            </div>
          </template>

          <div v-if="openingFile" class="flex h-96 items-center justify-center">
            <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
          </div>
          <div v-else-if="!selectedFile" class="flex h-96 items-center justify-center text-dimmed">
            Select a file to edit
          </div>
          <Codemirror
            v-else
            v-model="editorContent"
            :extensions="editorExtensions"
            :style="{ height: '100%', minHeight: '24rem' }"
            @update:model-value="editorDirty = true; saveMsg = null"
          />
        </UCard>
      </div>
    </template>

    <!-- Create modal -->
    <UModal v-model:open="createOpen">
      <template #content>
        <div class="space-y-4 p-4">
          <h3 class="text-lg font-semibold">
            {{ isCloud ? 'Create File' : `Create ${createType === 'dir' ? 'Directory' : 'File'}` }}
          </h3>
          <UFormField label="Name">
            <UInput v-model="createName" :placeholder="isCloud ? 'new-file.txt' : (createType === 'dir' ? 'new-folder' : 'new-file.txt')" />
          </UFormField>
          <div class="flex justify-end gap-2">
            <UButton label="Cancel" variant="ghost" color="neutral" @click="createOpen = false" />
            <UButton :label="creating ? 'Creating...' : 'Create'" :disabled="creating" @click="doCreate">
              <template v-if="creating" #leading>
                <UIcon name="i-lucide-loader-circle" class="size-5 animate-spin" />
              </template>
            </UButton>
          </div>
        </div>
      </template>
    </UModal>

    <!-- Delete confirmation modal -->
    <UModal v-model:open="deleteOpen">
      <template #content>
        <div class="space-y-4 p-4">
          <h3 class="text-lg font-semibold">Delete {{ deleteTarget?.isDir ? 'Directory' : 'File' }}</h3>
          <p class="text-sm text-muted">
            Are you sure you want to delete <strong>{{ deleteTarget?.name }}</strong>?
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
