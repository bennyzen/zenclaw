<script setup lang="ts">
const { state, sendChatStream, getChatHistory } = useConnection()

interface Message {
  id: string
  role: 'user' | 'assistant'
  parts: { type: 'text'; text: string }[]
}

const messages = ref<Message[]>([])
const input = ref('')
const sending = ref(false)
const loading = ref(false)
const error = ref<string | null>(null)
let nextId = 1

function addMessage(role: 'user' | 'assistant', text: string): Message {
  const msg: Message = {
    id: String(nextId++),
    role,
    parts: [{ type: 'text', text }],
  }
  messages.value.push(msg)
  return msg
}

async function loadHistory() {
  loading.value = true
  try {
    const result = await getChatHistory('web', 50)
    messages.value = result.messages.map((m) => ({
      id: String(nextId++),
      role: m.role as 'user' | 'assistant',
      parts: [{ type: 'text', text: m.content }],
    }))
  } catch {
    // No history available
  }
  loading.value = false
}

function send() {
  const text = input.value.trim()
  if (!text || sending.value) return

  input.value = ''
  error.value = null
  addMessage('user', text)
  sending.value = true

  // Create an empty assistant message to stream into
  const assistantMsg = addMessage('assistant', '')
  const msgIdx = messages.value.length - 1

  sendChatStream(
    text,
    (delta) => {
      const msg = messages.value[msgIdx]
      if (msg) {
        msg.parts = [{ type: 'text', text: (msg.parts[0]?.text || '') + delta }]
      }
    },
    (_fullText) => {
      sending.value = false
    },
    (err) => {
      error.value = err
      if (!messages.value[msgIdx]?.parts[0]?.text) {
        messages.value.splice(msgIdx, 1)
      }
      sending.value = false
    },
  )
}

onMounted(() => {
  if (state.networkConnected) loadHistory()
})

watch(() => state.networkConnected, (connected) => {
  if (connected) loadHistory()
})

const chatStatus = computed(() => {
  if (sending.value) return 'streaming' as const
  return 'ready' as const
})
</script>

<template>
  <div class="flex flex-col h-[calc(100vh-8rem)]">
    <h1 class="text-2xl font-bold mb-4">Chat</h1>

    <template v-if="state.networkConnected">
      <div v-if="loading" class="flex justify-center py-8">
        <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
      </div>

      <template v-else>
        <UChatMessages
          :messages="messages"
          :status="chatStatus"
          class="flex-1 min-h-0 overflow-y-auto"
          :user="{ variant: 'soft', side: 'right' }"
          :assistant="{ variant: 'soft', side: 'left', icon: 'i-lucide-bot' }"
        >
          <template #content="{ message }">
            <div v-if="message.role === 'assistant' && !message.parts[0]?.text" class="flex items-center gap-1 py-1">
              <span class="size-2 rounded-full bg-zinc-400 animate-bounce" style="animation-delay: 0ms" />
              <span class="size-2 rounded-full bg-zinc-400 animate-bounce" style="animation-delay: 150ms" />
              <span class="size-2 rounded-full bg-zinc-400 animate-bounce" style="animation-delay: 300ms" />
            </div>
            <template v-else v-for="(part, index) in message.parts" :key="`${message.id}-${part.type}-${index}`">
              <MDC
                v-if="part.type === 'text' && message.role === 'assistant'"
                :value="part.text"
                :cache-key="`${message.id}-${index}`"
                class="*:first:mt-0 *:last:mb-0"
              />
              <p v-else-if="part.type === 'text'" class="whitespace-pre-wrap">{{ part.text }}</p>
            </template>
          </template>
        </UChatMessages>

        <p v-if="error" class="text-sm text-red-400 py-2">{{ error }}</p>

        <UChatPrompt
          v-model="input"
          placeholder="Send a message..."
          :disabled="sending"
          size="xl"
          @submit="send"
        >
          <UChatPromptSubmit
            :status="chatStatus"
            size="xl"
          />
        </UChatPrompt>
      </template>
    </template>
  </div>
</template>
