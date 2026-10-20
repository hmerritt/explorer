import {
  createRootRoute,
  HeadContent,
  Link,
  Outlet,
  Scripts,
} from '@tanstack/react-router'
import type { ReactNode } from 'react'
import stylesheet from '../styles.css?url'
import { SITE_URL, HOME_URL } from '../lib/releases'
import { motionBootstrap } from '../lib/page-motion'

const title = 'Explorer — Windows File Explorer for macOS, Linux and Windows'
const plausibleUrl = import.meta.env.VITE_PLAUSIBLE_URL
const description =
  'A free cross-platform file explorer packed with features. Tabs, split-screen, instant previews, SFTP support, URL downloads, Git repo integrations, a built in image viewer and EPUB reader. Free and open source.'

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: 'utf-8' },
      { name: 'viewport', content: 'width=device-width, initial-scale=1' },
      { title },
      {
        name: 'description',
        content: description,
      },
      { name: 'theme-color', content: '#f2f1ee' },
      { name: 'apple-mobile-web-app-title', content: 'Explorer' },
      { property: 'og:type', content: 'website' },
      { property: 'og:site_name', content: 'Explorer' },
      { property: 'og:title', content: title },
      { property: 'og:description', content: description },
      { property: 'og:url', content: HOME_URL },
      { property: 'og:image', content: `${SITE_URL}/images/social.png` },
      { property: 'og:image:width', content: '1200' },
      { property: 'og:image:height', content: '630' },
      {
        property: 'og:image:alt',
        content: 'Explorer — Windows File Explorer on macOS, Linux and Windows',
      },
      { name: 'twitter:card', content: 'summary_large_image' },
      { name: 'twitter:title', content: title },
      { name: 'twitter:description', content: description },
      { name: 'twitter:image', content: `${SITE_URL}/images/social.png` },
    ],
    links: [
      { rel: 'stylesheet', href: stylesheet },
      {
        rel: 'icon',
        type: 'image/png',
        href: '/favicon-96x96.png',
        sizes: '96x96',
      },
      { rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' },
      { rel: 'shortcut icon', href: '/favicon.ico' },
      {
        rel: 'apple-touch-icon',
        sizes: '180x180',
        href: '/apple-touch-icon.png',
      },
      { rel: 'manifest', href: '/site.webmanifest' },
      {
        rel: 'preload',
        href: '/fonts/archivo-latin.woff2',
        as: 'font',
        type: 'font/woff2',
        crossOrigin: 'anonymous',
      },
    ],
  }),
  component: Outlet,
  shellComponent: RootDocument,
  notFoundComponent: NotFound,
})

function RootDocument({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: motionBootstrap }} />
        {plausibleUrl && (
          <script
            defer
            data-domain={new URL(SITE_URL).hostname}
            src={plausibleUrl}
          />
        )}
        <HeadContent />
      </head>
      <body>
        {children}
        <Scripts />
      </body>
    </html>
  )
}

function NotFound() {
  return (
    <main className="not-found">
      <h1>Page not found</h1>
      <p>This page does not exist.</p>
      <Link to="/">Back to Explorer</Link>
    </main>
  )
}
