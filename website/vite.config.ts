import { tanstackStart } from '@tanstack/react-start/plugin/vite'
import react from '@vitejs/plugin-react'
import { defineConfig, loadEnv } from 'vite'
import { siteOrigin } from './src/lib/site-url.ts'

export default defineConfig(({ command, mode }) => {
  const origin = siteOrigin(
    loadEnv(mode, process.cwd()).VITE_SITE_URL,
    command === 'build',
  )
  return {
    define: { 'import.meta.env.VITE_SITE_URL': JSON.stringify(origin) },
    plugins: [
      {
        name: 'explorer-crawler-files',
        apply: 'build',
        generateBundle() {
          if (this.environment.name !== 'client') return
          this.emitFile({
            type: 'asset',
            fileName: 'robots.txt',
            source: `User-agent: *\nAllow: /\n\nSitemap: ${origin}/sitemap.xml\n`,
          })
          this.emitFile({
            type: 'asset',
            fileName: 'sitemap.xml',
            source: `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n  <url><loc>${origin}/</loc></url>\n</urlset>\n`,
          })
        },
      },
      tanstackStart({
        prerender: {
          enabled: true,
          autoStaticPathsDiscovery: false,
          crawlLinks: false,
          failOnError: true,
        },
        pages: [{ path: '/' }],
      }),
      react(),
    ],
    server: {
      port: 3000,
      strictPort: true,
    },
  }
})
