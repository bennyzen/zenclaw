export default defineNuxtConfig({
  modules: ['@nuxt/ui', '@nuxtjs/mdc'],

  // Static generation for GitHub Pages
  ssr: false,

  colorMode: {
    preference: 'dark',
    fallback: 'dark',
  },

  css: ['~/assets/css/main.css'],

  app: {
    head: {
      title: 'ZenClaw',
      meta: [
        { name: 'description', content: 'ZenClaw ESP32 Agent Manager' },
        { name: 'theme-color', content: '#18181b' },
      ],
      link: [
        { rel: 'manifest', href: '/manifest.json' },
      ],
    },
  },

  compatibilityDate: '2025-07-15',
  devtools: { enabled: true },

  vite: {
    optimizeDeps: {
      include: ['@vue/devtools-core', '@vue/devtools-kit', 'esptool-js'],
    },
  },
})
