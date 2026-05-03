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
const messagesInner = ref<HTMLElement | null>(null)
const stickToBottom = ref(true)
let lastScrollHeight = 0
let resizeObserver: ResizeObserver | null = null

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
}

function isNearBottom() {
  const el = messageContainer.value
  if (!el) return true
  return el.scrollHeight - el.scrollTop - el.clientHeight < 80
}

function onScroll() {
  stickToBottom.value = isNearBottom()
}

// MDC renders markdown asynchronously (after nextTick), so measuring
// scrollHeight from a Vue effect lands mid-stream. Observe the messages
// wrapper instead — any layout-affecting change (new bubble, MDC late
// render, tool-widget expansion) fires here once the DOM has settled.
watch(messagesInner, (el, _prev, onCleanup) => {
  resizeObserver?.disconnect()
  resizeObserver = null
  if (!el) return
  lastScrollHeight = 0
  resizeObserver = new ResizeObserver(() => {
    if (!stickToBottom.value) return
    const c = messageContainer.value
    if (!c) return
    const delta = c.scrollHeight - lastScrollHeight
    lastScrollHeight = c.scrollHeight
    // Big jump (initial load, large message arriving) → instant, so we
    // don't burn a long animated scroll from top to bottom. Small jump
    // (token streaming, single bubble) → smooth, for that polished feel.
    c.scrollTo({
      top: c.scrollHeight,
      behavior: delta > 200 ? 'auto' : 'smooth',
    })
  })
  resizeObserver.observe(el)
  onCleanup(() => {
    resizeObserver?.disconnect()
    resizeObserver = null
  })
})

async function loadHistory() {
  loading.value = true
  stickToBottom.value = true
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
  // The ResizeObserver attached after `loading` flips will scroll to bottom
  // automatically as MDC content lays out — no manual scroll needed here.
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
  // User just engaged — re-anchor to the bottom even if they had scrolled up.
  // The ResizeObserver handles the actual scroll once layout settles.
  stickToBottom.value = true
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
          class="flex-1 min-h-0 overflow-y-auto pr-1"
          @scroll.passive="onScroll"
        >
          <div ref="messagesInner" class="space-y-2">
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

            <UCollapsible
              v-else-if="item.kind === 'tool'"
              v-model:open="item.expanded"
              class="ml-6 max-w-[80%]"
            >
              <template #default="{ open }">
                <UButton
                  color="neutral"
                  variant="subtle"
                  size="xs"
                  :ui="{ base: 'font-mono' }"
                >
                  <template #leading>
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
                  </template>
                  {{ item.name }}
                  <template #trailing>
                    <UIcon
                      :name="open ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
                      class="size-3.5 text-dimmed"
                    />
                  </template>
                </UButton>
              </template>

              <template #content>
                <div class="mt-2 space-y-3 rounded-md border border-default bg-elevated/40 p-3">
                  <div>
                    <p class="mb-1 text-[11px] font-medium uppercase tracking-wide text-dimmed">args</p>
                    <pre class="overflow-x-auto whitespace-pre-wrap break-all rounded bg-default p-2 text-xs leading-relaxed">{{ formatArgs(item.args) }}</pre>
                  </div>
                  <div v-if="item.status !== 'pending'">
                    <p
                      class="mb-1 text-[11px] font-medium uppercase tracking-wide"
                      :class="item.status === 'ok' ? 'text-dimmed' : 'text-error'"
                    >
                      {{ item.status === 'ok' ? 'result' : 'error' }}
                    </p>
                    <pre class="overflow-x-auto whitespace-pre-wrap break-all rounded bg-default p-2 text-xs leading-relaxed">{{ clamp(item.status === 'ok' ? item.result : item.error, 25) }}</pre>
                  </div>
                </div>
              </template>
            </UCollapsible>

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
