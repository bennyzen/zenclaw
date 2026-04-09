// https://nuxt.com/docs/api/configuration/nuxt-config
export default defineNuxtConfig({
  modules: ['@nuxt/ui', '@nuxtjs/mdc'],

  ssr: false,

  app: {
    baseURL: process.env.NUXT_APP_BASE_URL || '/',
    head: {
      title: 'ZenClaw',
      meta: [
        { name: 'description', content: 'ZenClaw ESP32 Agent Manager' },
        { name: 'theme-color', content: '#18181b' },
      ],
      link: [
        { rel: 'manifest', href: (process.env.NUXT_APP_BASE_URL || '/') + 'manifest.json' },
      ],
    },
  },

  colorMode: {
    preference: 'dark',
    fallback: 'dark',
  },

  css: ['~/assets/css/main.css'],

  compatibilityDate: '2025-07-15',

  devtools: { enabled: true },

  nitro: {
    prerender: {
      crawlLinks: false,
      routes: ['/'],
    },
  },

  vite: {
    optimizeDeps: {
      include: ['@vue/devtools-core', '@vue/devtools-kit', 'esptool-js', 'pretty-bytes'],
    },
  },
})
