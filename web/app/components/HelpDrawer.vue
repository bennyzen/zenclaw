<script setup lang="ts">
const open = ref(false)
const route = useRoute()

const helpComponents: Record<string, string> = {
  '/': 'HelpDashboard',
  '/provision': 'HelpProvision',
  '/config': 'HelpConfig',
  '/chat': 'HelpChat',
  '/files': 'HelpFiles',
  '/logs': 'HelpLogs',
}

const currentHelp = computed(() => helpComponents[route.path] ?? 'HelpDashboard')
</script>

<template>
  <UButton
    icon="i-lucide-circle-help"
    size="sm"
    variant="ghost"
    color="neutral"
    aria-label="Help"
    @click="open = true"
  />
  <USlideover v-model:open="open" side="right" :ui="{ width: 'w-[480px]' }">
    <div class="p-6 overflow-y-auto h-full">
      <div class="flex items-center justify-between mb-6">
        <h2 class="text-lg font-semibold">Help</h2>
        <UButton icon="i-lucide-x" size="sm" variant="ghost" color="neutral" @click="open = false" />
      </div>
      <component :is="resolveComponent(currentHelp)" />
    </div>
  </USlideover>
</template>
