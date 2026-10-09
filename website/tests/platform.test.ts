import { test } from 'node:test'
import assert from 'node:assert/strict'
import { detectPlatform, heroDownload } from '../src/lib/platform.ts'
import { RELEASE_FALLBACK } from '../src/lib/releases.ts'

test('detects desktop systems and keeps mobile or unknown visitors on the generic download path', () => {
  assert.equal(
    detectPlatform('Mozilla/5.0 (Windows NT 10.0; Win64; x64)', 0),
    'windows',
  )
  assert.equal(
    detectPlatform('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', 0),
    'macos',
  )
  assert.equal(detectPlatform('Mozilla/5.0 (X11; Linux x86_64)', 0), 'linux')
  assert.equal(detectPlatform('Mozilla/5.0 (Linux; Android 15)', 5), null)
  assert.equal(
    detectPlatform('Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)', 5),
    null,
  )
  assert.equal(
    detectPlatform('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15)', 5),
    null,
  )
  assert.equal(detectPlatform('Unknown', 0), null)
})

test('SSR and unknown systems use the generic CTA; Mac and Linux expose architecture choices', () => {
  assert.deepEqual(heroDownload(RELEASE_FALLBACK, null), {
    label: 'Download Explorer',
    href: '#get-it',
  })
  assert.equal(heroDownload(RELEASE_FALLBACK, 'macos').href, '#download-macos')
  assert.equal(heroDownload(RELEASE_FALLBACK, 'linux').href, '#download-linux')
  assert.equal(
    heroDownload(RELEASE_FALLBACK, 'windows').href,
    '#download-windows',
  )
})

test('Windows selects its installer rather than a portable ZIP; a missing installer keeps the platform chooser', () => {
  const release = {
    ...RELEASE_FALLBACK,
    assets: [
      {
        platform: 'windows' as const,
        architecture: 'amd64' as const,
        kind: 'portable' as const,
        name: 'portable.zip',
        url: 'https://github.com/hmerritt/explorer/releases/latest',
      },
      {
        platform: 'windows' as const,
        architecture: 'amd64' as const,
        kind: 'installer' as const,
        name: 'installer.exe',
        url: 'https://github.com/hmerritt/explorer/releases/download/0.24.0/installer.exe',
      },
    ],
  }
  assert.equal(heroDownload(release, 'windows').href, release.assets[1].url)
  assert.equal(
    heroDownload({ ...release, assets: release.assets.slice(0, 1) }, 'windows')
      .href,
    '#download-windows',
  )
})
