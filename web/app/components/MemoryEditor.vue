<script setup lang="ts">
import type { MemoryBlock } from '~/utils/memory'
import { MAX_TITLE_CHARS, parseTags } from '~/utils/memory'

const props = defineProps<{
  /** The block being edited; null when creating a new entry. */
  block: MemoryBlock | null
  open: boolean
  saving?: boolean
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  /**
   * Emitted when the user hits Save. Parent owns the actual file write.
   * - For new blocks, `id` and `timestamp` will be assigned by the parent.
   * - For edits, the parent uses `block.id` to locate the entry to replace.
   */
  save: [{ title: string; content: string; tags: string[] }]
}>()

const isNew = computed(() => props.block === null)

const draftTitle = ref('')
const draftContent = ref('')
const draftTagsRaw = ref('')

watch(() => props.open, (open) => {
  if (!open) return
  draftTitle.value = props.block?.title ?? ''
  draftContent.value = props.block?.content ?? ''
  draftTagsRaw.value = props.block?.tags.join(', ') ?? ''
})

const titleChars = computed(() => [...draftTitle.value].length)
const titleTooLong = computed(() => titleChars.value > MAX_TITLE_CHARS)
const titleEmpty = computed(() => draftTitle.value.trim().length === 0)
const canSave = computed(() => !titleEmpty.value && !titleTooLong.value)

function close() {
  emit('update:open', false)
}

function save() {
  if (!canSave.value) return
  emit('save', {
    title:   draftTitle.value.trim(),
    content: draftContent.value.trim(),
    tags:    parseTags(draftTagsRaw.value),
  })
}
</script>

<template>
  <USlideover
    :open="open"
    :title="isNew ? 'New memory' : 'Edit memory'"
    :description="isNew
      ? 'Capture something the agent should remember across conversations.'
      : (block?.id ?? '')"
    :ui="{ content: 'sm:max-w-2xl' }"
    @update:open="emit('update:open', $event)"
  >
    <template #body>
      <div class="space-y-5">
        <UFormField
          label="Title"
          name="title"
          required
          :hint="`Short label — ${titleChars}/${MAX_TITLE_CHARS} chars. Like a commit subject.`"
          :error="titleTooLong ? `Too long: max ${MAX_TITLE_CHARS} chars.` : undefined"
        >
          <UInput
            v-model="draftTitle"
            placeholder="Prefers explicit error handling"
            size="lg"
            :ui="{ root: 'w-full' }"
          />
        </UFormField>

        <UFormField
          label="Content"
          name="content"
          hint="Optional body — fuller detail, examples, context. A title-only one-liner is fine."
        >
          <UTextarea
            v-model="draftContent"
            placeholder="Hates unwrap() outside tests, prefers Result<T, E>."
            :rows="6"
            autoresize
            class="w-full"
          />
        </UFormField>

        <UFormField
          label="Tags"
          name="tags"
          hint="Comma-separated. Used for filtering and tag-scoped search."
        >
          <UInput
            v-model="draftTagsRaw"
            placeholder="preference, code-style"
            :ui="{ root: 'w-full' }"
          />
        </UFormField>

        <div v-if="!isNew && block" class="text-xs text-dimmed font-mono space-y-1">
          <p>id: {{ block.id }}</p>
          <p>created: {{ block.timestamp }}</p>
          <p class="text-dimmed/70">(id and timestamp are preserved on edit)</p>
        </div>
      </div>
    </template>

    <template #footer>
      <div class="flex items-center justify-end gap-2 w-full">
        <UButton label="Cancel" variant="ghost" color="neutral" @click="close" />
        <UButton
          :label="isNew ? 'Create' : 'Save'"
          :disabled="!canSave || saving"
          :loading="saving"
          @click="save"
        />
      </div>
    </template>
  </USlideover>
</template>
