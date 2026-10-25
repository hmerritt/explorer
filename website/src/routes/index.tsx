import { MediaPreview } from '../components/media-viewer'
import { PageMotion } from '../components/page-motion'
import { createFileRoute } from '@tanstack/react-router'
import { createServerFn } from '@tanstack/react-start'
import { staticFunctionMiddleware } from '@tanstack/start-static-server-functions'
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
import { REPOSITORY, RELEASES_URL, SITE_URL, HOME_URL } from '../lib/releases'
import type { Platform, ReleaseAsset, ReleaseInfo } from '../lib/releases'
import { detectPlatform, heroDownload } from '../lib/platform'

const loadRelease = createServerFn({ method: 'GET' })
  .middleware([staticFunctionMiddleware])
  .handler(async () => {
    const { getLatestRelease } = await import('../lib/releases.server')
    return getLatestRelease()
  })

export const Route = createFileRoute('/')({
  loader: () => loadRelease(),
  head: () => ({
    links: [{ rel: 'canonical', href: HOME_URL }],
    scripts: [
      {
        type: 'application/ld+json',
        children: JSON.stringify({
          '@context': 'https://schema.org',
          '@type': 'SoftwareApplication',
          name: 'Explorer',
          url: HOME_URL,
          description:
            'A free cross-platform file explorer packed with features. Tabs, split-screen, instant previews, SFTP support, URL downloads, Git repo integrations, a built in image viewer and EPUB reader. Free and open source.',
          applicationCategory: 'UtilitiesApplication',
          operatingSystem: 'macOS, Linux, Windows',
          isAccessibleForFree: true,
          offers: { '@type': 'Offer', price: 0, url: `${HOME_URL}#download` },
          sameAs: REPOSITORY,
          downloadUrl: RELEASES_URL,
          screenshot: `${SITE_URL}/images/overview.webp`,
          license: `${REPOSITORY}/blob/master/LICENSE.txt`,
        }).replace(/</g, '\\u003c'),
      },
    ],
  }),
  component: Home,
})

const highlights = [
  ['Native. And fast.', 'Built from the ground up. Written in Rust with GPUI.'],
  [
    'Tabs & Split-screen support',
    'Each individual tab can be made split-screen, with support for nested splits both vertically and horizontally.',
  ],
  [
    'Instant preview media files',
    'Images, videos, PDFs, EPUBs, and text files can all be instantly previewed when you hold Alt key and hover over them.',
  ],
  [
    'Image viewer and EPUB reader',
    'Inside Explorer is a fully functional Image viewer and EPUB reader, both GPU-accelerated just like Explorer is. You can set Explorer as the default program for Images and EPUBs.',
  ],
  [
    'Download files and YT videos',
    <>
      A built-in HTTP downloader allows you to literally paste in URLs to
      download them into the current directory. <code>yt-dlp</code> is also
      supported to download youtube videos, and most other video websites.
    </>,
  ],
  [
    'Settings & Customization',
    'All features can be customized and changed. The contextmenu is fully customizable so you only see the programs you actually want to use.',
  ],
] as const

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
    <section id="explorer" className="frame" data-section-motion>
      <SectionBar
        number="01"
        label="Explorer"
        note="File explorer · Free and open source"
      />
      <div className="hero-content">
        <div className="hero-identity" data-hero-reveal>
          <img
            className="hero-icon"
            src="/icon.png"
            alt="Explorer app icon"
            width="112"
            height="112"
          />
          <div className="hero-brand">
            <span>Explorer</span>
          </div>
        </div>
        <h1 data-hero-reveal>
          <span className="strikethrough">Windows</span> File Explorer.
          <br />
          On <span className="accent">macOS</span>, Linux
          <br className="desktop-break" /> and Windows.
        </h1>
        <p className="hero-description" data-hero-reveal>
          A file explorer packed with features: Tabs, split-screen, instant
          previews, SFTP support, URL downloads, Git repo integrations, a built
          in image viewer and EPUB reader.
        </p>
        <div className="hero-action-area" data-hero-reveal>
          <div className="hero-actions">
            <ButtonLink
              href={href}
              primary
              down
              download={!href.startsWith('#')}
            >
              {label}
            </ButtonLink>
            <ButtonLink href={REPOSITORY} icon={<GitHubIcon />}>
              View on GitHub
            </ButtonLink>
          </div>
          <a className="other-downloads label" href="#download">
            Other platforms and downloads <Arrow down />
          </a>
          <p className="platform-label label">
            macOS · Linux · Windows · Free and open source
          </p>
          <ShareLinks />
        </div>
      </div>
      <figure className="hero-screenshot" data-hero-screenshot-reveal>
        <figcaption className="image-caption label">
          <span>A familiar place for your files</span>
          <span>Screenshot</span>
        </figcaption>
        <MediaPreview
          src="/images/overview.png"
          label="View the Explorer overview screenshot at full size"
        >
          <img
            src="/images/overview.webp"
            alt="Explorer’s details view with demo folders, a pinned sidebar and familiar file controls"
            width="1202"
            height="822"
            fetchPriority="high"
          />
        </MediaPreview>
      </figure>
    </section>
  )
}

function Highlights() {
  return (
    <section id="highlights" className="frame" data-section-motion>
      <SectionBar number="02" label="Highlights" note="File explorer" />
      <div className="section-heading">
        <h2 data-reveal>
          What’s <span className="accent">inside</span>.
        </h2>
      </div>
      <ol className="highlights-grid" data-reveal-group>
        {highlights.map(([title, description], index) => (
          <li key={title} data-reveal>
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
    <section id="gallery" className="frame" data-section-motion>
      <SectionBar number="03" label="Screenshots" note="Captured in Explorer" />
      <div className="gallery-grid" data-reveal-group>
        {gallery.map((image, index) => (
          <figure key={image.name} data-reveal>
            {image.video !== undefined ? (
              <MediaPreview
                src={image.video}
                label={`View ${image.caption.toLowerCase()} at full size`}
                video
                className="gallery-image"
              >
                <video
                  src={image.video}
                  aria-label={image.caption}
                  autoPlay
                  loop
                  muted
                  playsInline
                />
              </MediaPreview>
            ) : (
              <MediaPreview
                src={`/images/${image.name}.png`}
                className="gallery-image"
                label={`View ${image.caption.toLowerCase()} at full size`}
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
              </MediaPreview>
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
    <section id="download" className="frame" data-section-motion>
      <SectionBar number="04" label="Download" note="Free · Open source" />
      <div className="downloads-grid">
        {platforms.map((item, index) => {
          const assets = release.assets.filter(
            (asset) => asset.platform === item.id,
          )
          return (
            <article
              data-reveal
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
        <div data-reveal>
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
        <div data-reveal>
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
    <section id="start" className="frame start-section" data-section-motion>
      <SectionBar number="05" label="Get started" note="Built in the open" />
      <div className="start-content">
        <span className="label" data-reveal>
          Open source · Free
        </span>
        <h2 data-reveal>
          Help shape <span className="accent">Explorer</span>.
        </h2>
        <p data-reveal>
          Found a bug? Have a feature in mind? Report issues, share feedback and
          contribute on GitHub.
        </p>
        <div className="hero-actions" data-reveal>
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
      <PageMotion />
    </>
  )
}
