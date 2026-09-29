import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { readFileSync } from 'node:fs'
import { fileURLToPath, URL } from 'node:url'

const appVersion = readFileSync(new URL('../VERSION', import.meta.url), 'utf8').trim()

// During `pnpm dev` the UI runs on Vite's port and proxies API calls to a
// locally running gateway (`make run`). In production the Go binary serves
// the built files itself, so everything is same-origin.
const gateway = process.env.HEIGATE_DEV_TARGET ?? 'http://127.0.0.1:8080'
const devGatewayURL = `${gateway.replace(/\/+$/, '')}/v1`

export default defineConfig({
  plugins: [vue()],
  define: {
    __HEIGATE_DEV_GATEWAY_URL__: JSON.stringify(devGatewayURL),
    __HEIGATE_APP_VERSION__: JSON.stringify(appVersion),
  },
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  build: {
    // The Go binary embeds this directory with -tags embed.
    outDir: '../internal/web/dist',
    emptyOutDir: false,
    target: 'es2022',
    chunkSizeWarningLimit: 900,
  },
  server: {
    port: 5273,
    strictPort: true,
    proxy: {
      '/api': { target: gateway, changeOrigin: false },
      '/health': { target: gateway, changeOrigin: false },
      '/v1': { target: gateway, changeOrigin: false },
    },
  },
})
