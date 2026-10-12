import { afterEach, expect, test } from 'bun:test'
// assert.rejects avoids Bun's nested event-loop polling while SSH sockets close.
// https://github.com/oven-sh/bun/pull/42667
import assert from 'node:assert/strict'
import { generateKeyPairSync } from 'node:crypto'
import { mkdtemp, mkdir, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { Server, utils } from 'ssh2'
import type { Attributes, Connection, SFTPWrapper } from 'ssh2'
import {
  configuration,
  connect,
  deploy,
  hostFingerprint,
  loadConfiguration,
  localManifest,
  mirror,
  mirrorPlan,
  preflight,
  Remote,
} from '../deploy.ts'

const cleanup: (() => Promise<unknown>)[] = []
afterEach(async () => {
  for (const dispose of cleanup.splice(0).reverse()) await dispose()
})
async function temporary() {
  const directory = await mkdtemp(path.join(tmpdir(), 'explorer-deploy-'))
  cleanup.push(() => rm(directory, { recursive: true, force: true }))
  return directory
}
const keys = generateKeyPairSync('rsa', {
  modulusLength: 2048,
  privateKeyEncoding: { type: 'pkcs1', format: 'pem' },
  publicKeyEncoding: { type: 'pkcs1', format: 'pem' },
})
const parsed = utils.parseKey(keys.privateKey)
if (parsed instanceof Error || Array.isArray(parsed))
  throw new Error('Invalid fixture key')
const fingerprint = hostFingerprint(parsed.getPublicSSH())
const root = "/srv/Explorer's site"
const env = {
  SSH_HOST: '127.0.0.1',
  SSH_USER: 'deployer',
  SSH_PASSWORD: 'secret',
  SSH_HOST_FINGERPRINT: fingerprint,
  DEPLOY_PATH: root,
  VITE_SITE_URL: 'https://explorer.mrrtt.me',
}
type Node = { attrs: Attributes; content: Buffer }
const node = (
  kind: 'file' | 'directory' | 'link',
  content = '',
  mode = kind === 'directory' ? 0o755 : 0o644,
): Node => ({
  attrs: {
    mode:
      (kind === 'directory'
        ? 0o040000
        : kind === 'link'
          ? 0o120000
          : 0o100000) | mode,
    size: Buffer.byteLength(content),
    mtime: 1,
    atime: 1,
    uid: 1,
    gid: 1,
  },
  content: Buffer.from(content),
})

// A real SSH connection with an in-memory SFTP filesystem: no VPS or OS tools.
async function fixture() {
  const nodes = new Map<string, Node>([[root, node('directory')]])
  const operations: string[] = []
  const auth: string[] = []
  let failWrites = false,
    rejectKeys = false,
    rejectRename = false,
    preflightCode = 0,
    resolvedRoot = root
  const connections = new Set<Connection>()
  const server = new Server({ hostKeys: [keys.privateKey] }, (connection) => {
    connections.add(connection)
    connection.on('error', () => {})
    connection.on('close', () => connections.delete(connection))
    connection.on('authentication', (context) => {
      auth.push(context.method)
      if (
        context.username === 'deployer' &&
        ((context.method === 'publickey' && !rejectKeys) ||
          (context.method === 'password' && context.password === 'secret'))
      )
        context.accept()
      else context.reject(['publickey', 'password'])
    })
    connection.on('ready', () =>
      connection.on('session', (accept) => {
        const session = accept()
        session.on('exec', (accept, _reject, info) => {
          operations.push(`exec:${info.command}`)
          const stream = accept()
          stream.exit(preflightCode)
          stream.end()
        })
        session.on('sftp', (accept) => {
          const sftp = accept()
          // ssh2's fixture server doesn't advertise extensions itself. Append the
          // OpenSSH rename advertisement to its VERSION packet on the wire.
          const protocol = (
            sftp as unknown as {
              _protocol: { channelData: (id: number, data: Buffer) => void }
            }
          )._protocol
          const send = protocol.channelData.bind(protocol)
          protocol.channelData = (id, data) => {
            if (data.length === 9 && data[4] === 2) {
              const string = (value: string) => {
                const bytes = Buffer.from(value)
                const length = Buffer.alloc(4)
                length.writeUInt32BE(bytes.length)
                return Buffer.concat([length, bytes])
              }
              data = Buffer.concat([
                data,
                string('posix-rename@openssh.com'),
                string('1'),
              ])
              data.writeUInt32BE(data.length - 4)
            }
            send(id, data)
          }
          const handles = new Map<string, { name: string; read: boolean }>()
          let nextHandle = 0
          const open = (id: number, name: string) => {
            const handle = String(nextHandle++)
            handles.set(handle, { name, read: false })
            sftp.handle(id, Buffer.from(handle))
          }
          const stat = (id: number, name: string) => {
            const entry = nodes.get(name)
            if (entry) sftp.attrs(id, entry.attrs)
            else sftp.status(id, 2)
          }
          sftp.on('REALPATH', (id, name) =>
            sftp.name(id, [
              {
                filename: name === root ? resolvedRoot : name,
                longname: '',
                attrs: nodes.get(root)!.attrs,
              },
            ]),
          )
          sftp.on('LSTAT', stat)
          sftp.on('STAT', stat)
          sftp.on('OPENDIR', (id, name) => {
            operations.push(`list:${name}`)
            if ((nodes.get(name)?.attrs.mode ?? 0) & 0o040000) open(id, name)
            else sftp.status(id, 4)
          })
          sftp.on('READDIR', (id, handle) => {
            const directory = handles.get(handle.toString())!
            if (directory.read) return sftp.status(id, 1)
            directory.read = true
            const children = [...nodes].filter(
              ([name]) =>
                name !== directory.name &&
                path.posix.dirname(name) === directory.name,
            )
            if (!children.length) return sftp.status(id, 1)
            sftp.name(
              id,
              children.map(([name, entry]) => ({
                filename: path.posix.basename(name),
                longname: '',
                attrs: entry.attrs,
              })),
            )
          })
          sftp.on('OPEN', (id, name, _flags, attrs) => {
            operations.push(`open:${name}`)
            nodes.set(name, node('file', '', attrs.mode))
            open(id, name)
          })
          sftp.on('WRITE', (id, handle, offset, data) => {
            const name = handles.get(handle.toString())!.name
            operations.push(`write:${name}`)
            if (failWrites) return sftp.status(id, 4)
            const entry = nodes.get(name)!
            const content = Buffer.alloc(
              Math.max(entry.content.length, offset + data.length),
            )
            entry.content.copy(content)
            data.copy(content, offset)
            entry.content = content
            entry.attrs.size = content.length
            sftp.status(id, 0)
          })
          sftp.on('FSTAT', (id, handle) =>
            stat(id, handles.get(handle.toString())!.name),
          )
          sftp.on('CLOSE', (id, handle) => {
            handles.delete(handle.toString())
            sftp.status(id, 0)
          })
          sftp.on('SETSTAT', (id, name, attrs) => {
            operations.push(`setstat:${name}`)
            const entry = nodes.get(name)
            if (!entry) return sftp.status(id, 2)
            if (attrs.mode !== undefined)
              entry.attrs.mode =
                (entry.attrs.mode! & 0o170000) | (attrs.mode & 0o777)
            if (attrs.mtime !== undefined) entry.attrs.mtime = attrs.mtime
            sftp.status(id, 0)
          })
          sftp.on('MKDIR', (id, name, attrs) => {
            operations.push(`mkdir:${name}`)
            if (nodes.has(name)) return sftp.status(id, 4)
            nodes.set(name, node('directory', '', attrs.mode))
            sftp.status(id, 0)
          })
          sftp.on('REMOVE', (id, name) => {
            operations.push(`remove:${name}`)
            sftp.status(id, nodes.delete(name) ? 0 : 2)
          })
          sftp.on('RMDIR', (id, name) => {
            operations.push(`rmdir:${name}`)
            if ([...nodes.keys()].some((key) => key.startsWith(`${name}/`)))
              return sftp.status(id, 4)
            sftp.status(id, nodes.delete(name) ? 0 : 2)
          })
          sftp.on('EXTENDED', (id, extension, data) => {
            if (rejectRename || extension !== 'posix-rename@openssh.com')
              return sftp.status(id, 8)
            const length = data.readUInt32BE(0),
              from = data.subarray(4, 4 + length).toString()
            const toLength = data.readUInt32BE(4 + length),
              to = data.subarray(8 + length, 8 + length + toLength).toString()
            operations.push(`rename:${to}`)
            const entry = nodes.get(from)
            if (!entry) return sftp.status(id, 2)
            nodes.set(to, entry)
            if (from !== to) nodes.delete(from)
            sftp.status(id, 0)
          })
        })
      }),
    )
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const address = server.address()
  if (!address || typeof address === 'string')
    throw new Error('Missing server address')
  cleanup.push(async () => {
    for (const connection of connections) connection.end()
    await new Promise<void>((resolve) => server.close(() => resolve()))
  })
  const config = configuration(
    { ...env, SSH_PORT: String(address.port) },
    process.cwd(),
  )
  async function remote() {
    const client = await connect(config)
    cleanup.push(async () => {
      client.destroy()
    })
    const sftp = await new Promise<SFTPWrapper>((resolve, reject) =>
      client.sftp((error, stream) => (error ? reject(error) : resolve(stream))),
    )
    return { remote: new Remote(sftp), client }
  }
  return {
    nodes,
    operations,
    auth,
    config,
    remote,
    failUploads: () => {
      failWrites = true
    },
    rejectKeys: () => {
      rejectKeys = true
    },
    rejectRename: () => {
      rejectRename = true
    },
    rejectPreflight: () => {
      preflightCode = 1
    },
    rootLink: () => {
      resolvedRoot = '/'
    },
  }
}

test('validates destination, authentication, origin, port and pinned fingerprint', () => {
  for (const destination of [
    '',
    '/',
    '//',
    '/.',
    '/./',
    '/srv/../site',
    'relative',
    '/bad\npath',
  ])
    expect(() =>
      configuration({ ...env, DEPLOY_PATH: destination }, '.'),
    ).toThrow()
  for (const port of ['0', '65536', 'NaN', '1.5', ''])
    expect(() => configuration({ ...env, SSH_PORT: port }, '.')).toThrow()
  for (const origin of ['', 'invalid', 'https://example.com/path'])
    expect(() =>
      configuration({ ...env, VITE_SITE_URL: origin }, '.'),
    ).toThrow()
  expect(() => configuration({ ...env, SSH_PASSWORD: '' }, '.')).toThrow()
  expect(() =>
    configuration({ ...env, SSH_HOST_FINGERPRINT: 'SHA256:bad' }, '.'),
  ).toThrow()
  expect(
    configuration({ ...env, SSH_PRIVATE_KEY: 'keys/key' }, process.cwd())
      .keyPath,
  ).toBe(path.resolve('keys/key'))
  if (process.platform === 'win32')
    expect(
      configuration({ ...env, SSH_PRIVATE_KEY: 'C:/Users/me/.ssh/key' }, '.')
        .keyPath,
    ).toBe('C:\\Users\\me\\.ssh\\key')
  expect(
    configuration({ ...env, SSH_PRIVATE_KEY: '~/.ssh/key' }, '.').keyPath,
  ).toContain(path.join('.ssh', 'key'))
})

test('loads dotenv relative to the script directory and lets environment override it', async () => {
  const directory = await temporary()
  await assert.rejects(
    loadConfiguration(directory),
    /Copy \.env\.deploy\.example/,
  )
  await writeFile(
    path.join(directory, '.env.deploy'),
    Object.entries(env)
      .map(([key, value]) => `${key}="${value}"`)
      .join('\n'),
  )
  const old = process.env.SSH_USER
  process.env.SSH_USER = 'override'
  try {
    expect((await loadConfiguration(directory)).username).toBe('override')
  } finally {
    if (old === undefined) delete process.env.SSH_USER
    else process.env.SSH_USER = old
  }
})

test('invalid arguments, failed builds and empty output stop before connecting', async () => {
  const directory = await temporary()
  await writeFile(
    path.join(directory, '.env.deploy'),
    Object.entries(env)
      .map(([key, value]) => `${key}="${value}"`)
      .join('\n'),
  )
  await assert.rejects(deploy(['--invalid'], directory), /Usage:/)
  await assert.rejects(deploy(['--dry-run', 'extra'], directory), /Usage:/)
  await mkdir(path.join(directory, 'dist/client'), { recursive: true })
  await writeFile(path.join(directory, 'dist/client/index.html'), 'old output')
  await assert.rejects(
    deploy([], directory, async () => {
      throw new Error('build failed')
    }),
    /build failed/,
  )
  await writeFile(path.join(directory, 'dist/client/index.html'), '')
  await assert.rejects(
    deploy([], directory, async () => {}),
    /nonempty/,
  )
})

test('rejects local symlinks before transfer', async () => {
  const directory = await temporary()
  const outside = await temporary()
  // Junctions work on Windows without developer mode or elevation.
  await symlink(
    outside,
    path.join(directory, 'link'),
    process.platform === 'win32' ? 'junction' : 'dir',
  )
  await assert.rejects(localManifest(directory), /Unsupported local file type/)
})

test('SSH verifies the fingerprint and supports password and private-key authentication', async () => {
  const f = await fixture()
  const client = await connect(f.config)
  await preflight(client, root)
  client.destroy()
  expect(f.auth).toContain('password')
  expect(f.operations[0]).toContain("Explorer'\\''s site")
  const directory = await temporary(),
    keyPath = path.join(directory, 'key')
  await writeFile(keyPath, keys.privateKey)
  const keyed = await connect({ ...f.config, keyPath })
  keyed.destroy()
  expect(f.auth.at(-1)).toBe('publickey')
  const encrypted = generateKeyPairSync('rsa', {
    modulusLength: 2048,
    privateKeyEncoding: {
      type: 'pkcs1',
      format: 'pem',
      cipher: 'aes-256-cbc',
      passphrase: 'passphrase',
    },
    publicKeyEncoding: { type: 'spki', format: 'pem' },
  })
  await writeFile(keyPath, encrypted.privateKey)
  const encryptedClient = await connect({
    ...f.config,
    keyPath,
    passphrase: 'passphrase',
    password: undefined,
  })
  encryptedClient.destroy()
  f.rejectKeys()
  const beforeFallback = f.auth.length
  const fallback = await connect({
    ...f.config,
    keyPath,
    passphrase: 'passphrase',
  })
  fallback.destroy()
  expect(f.auth.slice(beforeFallback)).toEqual(['publickey', 'password'])
  const authenticationCount = f.auth.length
  await assert.rejects(
    connect({ ...f.config, fingerprint: `SHA256:${'A'.repeat(43)}` }),
  )
  expect(f.auth.length).toBe(authenticationCount)
  await assert.rejects(connect({ ...f.config, password: 'wrong' }))
})

test('dry runs list changes with no remote writes, and reject unsafe destinations', async () => {
  const f = await fixture(),
    directory = await temporary()
  await writeFile(path.join(directory, 'index.html'), 'new site')
  f.nodes.set(`${root}/old.txt`, node('file', 'old'))
  const { remote, client } = await f.remote()
  await preflight(client, root)
  const lines: string[] = []
  const plan = await mirror(
    remote,
    root,
    directory,
    await localManifest(directory),
    true,
    (line) => lines.push(line),
  )
  expect(plan.uploads).toEqual(['index.html'])
  expect(plan.deletions).toEqual(['old.txt'])
  expect(lines).toContain('Would uploads: index.html')
  expect(
    f.operations.every(
      (value) => value.startsWith('exec:') || value.startsWith('list:'),
    ),
  ).toBe(true)
  f.rejectPreflight()
  await assert.rejects(preflight(client, root), /Remote directory/)
  f.rootLink()
  await assert.rejects(
    mirror(remote, root, directory, await localManifest(directory), true),
    /Unsafe resolved/,
  )
})

test('mirrors nested files, both type conflicts and symlinks; skips unchanged files', async () => {
  const f = await fixture(),
    directory = await temporary()
  await mkdir(path.join(directory, 'assets'))
  await mkdir(path.join(directory, 'linked'))
  await writeFile(path.join(directory, 'index.html'), 'new site')
  await writeFile(path.join(directory, 'assets/app.js'), 'script')
  await writeFile(path.join(directory, 'linked/file'), 'inside')
  await writeFile(path.join(directory, 'keep.txt'), 'same')
  const local = await localManifest(directory)
  f.nodes.set(`${root}/assets`, node('file', 'conflict'))
  f.nodes.set(`${root}/index.html`, node('directory'))
  f.nodes.set(`${root}/index.html/child`, node('file', 'conflict'))
  f.nodes.set(`${root}/linked`, node('link', '/outside'))
  f.nodes.set(`${root}/obsolete`, node('directory'))
  f.nodes.set(`${root}/obsolete/file`, node('file', 'old'))
  const keep = node('file', 'same', 0o600)
  keep.attrs.mtime = local.get('keep.txt')!.mtime
  f.nodes.set(`${root}/keep.txt`, keep)
  const { remote } = await f.remote()
  const plan = await mirror(remote, root, directory, local, false, () => {})
  expect(plan.uploads).not.toContain('keep.txt')
  expect(f.nodes.get(`${root}/index.html`)!.content.toString()).toBe('new site')
  expect(f.nodes.get(`${root}/assets/app.js`)!.content.toString()).toBe(
    'script',
  )
  expect(f.nodes.get(`${root}/linked/file`)!.content.toString()).toBe('inside')
  expect(f.operations).not.toContain(`list:${root}/linked`)
  expect(f.nodes.has(`${root}/obsolete`)).toBe(false)
  expect(
    [...f.nodes.keys()].some((name) => name.includes('.explorer-deploy-')),
  ).toBe(false)
  expect(f.nodes.get(`${root}/keep.txt`)!.attrs.mode! & 0o777).toBe(0o644)
  expect(f.nodes.get(`${root}/assets`)!.attrs.mode! & 0o777).toBe(0o755)
  expect(f.nodes.get(`${root}/index.html`)!.attrs.mtime).toBe(
    local.get('index.html')!.mtime,
  )
  const lastWrite =
    f.operations.length -
    1 -
    [...f.operations].reverse().findIndex((value) => value.startsWith('write:'))
  const firstLiveChange = f.operations.findIndex(
    (value) =>
      value.startsWith('remove:') ||
      value.startsWith('rmdir:') ||
      value.startsWith('rename:'),
  )
  expect(firstLiveChange).toBeGreaterThan(lastWrite)
  expect(
    mirrorPlan(
      local,
      new Map(
        [...local].map(([name, entry]) => [
          name,
          { ...entry, mode: entry.kind === 'directory' ? 0o755 : 0o644 },
        ]),
      ),
    ).uploads,
  ).toEqual([])
})

test('a failed upload retains live files and cleans partial staging files', async () => {
  const f = await fixture(),
    directory = await temporary()
  await writeFile(path.join(directory, 'index.html'), 'new site')
  f.nodes.set(`${root}/index.html`, node('file', 'live site'))
  f.nodes.set(`${root}/extra`, node('file', 'keep on failure'))
  f.failUploads()
  const { remote } = await f.remote()
  await assert.rejects(
    mirror(
      remote,
      root,
      directory,
      await localManifest(directory),
      false,
      () => {},
    ),
  )
  expect(f.nodes.get(`${root}/index.html`)!.content.toString()).toBe(
    'live site',
  )
  expect(f.nodes.has(`${root}/extra`)).toBe(true)
  expect(
    [...f.nodes.keys()].some((name) => name.includes('.explorer-deploy-')),
  ).toBe(false)
})

test('unsupported rename extensions fail before deleting any live files', async () => {
  const f = await fixture(),
    directory = await temporary()
  await writeFile(path.join(directory, 'index.html'), 'new site')
  f.nodes.set(`${root}/index.html`, node('file', 'live site'))
  f.nodes.set(`${root}/extra`, node('file', 'extra'))
  f.rejectRename()
  const { remote } = await f.remote()
  await assert.rejects(
    mirror(
      remote,
      root,
      directory,
      await localManifest(directory),
      false,
      () => {},
    ),
  )
  expect(f.nodes.get(`${root}/index.html`)!.content.toString()).toBe(
    'live site',
  )
  expect(f.nodes.has(`${root}/extra`)).toBe(true)
  expect(
    [...f.nodes.keys()].some((name) => name.includes('.explorer-deploy-')),
  ).toBe(false)
})

test('CLI returns a nonzero exit status on invalid arguments', async () => {
  const directory = await temporary()
  const child = Bun.spawn(
    [
      process.execPath,
      path.resolve(import.meta.dir, '../deploy.ts'),
      '--unknown',
    ],
    { cwd: directory, stdout: 'pipe', stderr: 'pipe' },
  )
  expect(await child.exited).toBe(1)
  expect(await new Response(child.stderr).text()).toContain(
    'Usage: bun deploy.ts',
  )
})

test('deploy orchestrates a loopback dry run from a separate website directory', async () => {
  const f = await fixture(),
    directory = await temporary()
  await writeFile(
    path.join(directory, '.env.deploy'),
    Object.entries({ ...env, SSH_PORT: String(f.config.port) })
      .map(([key, value]) => `${key}="${value}"`)
      .join('\n'),
  )
  await deploy(['--dry-run'], directory, async (config) => {
    expect(config.origin).toBe(env.VITE_SITE_URL)
    await mkdir(path.join(directory, 'dist/client'), { recursive: true })
    await writeFile(
      path.join(directory, 'dist/client/index.html'),
      'built site',
    )
  })
  expect(f.nodes.size).toBe(1)
})
