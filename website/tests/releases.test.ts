import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  createReleaseService,
  normalizeRelease,
  RELEASE_FALLBACK,
  REPOSITORY,
} from '../src/lib/releases.ts'

const version = '0.23.0'
const suffixes = [
  'macos-arm64-apple-silicon.zip',
  'macos-amd64-intel.zip',
  'windows-amd64-installer.exe',
  'windows-amd64.zip',
  'linux-amd64.tar.gz',
  'linux-arm64.tar.gz',
  'full.nupkg',
]
function release(suffixList = suffixes) {
  return {
    tag_name: version,
    assets: suffixList.map((suffix) => ({
      name: `explorer-${version}-${suffix}`,
      browser_download_url: `${REPOSITORY}/releases/download/${version}/explorer-${version}-${suffix}`,
      size: 100,
    })),
  }
}

test('normalizes all six supported assets, excluding updater packages', () => {
  const result = normalizeRelease(release())
  assert.equal(result.version, version)
  assert.equal(result.assets.length, 6)
  assert.deepEqual(
    result.assets
      .filter((asset) => asset.platform === 'windows')
      .map((asset) => asset.kind),
    ['installer', 'portable'],
  )
  assert.ok(result.assets.every((asset) => !asset.name.endsWith('nupkg')))
})

test('missing architectures and unsupported formats never produce fabricated URLs', () => {
  const result = normalizeRelease(
    release(['linux-arm64.tar.gz', 'linux-amd64.AppImage']),
  )
  assert.equal(result.assets.length, 1)
  assert.equal(result.assets[0].architecture, 'arm64')
})

test('malformed, draft and prerelease payloads are rejected; malformed assets are skipped', () => {
  for (const payload of [
    null,
    {},
    { tag_name: '', assets: [] },
    { ...release(), draft: true },
    { ...release(), prerelease: true },
  ]) {
    assert.throws(() => normalizeRelease(payload))
  }
  assert.deepEqual(
    normalizeRelease({
      tag_name: version,
      assets: [
        null,
        {},
        {
          name: `explorer-${version}-windows-amd64.zip`,
          browser_download_url: 'javascript:alert(1)',
        },
      ],
    }).assets,
    [],
  )
})

test('concurrent requests share a fetch and successful metadata is cached for one hour', async () => {
  let calls = 0
  let time = 0
  const service = createReleaseService({
    now: () => time,
    fetchRelease: async () => {
      calls++
      return Response.json(release())
    },
  })
  const [first, second] = await Promise.all([service(), service()])
  assert.deepEqual(first, second)
  time = 3_599_999
  await service()
  assert.equal(calls, 1)
  time = 3_600_000
  await service()
  assert.equal(calls, 2)
})

test('rate limits preserve stale data and retry after one minute', async () => {
  let time = 0
  let calls = 0
  const service = createReleaseService({
    now: () => time,
    fetchRelease: async () => {
      calls++
      return calls === 1
        ? Response.json(release())
        : new Response(null, { status: 403 })
    },
  })
  const first = await service()
  time = 3_600_000
  assert.deepEqual(await service(), first)
  time += 59_999
  await service()
  assert.equal(calls, 2)
  time++
  await service()
  assert.equal(calls, 3)
})

test('cold failures and malformed responses return the stable releases fallback', async () => {
  for (const response of [
    new Response(null, { status: 429 }),
    Response.json({}),
    new Response('{'),
  ]) {
    const service = createReleaseService({ fetchRelease: async () => response })
    assert.deepEqual(await service(), RELEASE_FALLBACK)
  }
})

test('a stalled request is aborted and returns fallback; a later request can recover', async () => {
  let time = 0
  let calls = 0
  const service = createReleaseService({
    now: () => time,
    timeoutMs: 10,
    fetchRelease: async (_url, init) => {
      calls++
      if (calls > 1) return Response.json(release())
      return new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener(
          'abort',
          () => reject(new Error('Timed out')),
          { once: true },
        )
      })
    },
  })
  // AbortSignal.timeout is unref'ed; keep the test process alive until it fires.
  const keepAlive = setTimeout(() => {}, 100)
  try {
    assert.deepEqual(await service(), RELEASE_FALLBACK)
    time = 60_000
    assert.equal((await service()).version, version)
  } finally {
    clearTimeout(keepAlive)
  }
})
