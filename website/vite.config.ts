import netlify from '@netlify/vite-plugin-tanstack-start'
import { tanstackStart } from '@tanstack/react-start/plugin/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [
    tanstackStart(),
    netlify({
      // SSR uses Node functions; this starter has no Deno edge functions.
      dev: { edgeFunctions: { enabled: false } },
    }),
    react(),
  ],
  server: {
    port: 3000,
    strictPort: true,
  },
})
