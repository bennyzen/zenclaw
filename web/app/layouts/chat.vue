<template>
  <div class="flex flex-1 overflow-hidden -m-4 h-[calc(100vh-8rem)]">
    <SessionsSidebar />
    <main class="flex-1 overflow-hidden">
      <slot />
    </main>
  </div>
</template>
