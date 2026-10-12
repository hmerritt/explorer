/** Resolve the optional analytics script only for production builds. */
export function plausibleScriptUrl(
  value: string | undefined,
  environment: { command: string; mode: string; nodeEnv: string | undefined },
): string | undefined {
  if (
    environment.command !== 'build' ||
    ['development', 'test'].includes(environment.mode) ||
    ['development', 'test'].includes(environment.nodeEnv ?? '')
  )
    return undefined

  const configured = value?.trim()
  if (!configured || !/^https?:\/\//i.test(configured)) return undefined
  try {
    const url = new URL(configured)
    if (
      !['http:', 'https:'].includes(url.protocol) ||
      url.username ||
      url.password
    )
      return undefined
    return url.href
  } catch {
    return undefined
  }
}
