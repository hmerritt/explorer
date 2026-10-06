import {
  createRootRoute,
  HeadContent,
  Link,
  Outlet,
  Scripts,
} from '@tanstack/react-router'
import type { ReactNode } from 'react'
import stylesheet from '../styles.css?url'

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: 'utf-8' },
      { name: 'viewport', content: 'width=device-width, initial-scale=1' },
      { title: 'Explorer' },
      {
        name: 'description',
        content:
          'Explorer: a cross-platform file explorer built with Rust and GPUI.',
      },
    ],
    links: [{ rel: 'stylesheet', href: stylesheet }],
  }),
  component: Outlet,
  shellComponent: RootDocument,
  notFoundComponent: NotFound,
})

function RootDocument({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <head>
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
    <main>
      <h1>Page not found</h1>
      <p>This page does not exist.</p>
      <Link to="/">Back to Explorer</Link>
    </main>
  )
}
