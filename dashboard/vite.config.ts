import path from 'node:path'
import { fileURLToPath } from 'node:url'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

const root = fileURLToPath(new URL('.', import.meta.url))

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(root, 'src'),
      '@iii-dev/console-ui': path.resolve(root, 'test/console-ui.tsx'),
    },
  },
  test: {
    environment: 'node',
  },
})
