export const REPOSITORY = 'https://github.com/hmerritt/explorer'
export const RELEASES_URL = `${REPOSITORY}/releases/latest`
export const SITE_URL = 'https://hmerritt-explorer.netlify.app'

export type Platform = 'macos' | 'windows' | 'linux'
export type ReleaseAsset = {
  platform: Platform
  architecture: 'amd64' | 'arm64'
  kind: 'installer' | 'portable' | 'archive'
  name: string
  url: string
}
export type ReleaseInfo = {
  version: string | null
  url: string
  assets: ReleaseAsset[]
}

export const RELEASE_FALLBACK: ReleaseInfo = {
  version: null,
  url: RELEASES_URL,
  assets: [],
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

/** Normalize only the distributable formats produced by Explorer's release CI. */
export function normalizeRelease(value: unknown): ReleaseInfo {
  if (
    !isRecord(value) ||
    typeof value.tag_name !== 'string' ||
    !value.tag_name.trim() ||
    !Array.isArray(value.assets) ||
    value.draft === true ||
    value.prerelease === true
  ) {
    throw new Error('Invalid stable Explorer release')
  }

  const version = value.tag_name
  const assets: ReleaseAsset[] = []
  const formats: [
    string,
    Platform,
    ReleaseAsset['architecture'],
    ReleaseAsset['kind'],
  ][] = [
    ['macos-arm64-apple-silicon.zip', 'macos', 'arm64', 'archive'],
    ['macos-amd64-intel.zip', 'macos', 'amd64', 'archive'],
    ['windows-amd64-installer.exe', 'windows', 'amd64', 'installer'],
    ['windows-amd64.zip', 'windows', 'amd64', 'portable'],
    ['linux-amd64.tar.gz', 'linux', 'amd64', 'archive'],
    ['linux-arm64.tar.gz', 'linux', 'arm64', 'archive'],
  ]

  for (const [suffix, platform, architecture, kind] of formats) {
    const name = `explorer-${version}-${suffix}`
    const asset = value.assets.find(
      (item: unknown) => isRecord(item) && item.name === name,
    )
    if (!isRecord(asset) || typeof asset.browser_download_url !== 'string')
      continue
    const url = asset.browser_download_url
    // Do not turn untrusted API content into arbitrary external links.
    if (!url.startsWith(`${REPOSITORY}/releases/download/`) || asset.size === 0)
      continue
    assets.push({ platform, architecture, kind, name, url })
  }

  return {
    version,
    url: `${REPOSITORY}/releases/tag/${encodeURIComponent(version)}`,
    assets,
  }
}

/** A process-local cache. Failed refreshes keep stale data and retry after a minute. */
export function createReleaseService({
  fetchRelease = fetch,
  now = Date.now,
  timeoutMs = 5_000,
}: {
  fetchRelease?: typeof fetch
  now?: () => number
  timeoutMs?: number
} = {}) {
  let cached: ReleaseInfo | undefined
  let expiresAt = 0
  let pending: Promise<ReleaseInfo> | undefined

  return function getRelease(): Promise<ReleaseInfo> {
    if (now() < expiresAt) return Promise.resolve(cached ?? RELEASE_FALLBACK)
    if (pending) return pending

    pending = (async () => {
      try {
        const response = await fetchRelease(
          'https://api.github.com/repos/hmerritt/explorer/releases/latest',
          {
            headers: {
              Accept: 'application/vnd.github+json',
              'User-Agent': 'Explorer-website',
              'X-GitHub-Api-Version': '2022-11-28',
            },
            signal: AbortSignal.timeout(timeoutMs),
          },
        )
        if (!response.ok)
          throw new Error(`GitHub release HTTP ${response.status}`)
        cached = normalizeRelease(await response.json())
        expiresAt = now() + 60 * 60 * 1_000
      } catch {
        expiresAt = now() + 60 * 1_000
      }
      return cached ?? RELEASE_FALLBACK
    })().finally(() => {
      pending = undefined
    })

    return pending
  }
}
