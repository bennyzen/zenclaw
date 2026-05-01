<script setup lang="ts">
import type { MemoryBlock } from '~/utils/memory'
import { displayTitle, formatTimestamp } from '~/utils/memory'

const props = defineProps<{
  block: MemoryBlock
}>()

const emit = defineEmits<{ edit: []; delete: [] }>()

const headline = computed(() => displayTitle(props.block))
const isLegacy = computed(() => !props.block.title)
const ts = computed(() => formatTimestamp(props.block.timestamp))
const bytes = computed(() => new TextEncoder().encode(props.block.content).length)
const sizeLabel = computed(() => bytes.value < 1024
  ? `${bytes.value} B`
  : `${(bytes.value / 1024).toFixed(1)} KB`)

const shortId = computed(() => {
  const id = props.block.id
  return id.length <= 16 ? id : `${id.slice(0, 8)}…`
})

const expanded = ref(false)
const hasBody = computed(() => props.block.content.trim().length > 0)

const menuItems = computed(() => [
  [{ label: 'Edit',   icon: 'i-lucide-pencil',  onSelect: () => emit('edit') }],
  [{ label: 'Delete', icon: 'i-lucide-trash-2', color: 'error' as const, onSelect: () => emit('delete') }],
])
</script>

<template>
  <UCard
    :ui="{ root: 'overflow-hidden', body: 'sm:px-5 sm:py-4' }"
    variant="subtle"
  >
    <div class="flex items-start gap-3">
      <div class="min-w-0 flex-1 space-y-2">
        <div class="flex items-center gap-2 flex-wrap">
          <h3
            class="text-base font-semibold leading-snug"
            :class="isLegacy ? 'text-muted italic' : 'text-highlighted'"
          >
            {{ headline }}
          </h3>
          <UBadge
            v-if="isLegacy"
            label="untitled"
            variant="soft"
            color="warning"
            size="xs"
          />
        </div>

        <div v-if="block.tags.length > 0" class="flex items-center gap-1 flex-wrap">
          <UBadge
            v-for="t in block.tags"
            :key="t"
            :label="t"
            variant="soft"
            color="primary"
            size="xs"
          />
        </div>

        <div class="flex items-center gap-2 flex-wrap text-xs text-dimmed font-mono">
          <span :title="block.id">{{ shortId }}</span>
          <span>·</span>
          <span :title="block.timestamp">{{ ts }}</span>
          <span v-if="bytes > 0">·</span>
          <span v-if="bytes > 0">{{ sizeLabel }}</span>
        </div>

        <div v-if="hasBody" class="pt-1">
          <UButton
            :label="expanded ? 'Hide body' : 'Show body'"
            :trailing-icon="expanded ? 'i-lucide-chevron-up' : 'i-lucide-chevron-down'"
            variant="ghost"
            color="neutral"
            size="xs"
            @click="expanded = !expanded"
          />
          <p
            v-if="expanded"
            class="mt-2 text-sm whitespace-pre-wrap leading-relaxed text-muted border-l-2 border-default pl-3"
          >{{ block.content }}</p>
        </div>
      </div>

      <UDropdownMenu :items="menuItems" :content="{ align: 'end' }">
        <UButton
          icon="i-lucide-ellipsis-vertical"
          variant="ghost"
          color="neutral"
          size="xs"
        />
      </UDropdownMenu>
    </div>
  </UCard>
</template>
