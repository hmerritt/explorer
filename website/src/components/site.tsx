import { useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
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
        GitHub <Arrow />
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
}: {
  text: string
  label?: string
  success?: string
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
      <CopyButton text={SITE_URL} label="Copy link" success="Link copied" />
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
  const [active, setActive] = useState('explorer')
  const ruler = useRef<HTMLDivElement>(null)
  useEffect(() => {
    let frame = 0
    function update() {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => {
        let current = 'explorer'
        for (const { id } of sections) {
          const element = document.getElementById(id)
          if (
            element &&
            element.getBoundingClientRect().top <= window.innerHeight * 0.4
          )
            current = id
        }
        setActive(current)
        const height =
          document.documentElement.scrollHeight - window.innerHeight
        ruler.current?.style.setProperty(
          '--progress',
          `${height > 0 ? (window.scrollY / height) * 100 : 0}%`,
        )
      })
    }
    update()
    window.addEventListener('scroll', update, { passive: true })
    window.addEventListener('resize', update)
    return () => {
      cancelAnimationFrame(frame)
      window.removeEventListener('scroll', update)
      window.removeEventListener('resize', update)
    }
  }, [])
  return (
    <>
      <nav className="section-navigation" aria-label="Page sections">
        {sections.map(({ id, label }) => (
          <a
            key={id}
            href={`#${id}`}
            aria-current={id === active ? 'location' : undefined}
          >
            {label}
          </a>
        ))}
      </nav>
      <div className="scroll-ruler" ref={ruler} aria-hidden="true">
        <span className="ruler-start">0</span>
        <span className="ruler-middle">50</span>
        <span className="ruler-end">100</span>
        <i />
      </div>
    </>
  )
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
}: {
  href: string
  primary?: boolean
  children: ReactNode
  down?: boolean
  download?: boolean
}) {
  return (
    <a href={href} className={`button ${primary ? 'button-primary' : ''}`}>
      {children}
      {download ? <DownloadIcon /> : <Arrow down={down} />}
    </a>
  )
}
