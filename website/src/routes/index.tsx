import { createFileRoute } from '@tanstack/react-router'
import { createServerFn } from '@tanstack/react-start'
import { useSyncExternalStore } from 'react'
import {
  Header,
  SectionBar,
  SectionNavigator,
  Footer,
  ShareLinks,
  Terminal,
  ButtonLink,
  Arrow,
  DownloadIcon,
  GitHubIcon,
} from '../components/site'
import { REPOSITORY, RELEASES_URL } from '../lib/releases'
import type { Platform, ReleaseAsset, ReleaseInfo } from '../lib/releases'
import { detectPlatform, heroDownload } from '../lib/platform'

const loadRelease = createServerFn({ method: 'GET' }).handler(async () => {
  const { getLatestRelease } = await import('../lib/releases.server')
  return getLatestRelease()
})

export const Route = createFileRoute('/')({
  loader: () => loadRelease(),
  component: Home,
})

const highlights = [
  [
    'Familiar by design',
    'The menus, shortcuts and file interactions you already know. The Windows Explorer experience, wherever you work.',
  ],
  [
    'Native. And fast.',
    'Written in Rust with GPUI. A GPU-accelerated desktop interface, without Electron or a web view.',
  ],
  [
    'A place for every tab',
    'Keep folders close with tabs and pinned sidebar locations. Move between projects without losing your place.',
  ],
  [
    'Look before you open',
    'Hold Alt and hover to preview images, videos, PDFs and text. Open images in the built-in viewer.',
  ],
  [
    'Find your files',
    'Type to search the current folder or search through subfolders. No pre-indexing required.',
  ],
  [
    'Get things done',
    'Copy, move, rename and inspect your files. Create ZIPs, extract archives and work with removable storage.',
  ],
]

const gallery = [
  {
    name: 'large-icons',
    caption: 'Your media, at a glance',
    alt: 'Explorer displaying demo images as large thumbnails in the Media folder',
    width: 1202,
    height: 822,
  },
  {
    name: 'image-viewer',
    caption: 'A built-in image viewer',
    alt: 'Explorer’s native image viewer displaying the Harbor demo image',
    width: 1026,
    height: 822,
  },
  {
    name: 'image-previews',
    caption: 'Preview images, videos, PDFs and text files',
    video: '/videos/image-previews.mp4',
  },
]

const subscribePlatform = () => () => undefined
const browserPlatform = () =>
  detectPlatform(navigator.userAgent, navigator.maxTouchPoints)
const serverPlatform = () => null

function Hero({
  release,
  platform,
}: {
  release: ReleaseInfo
  platform: Platform | null
}) {
  const { label, href } = heroDownload(release, platform)
  return (
    <section id="explorer" className="frame">
      <SectionBar
        number="01"
        label="Explorer"
        note="File explorer · Free and open source"
      />
      <div className="hero-content">
        <img
          className="hero-icon"
          src="/icon.png"
          alt="Explorer app icon"
          width="112"
          height="112"
        />
        <div className="hero-brand">
          <span>Explorer</span>
          {/*<span className="number-tag"> [VERSION-NUMBER] </span>*/}
        </div>
        <h1>
          <span className="strikethrough">Windows</span> File Explorer.
          <br />
          On <span className="accent">macOS</span>, Linux
          <br className="desktop-break" /> and Windows.
        </h1>
        <p className="hero-description">
          A file explorer packed with features: Tabs, split-views, instant
          previews, SFTP support, URL downloads, Git repo integrations, a built
          in image viewer and EPUB reader.
        </p>
        <div className="hero-actions">
          <ButtonLink href={href} primary down download={!href.startsWith('#')}>
            {label}
          </ButtonLink>
          <ButtonLink href={REPOSITORY} icon={<GitHubIcon />}>
            View on GitHub
          </ButtonLink>
        </div>
        <a className="other-downloads label" href="#get-it">
          Other platforms and downloads <Arrow down />
        </a>
        <p className="platform-label label">
          macOS · Linux · Windows · Free and open source
        </p>
        <ShareLinks />
      </div>
      <figure className="hero-screenshot">
        <figcaption className="image-caption label">
          <span>A familiar place for your files</span>
          <span>Screenshot</span>
        </figcaption>
        <a
          href="/images/overview.png"
          aria-label="View the Explorer overview screenshot at full size"
        >
          <img
            src="/images/overview.png"
            alt="Explorer’s details view with demo folders, a pinned sidebar and familiar file controls"
            width="1202"
            height="822"
            fetchPriority="high"
          />
        </a>
      </figure>
    </section>
  )
}

function Highlights() {
  return (
    <section id="highlights" className="frame">
      <SectionBar number="02" label="Highlights" note="File explorer" />
      <div className="section-heading">
        <h2>
          What’s <span className="accent">inside</span>.
        </h2>
      </div>
      <ol className="highlights-grid">
        {highlights.map(([title, description], index) => (
          <li key={title}>
            <span className="label feature-number">0{index + 1}</span>
            <h3>{title}</h3>
            <p>{description}</p>
          </li>
        ))}
      </ol>
    </section>
  )
}

function Gallery() {
  return (
    <section id="gallery" className="frame">
      <SectionBar number="03" label="Screenshots" note="Captured in Explorer" />
      <div className="gallery-grid">
        {gallery.map((image, index) => (
          <figure key={image.name}>
            {'video' in image ? (
              <div className="gallery-image">
                <video
                  src={image.video}
                  aria-label={image.caption}
                  autoPlay
                  loop
                  muted
                  playsInline
                />
              </div>
            ) : (
              <a
                href={`/images/${image.name}.png`}
                className="gallery-image"
                aria-label={`View ${image.caption.toLowerCase()} at full size`}
              >
                <img
                  src={`/images/${image.name}.webp`}
                  alt={image.alt}
                  width={image.width}
                  height={image.height}
                  loading="lazy"
                  decoding="async"
                />
                <span className="full-size label">
                  Full size <Arrow />
                </span>
              </a>
            )}
            <figcaption className="image-caption">
              <span>{image.caption}</span>
              <span className="label">0{index + 2}</span>
            </figcaption>
          </figure>
        ))}
      </div>
    </section>
  )
}

const platforms: {
  id: Platform
  name: string
  description: string
  command: string
  packageManager: string
}[] = [
  {
    id: 'macos',
    name: 'macOS',
    description:
      'Apple silicon and Intel Macs. Download the ZIP and move Explorer to Applications.',
    command: 'brew install --cask hmerritt/tap/explorer',
    packageManager: 'Homebrew',
  },
  {
    id: 'windows',
    name: 'Windows',
    description:
      '64-bit Windows. Use the installer, or unzip the portable app and run it.',
    command:
      'scoop bucket add hmerritt https://github.com/hmerritt/scoop-bucket\nscoop install hmerritt/explorer',
    packageManager: 'Scoop',
  },
  {
    id: 'linux',
    name: 'Linux',
    description:
      'Wayland and X11. Install with the script, or unpack the archive for your architecture.',
    command:
      'curl -fsSL https://raw.githubusercontent.com/hmerritt/explorer/master/install.sh | sh',
    packageManager: 'Terminal',
  },
]

function assetLabel(asset: ReleaseAsset) {
  if (asset.platform === 'macos')
    return asset.architecture === 'arm64'
      ? 'Apple silicon · ZIP'
      : 'Intel · ZIP'
  if (asset.platform === 'windows')
    return asset.kind === 'installer'
      ? 'Installer · 64-bit'
      : 'Portable ZIP · 64-bit'
  return asset.architecture === 'arm64' ? 'ARM64 · tar.gz' : 'x86_64 · tar.gz'
}

function Downloads({
  release,
  platform,
}: {
  release: ReleaseInfo
  platform: Platform | null
}) {
  return (
    <section id="get-it" className="frame">
      <SectionBar number="04" label="Get Explorer" note="Free · Open source" />
      <div className="downloads-grid">
        {platforms.map((item, index) => {
          const assets = release.assets.filter(
            (asset) => asset.platform === item.id,
          )
          return (
            <article
              id={`download-${item.id}`}
              key={item.id}
              className={`download-card ${platform === item.id ? 'your-platform' : ''}`}
            >
              <div className="download-card-label label">
                <span>{platform === item.id ? 'Your system' : 'Platform'}</span>
                <span>0{index + 1}</span>
              </div>
              <h3>{item.name}</h3>
              <p>{item.description}</p>
              <div className="asset-links">
                {assets.length ? (
                  assets.map((asset, assetIndex) => (
                    <a
                      className={`button ${assetIndex === 0 ? 'button-primary' : ''}`}
                      key={asset.name}
                      href={asset.url}
                    >
                      {assetLabel(asset)}
                      <DownloadIcon />
                    </a>
                  ))
                ) : (
                  <ButtonLink href={RELEASES_URL}>
                    View {item.name} releases
                  </ButtonLink>
                )}
              </div>
              <div className="package-install">
                <p className="label">Install with {item.packageManager}</p>
                <Terminal title={item.packageManager}>{item.command}</Terminal>
              </div>
              {item.id === 'macos' && (
                <p className="install-note">
                  First launch may need approval in System Settings → Privacy
                  &amp; Security.
                </p>
              )}
            </article>
          )
        })}
      </div>
      <div className="release-source-grid">
        <div>
          <span className="label">Release</span>
          <h3>
            {release.version ? (
              <>
                Version <span className="accent">{release.version}</span>.
              </>
            ) : (
              'Get the latest release.'
            )}
          </h3>
          <p>
            Free for every platform. Follow the project on GitHub for updates,
            fixes and new builds.
          </p>
          <div className="text-links">
            <a href={release.url}>
              Release notes <Arrow />
            </a>
            <a href={`${REPOSITORY}/releases`}>
              All releases <Arrow />
            </a>
          </div>
          <p className="label release-platforms">macOS · Windows · Linux</p>
        </div>
        <div>
          <span className="label">Build from source</span>
          <h3>
            Run it <span className="accent">today</span>.
          </h3>
          <p>
            Install <a href="https://rustup.rs/">Rust</a> and your platform’s
            build dependencies, then run:
          </p>
          <Terminal title="Terminal">
            {
              'git clone https://github.com/hmerritt/explorer.git\ncd explorer\ncargo run --release --locked'
            }
          </Terminal>
          <a
            className="source-link"
            href={`${REPOSITORY}/blob/master/README-development.md`}
          >
            Build prerequisites and development guide <Arrow />
          </a>
        </div>
      </div>
    </section>
  )
}

function GetStarted() {
  return (
    <section id="start" className="frame start-section">
      <SectionBar number="05" label="Get started" note="Built in the open" />
      <div className="start-content">
        <span className="label">Open source · Free</span>
        <h2>
          Help shape <span className="accent">Explorer</span>.
        </h2>
        <p>
          Found a bug? Have a feature in mind? Report issues, share feedback and
          contribute on GitHub.
        </p>
        <div className="hero-actions">
          <ButtonLink href={`${REPOSITORY}/issues`} primary>
            Report an issue
          </ButtonLink>
          <ButtonLink href={REPOSITORY}>View source</ButtonLink>
        </div>
      </div>
    </section>
  )
}

function Home() {
  const release = Route.useLoaderData()
  const platform = useSyncExternalStore(
    subscribePlatform,
    browserPlatform,
    serverPlatform,
  )
  return (
    <>
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      <Header />
      <main id="main">
        <Hero release={release} platform={platform} />
        <Highlights />
        <Gallery />
        <Downloads release={release} platform={platform} />
        <GetStarted />
      </main>
      <Footer />
      <SectionNavigator />
    </>
  )
}
