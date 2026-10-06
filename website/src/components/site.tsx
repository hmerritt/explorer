import { useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { AnimatedSectionNavigator } from './section-navigator'
import { REPOSITORY, SITE_URL } from '../lib/releases'

const sections = [
  { id: 'explorer', label: 'Explorer' },
  { id: 'highlights', label: 'Highlights' },
  { id: 'gallery', label: 'Screenshots' },
  { id: 'get-it', label: 'Get it' },
  { id: 'start', label: 'Get started' },
]

export function Arrow({ down = false }: { down?: boolean }) {
  return (
    <span aria-hidden="true" className="arrow">
      {down ? '↓' : '↗'}
    </span>
  )
}

export function DownloadIcon() {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="24"
      height="24"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="download-icon"
      aria-hidden="true"
    >
      <path d="M12 17V3" />
      <path d="m6 11 6 6 6-6" />
      <path d="M19 21H5" />
    </svg>
  )
}

export function GitHubIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      className="github-icon"
    >
      <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
    </svg>
  )
}

export function Header() {
  const [open, setOpen] = useState(false)
  const toggle = useRef<HTMLButtonElement>(null)
  return (
    <header
      className="site-header"
      onKeyDown={(event) => {
        if (event.key === 'Escape' && open) {
          setOpen(false)
          toggle.current?.focus()
        }
      }}
    >
      <a className="header-logo" href="#explorer" aria-label="Explorer home">
        <img src="/icon.png" alt="" width="32" height="32" />
      </a>
      <button
        ref={toggle}
        className="menu-toggle"
        type="button"
        aria-expanded={open}
        aria-controls="main-navigation"
        onClick={() => setOpen(!open)}
      >
        {open ? 'Close ×' : 'Menu +'}
      </button>
      <nav
        id="main-navigation"
        className={`main-navigation ${open ? 'is-open' : ''}`}
        aria-label="Main navigation"
      >
        {sections.map(({ id, label }) => (
          <a key={id} href={`#${id}`} onClick={() => setOpen(false)}>
            {label}
          </a>
        ))}
      </nav>
      <a className="header-github" href={REPOSITORY}>
        GitHub <GitHubIcon />
      </a>
      <a className="header-download" href="#get-it">
        Download <Arrow down />
      </a>
    </header>
  )
}

export function SectionBar({
  number,
  label,
  note,
}: {
  number: string
  label: string
  note: string
}) {
  return (
    <div className="section-bar">
      <span>
        {number} / {label}
      </span>
      <span>{note}</span>
    </div>
  )
}

export function CopyButton({
  text,
  label = 'Copy',
  success = 'Copied',
  icon,
}: {
  text: string
  label?: string
  success?: string
  icon?: ReactNode
}) {
  const [status, setStatus] = useState<'idle' | 'copied' | 'failed'>('idle')
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current)
    },
    [],
  )
  async function copy() {
    if (timer.current) clearTimeout(timer.current)
    try {
      await navigator.clipboard.writeText(text)
      setStatus('copied')
    } catch {
      setStatus('failed')
    }
    timer.current = setTimeout(() => setStatus('idle'), 3_000)
  }
  return (
    <span className="copy-control">
      <button type="button" onClick={copy}>
        {icon}
        {label}
      </button>
      <span className="copy-feedback" role="status">
        {status === 'copied'
          ? success
          : status === 'failed'
            ? 'Could not copy. Select the text to copy manually.'
            : ''}
      </span>
    </span>
  )
}

export function Terminal({
  title,
  children,
}: {
  title: string
  children: string
}) {
  return (
    <div className="terminal">
      <div className="terminal-bar">
        <span>{title}</span>
        <CopyButton text={children} />
      </div>
      <pre>
        <code>{children}</code>
      </pre>
    </div>
  )
}

export function ShareLinks() {
  const url = encodeURIComponent(SITE_URL)
  const text = encodeURIComponent(
    'Explorer: Windows File Explorer. On macOS, Linux and Windows.',
  )
  return (
    <div className="share-links" aria-label="Share Explorer">
      <span>Share</span>
      <CopyButton
        text={SITE_URL}
        label="Copy link"
        success="Link copied"
        icon={
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="24"
            height="24"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            className="copy-icon"
            aria-hidden="true"
          >
            <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
            <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
          </svg>
        }
      />
      <a
        href={`https://x.com/intent/post?text=${text}&url=${url}`}
        aria-label="Share on X"
      >
        X
      </a>
      <a href={`https://www.reddit.com/submit?url=${url}&title=${text}`}>
        Reddit
      </a>
      <a href={`https://bsky.app/intent/compose?text=${text}%20${url}`}>
        Bluesky
      </a>
      <a href={`https://www.linkedin.com/sharing/share-offsite/?url=${url}`}>
        LinkedIn
      </a>
    </div>
  )
}

export function SectionNavigator() {
  return <AnimatedSectionNavigator sections={sections} />
}

export function Footer() {
  const groups: { title: string; links: [string, string][] }[] = [
    {
      title: 'Explorer',
      links: [
        ['Download', '#get-it'],
        ['Features', '#highlights'],
        ['Screenshots', '#gallery'],
      ],
    },
    {
      title: 'Resources',
      links: [
        ['Installation', `${REPOSITORY}#-install`],
        ['Configuration', `${REPOSITORY}#configuration`],
        ['Release notes', `${REPOSITORY}/releases`],
      ],
    },
    {
      title: 'Open source',
      links: [
        ['GitHub', REPOSITORY],
        ['Report an issue', `${REPOSITORY}/issues`],
        ['MIT license', `${REPOSITORY}/blob/master/LICENSE.txt`],
      ],
    },
  ]
  return (
    <footer className="site-footer frame">
      <div className="footer-main">
        <div className="footer-brand">
          <a href="#explorer" className="brand">
            <img src="/icon.png" width="40" height="40" alt="" />
            <span>Explorer</span>
          </a>
          <p>
            Windows File Explorer.
            <br />
            On macOS, Linux and Windows.
          </p>
        </div>
        {groups.map(({ title, links }) => (
          <nav key={title} aria-label={title}>
            <h2 className="label">{title}</h2>
            <ul>
              {links.map(([name, href]) => (
                <li key={name}>
                  <a href={href}>{name}</a>
                </li>
              ))}
            </ul>
          </nav>
        ))}
      </div>
      <div className="footer-bottom label">
        <span>Explorer · Free and open source</span>
        <span>Built in Rust. Made for your files.</span>
      </div>
    </footer>
  )
}

export function ButtonLink({
  href,
  primary = false,
  children,
  down = false,
  download = false,
  icon,
}: {
  href: string
  primary?: boolean
  children: ReactNode
  down?: boolean
  download?: boolean
  icon?: ReactNode
}) {
  return (
    <a href={href} className={`button ${primary ? 'button-primary' : ''}`}>
      {children}
      {icon ?? (download ? <DownloadIcon /> : <Arrow down={down} />)}
    </a>
  )
}
