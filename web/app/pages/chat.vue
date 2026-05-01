<script setup lang="ts">
import type { ChatEvent } from '~/types/connection'

const { state, openChatStream, getChatHistory } = useConnection()

type TimelineItem =
  | { kind: 'user'; id: string; text: string }
  | { kind: 'assistant'; id: string; text: string }
  | {
      kind: 'tool'
      id: string
      name: string
      args: unknown
      status: 'pending' | 'ok' | 'error'
      result?: string
      error?: string
      expanded: boolean
    }
  | { kind: 'error'; id: string; error: string }

const items = ref<TimelineItem[]>([])
const input = ref('')
const sending = ref(false)
const thinking = ref(false)
const loading = ref(false)
const fatalError = ref<string | null>(null)
let nextId = 1
let stream: ReturnType<typeof openChatStream> | null = null

const messageContainer = ref<HTMLElement | null>(null)

function newId() {
  return `i${nextId++}`
}

function applyEvent(evt: ChatEvent) {
  switch (evt.type) {
    case 'user_message':
      items.value.push({ kind: 'user', id: newId(), text: evt.text })
      break
    case 'assistant_text':
      items.value.push({ kind: 'assistant', id: newId(), text: evt.text })
      thinking.value = false
      break
    case 'tool_call_started':
      thinking.value = false
      items.value.push({
        kind: 'tool',
        id: evt.id,
        name: evt.name,
        args: evt.args,
        status: 'pending',
        expanded: false,
      })
      break
    case 'tool_call_finished': {
      const idx = items.value.findIndex(
        (i) => i.kind === 'tool' && i.id === evt.id,
      )
      if (idx >= 0) {
        const it = items.value[idx]
        if (it && it.kind === 'tool') {
          it.status = evt.ok ? 'ok' : 'error'
          it.result = evt.result
          it.error = evt.error
        }
      }
      break
    }
    case 'thinking_started':
      thinking.value = true
      break
    case 'thinking_ended':
      thinking.value = false
      break
    case 'done':
      thinking.value = false
      sending.value = false
      stream?.close()
      stream = null
      break
    case 'error':
      thinking.value = false
      sending.value = false
      items.value.push({ kind: 'error', id: newId(), error: evt.error })
      stream?.close()
      stream = null
      break
    case 'cancel':
      // outbound only — never received from server
      break
  }
  scrollToBottom()
}

function scrollToBottom() {
  nextTick(() => {
    const el = messageContainer.value
    if (el) el.scrollTop = el.scrollHeight
  })
}

async function loadHistory() {
  loading.value = true
  try {
    const result = await getChatHistory('web', 200)
    items.value = []
    for (const evt of result.events) {
      // Reuse the live-event reducer so history and live render identically.
      applyEvent(evt)
    }
  } catch {
    items.value = []
  }
  loading.value = false
  scrollToBottom()
}

function ensureStream() {
  if (stream && stream.isOpen()) return
  stream = openChatStream(applyEvent, 'web')
}

function send() {
  const text = input.value.trim()
  if (!text || sending.value) return
  input.value = ''
  fatalError.value = null
  sending.value = true
  thinking.value = true
  // Optimistically render the user bubble — the server doesn't echo it back
  // on the live WS path.
  items.value.push({ kind: 'user', id: newId(), text })
  scrollToBottom()
  ensureStream()
  stream!.send(text)
}

function cancelTurn() {
  if (!stream) return
  stream.cancel()
}

function onSubmitKey(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    send()
  }
}

function toggleTool(item: TimelineItem) {
  if (item.kind === 'tool') item.expanded = !item.expanded
}

function formatArgs(args: unknown): string {
  try {
    return JSON.stringify(args, null, 2)
  } catch {
    return String(args)
  }
}

function clamp(text: string | undefined, lines: number): string {
  if (!text) return ''
  const all = text.split('\n')
  if (all.length <= lines) return text
  return all.slice(0, lines).join('\n') + `\n… (${all.length - lines} more lines)`
}

onMounted(() => {
  if (state.networkConnected) loadHistory()
})

watch(
  () => state.networkConnected,
  (connected) => {
    if (connected) loadHistory()
    else {
      stream?.close()
      stream = null
    }
  },
)

onBeforeUnmount(() => {
  stream?.close()
  stream = null
})
</script>

<template>
  <div class="flex flex-col h-[calc(100vh-8rem)]">
    <div class="flex items-center justify-between mb-4">
      <h1 class="text-2xl font-bold">Chat</h1>
      <UButton
        v-if="sending"
        size="xs"
        color="error"
        variant="soft"
        icon="i-lucide-square"
        @click="cancelTurn"
      >
        Cancel
      </UButton>
    </div>

    <template v-if="state.networkConnected">
      <div v-if="loading" class="flex justify-center py-8">
        <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
      </div>

      <template v-else>
        <div
          ref="messageContainer"
          class="flex-1 min-h-0 overflow-y-auto space-y-2 pr-1"
        >
          <template v-for="item in items" :key="item.id">
            <div v-if="item.kind === 'user'" class="flex justify-end">
              <div class="bg-primary-500/10 text-default rounded-lg px-3 py-2 max-w-[80%] whitespace-pre-wrap">
                {{ item.text }}
              </div>
            </div>

            <div v-else-if="item.kind === 'assistant'" class="flex items-start gap-2">
              <UIcon name="i-lucide-bot" class="text-dimmed mt-1 shrink-0" />
              <div class="bg-elevated rounded-lg px-3 py-2 max-w-[80%]">
                <MDC :value="item.text" :cache-key="item.id" class="*:first:mt-0 *:last:mb-0" />
              </div>
            </div>

            <div v-else-if="item.kind === 'tool'" class="flex items-start gap-2 ml-6">
              <button
                type="button"
                class="flex items-center gap-2 text-xs text-dimmed hover:text-default border border-default rounded px-2 py-1 transition-colors"
                @click="toggleTool(item)"
              >
                <UIcon
                  v-if="item.status === 'pending'"
                  name="i-lucide-loader-2"
                  class="animate-spin"
                />
                <UIcon
                  v-else-if="item.status === 'ok'"
                  name="i-lucide-wrench"
                  class="text-success"
                />
                <UIcon
                  v-else
                  name="i-lucide-circle-alert"
                  class="text-error"
                />
                <span class="font-mono">{{ item.name }}</span>
                <UIcon
                  :name="item.expanded ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
                  class="size-3"
                />
              </button>

              <div v-if="item.expanded" class="flex-1 min-w-0 space-y-1">
                <details open class="text-xs">
                  <summary class="cursor-pointer text-dimmed">args</summary>
                  <pre class="bg-muted rounded p-2 overflow-x-auto text-xs whitespace-pre-wrap break-all">{{ formatArgs(item.args) }}</pre>
                </details>
                <details v-if="item.status !== 'pending'" open class="text-xs">
                  <summary class="cursor-pointer text-dimmed">
                    {{ item.status === 'ok' ? 'result' : 'error' }}
                  </summary>
                  <pre class="bg-muted rounded p-2 overflow-x-auto text-xs whitespace-pre-wrap break-all">{{ clamp(item.status === 'ok' ? item.result : item.error, 25) }}</pre>
                </details>
              </div>
            </div>

            <div v-else-if="item.kind === 'error'" class="flex items-start gap-2">
              <UIcon name="i-lucide-circle-alert" class="text-error mt-1" />
              <div class="text-error text-sm">{{ item.error }}</div>
            </div>
          </template>

          <div v-if="thinking" class="flex items-center gap-2 ml-6 text-dimmed text-xs">
            <span class="size-2 rounded-full bg-zinc-400 animate-bounce" style="animation-delay: 0ms" />
            <span class="size-2 rounded-full bg-zinc-400 animate-bounce" style="animation-delay: 150ms" />
            <span class="size-2 rounded-full bg-zinc-400 animate-bounce" style="animation-delay: 300ms" />
            <span>thinking</span>
          </div>
        </div>

        <p v-if="fatalError" class="text-sm text-red-400 py-2">{{ fatalError }}</p>

        <div class="mt-3 flex gap-2 items-end">
          <UTextarea
            v-model="input"
            placeholder="Send a message…"
            :rows="1"
            autoresize
            class="flex-1"
            @keydown="onSubmitKey"
          />
          <UButton
            icon="i-lucide-send"
            :loading="sending"
            :disabled="sending || !input.trim()"
            @click="send"
          />
        </div>
      </template>
    </template>
  </div>
</template>
