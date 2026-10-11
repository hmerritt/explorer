/** Resolve one origin for metadata, sharing and generated crawler files. */
export function siteOrigin(
  value: string | undefined,
  production = false,
): string {
  const configuredUrl = value?.trim() || 'http://explorer.mrrtt.me'

  let url: URL
  try {
    url = new URL(configuredUrl)
  } catch {
    throw new Error('VITE_SITE_URL must be an absolute HTTP(S) origin.')
  }
  if (
    !['http:', 'https:'].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.pathname.replace(/\/+$/, '') ||
    url.search ||
    url.hash
  ) {
    throw new Error(
      'VITE_SITE_URL must be an HTTP(S) origin without credentials, a path, query or fragment.',
    )
  }
  if (
    production &&
    (!/^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(
      url.hostname,
    ) ||
      /^[\d.]+$/.test(url.hostname) ||
      url.hostname.startsWith('[') ||
      /(^|\.)(localhost|local|internal|lan|test|invalid|example|home\.arpa)$/.test(
        url.hostname,
      ))
  ) {
    throw new Error(
      'VITE_SITE_URL must use a public domain for production builds.',
    )
  }
  return url.origin
}
