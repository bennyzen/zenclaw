<template>
  <!-- ui.base override: UDashboardGroup defaults to `fixed inset-0` (it expects
       to BE the app shell). We already have a header/connection-banner/footer
       wrapping <main>, so swap `fixed` for `absolute` and let the parent <main>
       (set `relative` in app.vue) provide the containing block. -->
  <UDashboardGroup :ui="{ base: 'absolute inset-0 flex w-full overflow-hidden' }">
    <SessionsSidebar />
    <!-- ui.root override: kill the theme's `min-h-svh` (which inflates the
         panel to viewport height and pushes the message-input textarea below
         the fold) by including `min-h-0` so twmerge collapses the family.
         Keep flex-1 + min-w-0 so the page fills remaining width and can
         shrink inside the flex row. -->
    <UDashboardPanel :ui="{ root: 'relative flex flex-col flex-1 min-w-0 min-h-0 h-full' }">
      <slot />
    </UDashboardPanel>
  </UDashboardGroup>
</template>
