<script setup lang="ts">
const navItems = [
  { label: 'Dashboard', to: '/', icon: 'i-lucide-layout-dashboard' },
  { label: 'Chat', to: '/chat', icon: 'i-lucide-message-circle' },
  { label: 'Provision', to: '/provision', icon: 'i-lucide-cpu' },
  { label: 'Files', to: '/files', icon: 'i-lucide-folder' },
  { label: 'Config', to: '/config', icon: 'i-lucide-settings' },
  { label: 'Logs', to: '/logs', icon: 'i-lucide-scroll-text' },
]
</script>

<template>
  <UApp>
  <div class="flex min-h-screen flex-col bg-default text-default">
    <!-- Header -->
    <header class="flex items-center gap-4 border-b border-default bg-elevated px-4 py-3">
      <div class="flex items-center gap-2">
        <UIcon name="i-lucide-cpu" class="text-xl text-primary" />
        <span class="text-lg font-semibold">ZenClaw</span>
      </div>
      <nav class="flex items-center gap-1">
        <UButton
          v-for="item in navItems"
          :key="item.to"
          :to="item.to"
          :icon="item.icon"
          :label="item.label"
          variant="ghost"
          color="neutral"
          size="sm"
        />
      </nav>
      <div class="ml-auto">
        <UColorModeButton size="sm" variant="ghost" color="neutral" />
      </div>
    </header>

    <!-- Connection banner (shown when disconnected) -->
    <ConnectionBanner />

    <!-- Main content -->
    <main class="flex-1 p-4">
      <NuxtPage />
    </main>

    <!-- Footer -->
    <AppFooter />
  </div>
  </UApp>
</template>
