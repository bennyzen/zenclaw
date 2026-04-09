<script setup lang="ts">
const open = ref(false)
const route = useRoute()

const routeHelp: Record<string, string> = {
  '/': 'dashboard',
  '/dashboard': 'dashboard',
  '/provision': 'provision',
  '/config': 'config',
  '/chat': 'chat',
  '/files': 'files',
  '/logs': 'logs',
}

const currentHelp = computed(() => routeHelp[route.path] ?? 'dashboard')
</script>

<template>
  <div>
    <UButton
      icon="i-lucide-circle-help"
      size="sm"
      variant="ghost"
      color="neutral"
      aria-label="Help"
      @click="open = true"
    />
    <USlideover v-model:open="open" side="right">
      <template #body>
        <HelpDashboard v-if="currentHelp === 'dashboard'" />
        <HelpProvision v-else-if="currentHelp === 'provision'" />
        <HelpConfig v-else-if="currentHelp === 'config'" />
        <HelpChat v-else-if="currentHelp === 'chat'" />
        <HelpFiles v-else-if="currentHelp === 'files'" />
        <HelpLogs v-else-if="currentHelp === 'logs'" />
      </template>
    </USlideover>
  </div>
</template>
