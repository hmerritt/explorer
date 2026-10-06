import type { Platform, ReleaseInfo } from './releases'

export function detectPlatform(
  userAgent: string,
  maxTouchPoints: number,
): Platform | null {
  if (/Android|iPhone|iPad|iPod|Windows Phone/i.test(userAgent)) return null
  if (/Windows/i.test(userAgent)) return 'windows'
  if (/Macintosh|Mac OS X/i.test(userAgent) && maxTouchPoints < 2)
    return 'macos'
  if (/Linux/i.test(userAgent)) return 'linux'
  return null
}

export function heroDownload(release: ReleaseInfo, platform: Platform | null) {
  const installer = release.assets.find(
    (asset) => asset.platform === 'windows' && asset.kind === 'installer',
  )
  const name =
    platform === 'macos' ? 'macOS' : platform === 'linux' ? 'Linux' : 'Windows'
  return {
    label: platform ? `Download for ${name}` : 'Download Explorer',
    href:
      platform === 'windows' && installer
        ? installer.url
        : platform
          ? `#download-${platform}`
          : '#get-it',
  }
}
