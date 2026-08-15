import path from 'node:path'
import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

const dashboardBackend =
  process.env.HARNESS_E2E_DASHBOARD_URL ?? 'http://127.0.0.1:4173'
const root = fileURLToPath(new URL('.', import.meta.url))

export default defineConfig({
  base: './',
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(root, 'src'),
    },
  },
  server: {
    host: '0.0.0.0',
    allowedHosts: true,
    proxy: {
      '/api': dashboardBackend,
      '/runs': dashboardBackend,
      '/data.js': dashboardBackend,
      '/executions.js': dashboardBackend,
    },
  },
  test: {
    environment: 'node',
  },
})
