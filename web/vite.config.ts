import path from 'node:path'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes('node_modules')) return
          if (id.includes('@assistant-ui')) return 'vendor-assistant-ui'
          if (id.includes('streamdown') || id.includes('shiki')) return 'vendor-streamdown'
          if (id.includes('@radix-ui')) return 'vendor-radix'
          if (id.includes('@xterm') || id.includes('/xterm/')) return 'vendor-xterm'
          if (id.includes('react-markdown') || id.includes('remark-')) {
            if (id.includes('streamdown') || id.includes('shiki')) return 'vendor-streamdown'
            return 'vendor-markdown'
          }
          if (id.includes('react-dom')) return 'vendor-react'
          if (id.includes('/react/')) return 'vendor-react'
          if (id.includes('@tanstack')) return 'vendor-query'
        },
      },
    },
  },
})
