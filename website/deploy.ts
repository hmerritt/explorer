import { createHash, randomUUID } from 'node:crypto'
import { lstat, readdir, readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseEnv, promisify } from 'node:util'
import { Client } from 'ssh2'
import type { ConnectConfig, SFTPWrapper } from 'ssh2'
import { siteOrigin } from './src/lib/site-url.ts'

export const websiteDirectory = fileURLToPath(new URL('.', import.meta.url))
export type Config = {
  host: string
  port: number
  username: string
  keyPath?: string
  passphrase?: string
  password?: string
  fingerprint: string
  destination: string
  origin: string
}
type Entry = {
  kind: 'file' | 'directory' | 'link'
  size: number
  mtime: number
  mode: number
}
export type Manifest = Map<string, Entry>

export function configuration(
  env: Record<string, string | undefined>,
  directory: string,
): Config {
  const required = (name: string) => {
    const value = env[name]
    if (!value) throw new Error(`${name} is required.`)
    return value
  }
  const host = required('SSH_HOST')
  if (/^[\s-]|[\s/\\\0]/.test(host))
    throw new Error('SSH_HOST must be a hostname or IP address.')
  const username = required('SSH_USER')
  const port = Number(env.SSH_PORT ?? '22')
  if (!Number.isInteger(port) || port < 1 || port > 65535)
    throw new Error('SSH_PORT must be between 1 and 65535.')
  const destination = required('DEPLOY_PATH')
  if (
    !destination.startsWith('/') ||
    /^\/+$/u.test(destination) ||
    destination.split('/').some((part) => part === '.' || part === '..') ||
    /[\0\r\n]/.test(destination)
  ) {
    throw new Error(
      'DEPLOY_PATH must be an absolute non-root directory without . or .. components.',
    )
  }
  const fingerprint = required('SSH_HOST_FINGERPRINT')
  if (!/^SHA256:[A-Za-z0-9+/]{43}$/.test(fingerprint))
    throw new Error(
      'SSH_HOST_FINGERPRINT must be an OpenSSH SHA256 fingerprint.',
    )
  let keyPath = env.SSH_PRIVATE_KEY
  if (keyPath)
    keyPath =
      keyPath.startsWith('~/') || keyPath.startsWith('~\\')
        ? path.join(homedir(), keyPath.slice(2))
        : path.resolve(directory, keyPath)
  if (!keyPath && !env.SSH_PASSWORD)
    throw new Error('SSH_PRIVATE_KEY or SSH_PASSWORD is required.')
  return {
    host,
    port,
    username,
    keyPath,
    passphrase: env.SSH_PASSPHRASE,
    password: env.SSH_PASSWORD,
    fingerprint,
    destination: path.posix.normalize(destination),
    origin: siteOrigin(required('VITE_SITE_URL'), true),
  }
}

export async function loadConfiguration(directory: string): Promise<Config> {
  let contents: string
  try {
    contents = await readFile(path.join(directory, '.env.deploy'), 'utf8')
  } catch {
    throw new Error(
      'Copy .env.deploy.example to .env.deploy and configure it first.',
    )
  }
  return configuration({ ...parseEnv(contents), ...process.env }, directory)
}

export function hostFingerprint(key: Buffer): string {
  return `SHA256:${createHash('sha256').update(key).digest('base64').replace(/=+$/, '')}`
}

export async function connect(config: Config): Promise<Client> {
  const client = new Client()
  const options: ConnectConfig = {
    host: config.host,
    port: config.port,
    username: config.username,
    privateKey: config.keyPath ? await readFile(config.keyPath) : undefined,
    passphrase: config.passphrase,
    password: config.password,
    authHandler: [
      ...(config.keyPath ? ['publickey' as const] : []),
      ...(config.password ? ['password' as const] : []),
    ],
    hostVerifier: (key: Buffer) => hostFingerprint(key) === config.fingerprint,
    readyTimeout: 20_000,
    keepaliveInterval: 10_000,
    keepaliveCountMax: 3,
  }
  return new Promise((resolve, reject) => {
    client.once('ready', () => resolve(client))
    // Keep an error listener after the handshake; SFTP/exec operations surface failures.
    client.on('error', (error) => {
      client.destroy()
      reject(error)
    })
    client.once('close', () =>
      reject(new Error('SSH connection closed before it was ready.')),
    )
    client.connect(options)
  })
}

const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`
export async function preflight(
  client: Client,
  destination: string,
): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    client.exec(
      `test -d ${quote(destination)} && cd ${quote(destination)} && test -w . && test "$(pwd -P)" != /`,
      (error, channel) => {
        if (error) return reject(error)
        channel.resume()
        channel.stderr.resume()
        channel.once('error', reject)
        channel.once('close', (code: number | null) =>
          code === 0
            ? resolve()
            : reject(
                new Error(
                  'Remote directory must exist, be writable and not resolve to /.',
                ),
              ),
        )
      },
    )
  })
}

export class Remote {
  constructor(readonly sftp: SFTPWrapper) {}
  realpath = (name: string) =>
    promisify(this.sftp.realpath.bind(this.sftp))(name)
  list = (name: string) => promisify(this.sftp.readdir.bind(this.sftp))(name)
  stat = (name: string) => promisify(this.sftp.lstat.bind(this.sftp))(name)
  mkdir = (name: string) =>
    new Promise<void>((resolve, reject) =>
      this.sftp.mkdir(name, { mode: 0o755 }, (error) =>
        error ? reject(error) : resolve(),
      ),
    )
  chmod = (name: string, mode: number) =>
    promisify(this.sftp.chmod.bind(this.sftp))(name, mode)
  unlink = (name: string) => promisify(this.sftp.unlink.bind(this.sftp))(name)
  rmdir = (name: string) => promisify(this.sftp.rmdir.bind(this.sftp))(name)
  upload = (local: string, remote: string) =>
    new Promise<void>((resolve, reject) =>
      this.sftp.fastPut(local, remote, { mode: 0o644 }, (error) =>
        error ? reject(error) : resolve(),
      ),
    )
  times = (name: string, mtime: number) =>
    promisify(this.sftp.utimes.bind(this.sftp))(name, mtime, mtime)
  rename = (from: string, to: string) =>
    promisify(this.sftp.ext_openssh_rename.bind(this.sftp))(from, to)
}

export async function localManifest(root: string): Promise<Manifest> {
  const rootStats = await lstat(root)
  if (!rootStats.isDirectory() || rootStats.isSymbolicLink())
    throw new Error('Local build output must be a real directory.')
  const manifest: Manifest = new Map()
  async function walk(relative: string) {
    for (const name of await readdir(path.join(root, relative))) {
      const local = path.join(root, relative, name)
      const stats = await lstat(local)
      if (!stats.isDirectory() && !stats.isFile())
        throw new Error(`Unsupported local file type: ${local}`)
      const key = relative ? `${relative}/${name}` : name
      manifest.set(key, {
        kind: stats.isDirectory() ? 'directory' : 'file',
        size: stats.size,
        mtime: Math.floor(stats.mtimeMs / 1000),
        mode: stats.mode & 0o7777,
      })
      if (stats.isDirectory()) await walk(key)
    }
  }
  await walk('')
  return manifest
}

export async function remoteManifest(
  remote: Remote,
  root: string,
): Promise<Manifest> {
  const manifest: Manifest = new Map()
  async function walk(relative: string) {
    for (const entry of await remote.list(path.posix.join(root, relative))) {
      const name = entry.filename
      if (name === '.' || name === '..') continue
      if (!name || name.includes('/') || name.includes('\0'))
        throw new Error('Invalid filename returned by SFTP server.')
      const key = relative ? `${relative}/${name}` : name
      const stats = await remote.stat(path.posix.join(root, key))
      const kind = stats.isSymbolicLink()
        ? 'link'
        : stats.isDirectory()
          ? 'directory'
          : stats.isFile()
            ? 'file'
            : null
      if (!kind) throw new Error(`Unsupported remote file type: ${key}`)
      manifest.set(key, {
        kind,
        size: stats.size,
        mtime: stats.mtime,
        mode: stats.mode & 0o7777,
      })
      if (kind === 'directory') await walk(key)
    }
  }
  await walk('')
  return manifest
}

export function mirrorPlan(local: Manifest, remote: Manifest) {
  const uploads: string[] = [],
    directories: string[] = [],
    permissions: string[] = [],
    deletions: string[] = []
  for (const [name, entry] of local) {
    const previous = remote.get(name)
    if (entry.kind === 'directory') {
      if (previous?.kind !== 'directory') directories.push(name)
    } else if (
      previous?.kind !== 'file' ||
      previous.size !== entry.size ||
      previous.mtime !== entry.mtime
    )
      uploads.push(name)
    if (
      previous?.kind === entry.kind &&
      previous.mode !== (entry.kind === 'directory' ? 0o755 : 0o644)
    )
      permissions.push(name)
  }
  for (const [name, entry] of remote)
    if (local.get(name)?.kind !== entry.kind) deletions.push(name)
  const depth = (name: string) => name.split('/').length
  directories.sort((a, b) => depth(a) - depth(b) || a.localeCompare(b))
  deletions.sort((a, b) => depth(b) - depth(a) || a.localeCompare(b))
  uploads.sort(
    (a, b) =>
      Number(a === 'index.html') - Number(b === 'index.html') ||
      a.localeCompare(b),
  )
  return { uploads, directories, permissions, deletions }
}

export async function mirror(
  remote: Remote,
  destination: string,
  localRoot: string,
  local: Manifest,
  dryRun: boolean,
  log: (line: string) => void = console.log,
) {
  const root = await remote.realpath(destination)
  if (root === '/' || !root.startsWith('/'))
    throw new Error('Unsafe resolved remote directory.')
  const previous = await remoteManifest(remote, root)
  const plan = mirrorPlan(local, previous)
  const rootStats = await remote.stat(root)
  if (!rootStats.isDirectory())
    throw new Error('Resolved destination is not a directory.')
  const rootPermissions = (rootStats.mode & 0o7777) !== 0o755
  for (const [action, names] of Object.entries(plan))
    for (const name of names) log(`${dryRun ? 'Would ' : ''}${action}: ${name}`)
  if (rootPermissions) log(`${dryRun ? 'Would ' : ''}permissions: .`)
  if (dryRun) return plan
  const stageName = `.explorer-deploy-${randomUUID()}`
  const stage = path.posix.join(root, stageName)
  if (local.has(stageName) || previous.has(stageName))
    throw new Error('Staging directory collision.')
  let staged = false
  const stageDirectories = new Set<string>()
  const stagedFiles = new Set<string>()
  try {
    if ((await remote.realpath(destination)) !== root)
      throw new Error('Deployment destination changed during planning.')
    if (plan.uploads.length) {
      await remote.mkdir(stage)
      staged = true
      for (const name of plan.uploads) {
        const parts = name.split('/').slice(0, -1)
        for (let i = 1; i <= parts.length; i++) {
          const directory = parts.slice(0, i).join('/')
          if (!stageDirectories.has(directory)) {
            await remote.mkdir(path.posix.join(stage, directory))
            stageDirectories.add(directory)
          }
        }
        // Register before uploading so a partial transfer is cleaned up too.
        stagedFiles.add(name)
        await remote.upload(
          path.join(localRoot, ...name.split('/')),
          path.posix.join(stage, name),
        )
        await remote.chmod(path.posix.join(stage, name), 0o644)
        await remote.times(path.posix.join(stage, name), local.get(name)!.mtime)
      }
      // Confirm extension support before any deletions or live replacements.
      const probe = path.posix.join(stage, plan.uploads[0])
      await remote.rename(probe, probe)
    }
    // No live files are removed or replaced until every upload has completed.
    for (const name of plan.deletions) {
      const target = path.posix.join(root, name)
      if (previous.get(name)!.kind === 'directory') await remote.rmdir(target)
      else await remote.unlink(target)
    }
    for (const name of plan.directories) {
      await remote.mkdir(path.posix.join(root, name))
      await remote.chmod(path.posix.join(root, name), 0o755)
    }
    for (const name of plan.uploads) {
      await remote.rename(
        path.posix.join(stage, name),
        path.posix.join(root, name),
      )
      stagedFiles.delete(name)
    }
    for (const name of plan.permissions)
      await remote.chmod(
        path.posix.join(root, name),
        local.get(name)!.kind === 'directory' ? 0o755 : 0o644,
      )
    if (rootPermissions) await remote.chmod(root, 0o755)
    for (const [name, entry] of [...local].reverse())
      if (entry.kind === 'directory')
        await remote.times(path.posix.join(root, name), entry.mtime)
    return plan
  } finally {
    if (staged) {
      try {
        for (const name of stagedFiles) {
          try {
            await remote.unlink(path.posix.join(stage, name))
          } catch (error) {
            if ((error as { code?: number }).code !== 2)
              log(
                `Could not clean staging file: ${path.posix.join(stage, name)}`,
              )
          }
        }
        for (const directory of [...stageDirectories].reverse())
          await remote.rmdir(path.posix.join(stage, directory))
        await remote.rmdir(stage)
      } catch {
        log(`Could not clean staging directory: ${stage}`)
      }
    }
  }
}

export async function deploy(
  args: string[],
  directory = websiteDirectory,
  build = async (config: Config) => {
    const child = Bun.spawn([process.execPath, 'run', 'build'], {
      cwd: directory,
      env: { ...process.env, VITE_SITE_URL: config.origin },
      stdin: 'inherit',
      stdout: 'inherit',
      stderr: 'inherit',
    })
    if ((await child.exited) !== 0)
      throw new Error('Website build failed; nothing was uploaded.')
  },
) {
  if (args.length > 1 || (args.length === 1 && args[0] !== '--dry-run'))
    throw new Error('Usage: bun deploy.ts [--dry-run]')
  const config = await loadConfiguration(directory)
  console.log('Building static website...')
  await build(config)
  const localRoot = path.join(directory, 'dist', 'client')
  const local = await localManifest(localRoot)
  if (
    local.get('index.html')?.kind !== 'file' ||
    !local.get('index.html')!.size
  )
    throw new Error('Build did not produce a nonempty dist/client/index.html.')
  const client = await connect(config)
  try {
    await preflight(client, config.destination)
    const sftp = await new Promise<SFTPWrapper>((resolve, reject) =>
      client.sftp((error, stream) => (error ? reject(error) : resolve(stream))),
    )
    await mirror(
      new Remote(sftp),
      config.destination,
      localRoot,
      local,
      args[0] === '--dry-run',
    )
    console.log(
      args[0] === '--dry-run'
        ? 'Dry run complete; no remote writes.'
        : 'Deployment complete.',
    )
  } finally {
    client.end()
  }
}

if (import.meta.main) {
  try {
    await deploy(process.argv.slice(2))
  } catch (error) {
    console.error(`Deploy: ${error instanceof Error ? error.message : error}`)
    process.exitCode = 1
  }
}
