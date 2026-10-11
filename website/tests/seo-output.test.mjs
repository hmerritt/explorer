import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, existsSync } from 'node:fs'
import { siteOrigin } from '../src/lib/site-url.ts'

const output = new URL('../dist/client/', import.meta.url)
const read = (path) => readFileSync(new URL(path, output), 'utf8')
const origin = siteOrigin(process.env.VITE_SITE_URL, true)
const home = `${origin}/`
const html = read('index.html')
const originalDescription =
  'A free cross-platform file explorer packed with features. Tabs, split-views, instant previews, SFTP support, URL downloads, Git repo integrations, a built in image viewer and EPUB reader. Free and open source.'

// These tests run after the production build, separately from source unit tests.
test('prerendered metadata consistently identifies the public homepage', () => {
  const links = html.match(/<link\b[^>]*>/g) ?? []
  const canonicals = links.filter((tag) => /rel="canonical"/.test(tag))
  assert.equal(canonicals.length, 1)
  assert.ok(canonicals[0].includes(`href="${home}"`))
  for (const [name, content] of [
    ['og:url', home],
    ['og:image', `${origin}/images/social.png`],
    ['twitter:image', `${origin}/images/social.png`],
    ['description', originalDescription],
    ['og:description', originalDescription],
    ['twitter:description', originalDescription],
  ]) {
    const tag = (html.match(/<meta\b[^>]*>/g) ?? []).find((tag) =>
      tag.includes(`="${name}"`),
    )
    assert.ok(tag?.includes(`content="${content}"`), name)
  }
  assert.doesNotMatch(html, /https?:\/\/(?:localhost|127\.0\.0\.1)/)
  assert.ok(
    html.includes(`url=${encodeURIComponent(origin)}`),
    'sharing uses the public origin',
  )
  assert.ok(existsSync(new URL('images/social.png', output)))
})

test('crawler files expose exactly the homepage and allow crawling', () => {
  const sitemap = read('sitemap.xml')
  // Validate the complete XML grammar of our single-URL sitemap, allowing whitespace.
  const document = sitemap.match(
    /^<\?xml version="1.0" encoding="UTF-8"\?>\s*<urlset xmlns="http:\/\/www.sitemaps.org\/schemas\/sitemap\/0.9">\s*<url>\s*<loc>([^<&]+)<\/loc>\s*<\/url>\s*<\/urlset>\s*$/,
  )
  assert.ok(document, 'well-formed sitemap XML containing one URL')
  assert.equal(document[1], home)
  const robots = read('robots.txt')
  assert.match(robots, /^User-agent: \*\nAllow: \/\n/m)
  assert.ok(robots.includes(`Sitemap: ${origin}/sitemap.xml`))
  assert.doesNotMatch(robots, /Disallow:\s*\/\s*$/m)
})

test('software metadata is present before JavaScript and describes actual free downloads', () => {
  const scripts = [
    ...html.matchAll(
      /<script\b[^>]*type="application\/ld\+json"[^>]*>([\s\S]*?)<\/script>/g,
    ),
  ]
  assert.equal(scripts.length, 1)
  const app = JSON.parse(scripts[0][1])
  assert.equal(app['@type'], 'SoftwareApplication')
  assert.equal(app.name, 'Explorer')
  assert.equal(app.description, originalDescription)
  assert.equal(app.url, home)
  assert.equal(app.operatingSystem, 'macOS, Linux, Windows')
  assert.equal(app.offers.price, 0)
  assert.equal(app.offers.url, `${home}#download`)
  assert.equal(app.sameAs, 'https://github.com/hmerritt/explorer')
  assert.equal(app.downloadUrl, `${app.sameAs}/releases/latest`)
  assert.equal(app.aggregateRating, undefined)
  assert.equal(app.review, undefined)
  assert.doesNotMatch(read('404.html'), /rel="canonical"|application\/ld\+json/)
})

test('homepage keeps semantic content and serves optimized media with original links', () => {
  assert.equal((html.match(/<h1\b/g) ?? []).length, 1)
  const paragraph = html.match(/<p class="hero-description">([\s\S]*?)<\/p>/)
  assert.ok(paragraph)
  assert.equal(
    paragraph[1].replace(/\s+/g, ' ').trim(),
    'A file explorer packed with features: Tabs, split-views, instant previews, SFTP support, URL downloads, Git repo integrations, a built in image viewer and EPUB reader.',
  )
  assert.match(html, /href="\/images\/overview.png"/)
  const hero = (html.match(/<img\b[^>]*>/g) ?? []).find((tag) =>
    tag.includes('src="/images/overview.webp"'),
  )
  assert.ok(hero)
  assert.match(hero, /width="1202"/)
  assert.match(hero, /height="822"/)
  assert.match(hero, /fetchPriority="high"/i)
  assert.ok(existsSync(new URL('images/overview.webp', output)))
  assert.match(html, /id="download"/)
  assert.doesNotMatch(html, /get-it|Get it|Get Explorer/)
  assert.match(html, /https:\/\/github.com\/hmerritt\/explorer\/releases/)
})
