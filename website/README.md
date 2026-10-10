# Explorer website

A TanStack Start React download website for Explorer, built as a static site. The
website is independent of the Rust desktop app: all JavaScript dependencies,
source, and tooling live in this directory.

## Requirements and development

Install **Bun 1.4.2** and **Node.js 24**. Bun installs dependencies and runs scripts;
Vite uses Node.js. The Bun version is pinned in
`package.json` and GitHub Actions. `.node-version` records
the Node.js major version for local version managers.

From the repository root:

```sh
cd website
bun install --frozen-lockfile
bun run dev
```

Open <http://localhost:3000>. The development port is fixed to 3000; stop any
other process on that port before starting the app.

The homepage is prerendered at build time, including feature copy and downloads.
After hydration the main download action recognizes desktop operating systems;
macOS and Linux visitors choose their architecture in the download section.
Unknown paths return a 404 with a link back to the homepage.

## Design, media and release data

`DESIGN.md` records the measured PhotoCraft reference and Explorer adaptations.
The page uses local Archivo, Inter and Geist Mono fonts; their OFL licenses ship
alongside the fonts in `public/fonts/`. The site has one light theme.

The homepage loader uses a TanStack static server function to request the latest
stable GitHub release during prerendering. Its result is embedded in the initial
HTML and emitted as static JSON for subsequent browser navigation. The deployed
site needs no Node process or server-function endpoint. Release links refresh
when you rebuild and deploy; new releases do not update an existing build.
The release service retains its five-second timeout and GitHub releases fallback
on cold failures. Only published Mac ZIPs, Linux tarballs and Windows
installer/portable downloads are shown. No API secret is required.

The icon comes from `assets/explorer.png`. The four screenshots in
`public/images/` are actual Explorer captures using the isolated README demo
fixture: overview, large icons, image viewer and image properties. PNGs are
retained for full-size links; the page uses optimized WebP versions.
`scripts/prepare-images.py` regenerates those WebPs and the 1200 × 630 social
card from the captures (requires Python and Pillow). The capture dimensions are
recorded in the page to reserve layout space. Below-the-fold gallery images are
loaded lazily. Do not substitute generated mockups or personal file listings.

Set `VITE_SITE_URL` to the public origin for sharing and social metadata, for
example `https://explorer.example.com`. Development defaults to
`http://localhost:3000`. This value is public and is embedded in the build.

## Commands

| Command                    | Purpose                                                       |
| -------------------------- | ------------------------------------------------------------- |
| `bun run dev`              | Start the development server with hot reload.                 |
| `bun run build`            | Generate routes, prerender HTML and build static assets/JSON. |
| `bun run deploy`           | Build and mirror the static site to the configured VPS.       |
| `bun run deploy --dry-run` | Build and preview transfers/deletions without remote writes.  |
| `bun run typecheck`        | Check TypeScript without emitting files.                      |
| `bun run lint`             | Run ESLint, treating warnings as failures.                    |
| `bun run format`           | Format website source and configuration with Prettier.        |
| `bun run format:check`     | Check formatting without modifying files.                     |
| `bun run test`             | Test release handling, caching, failures and platform CTAs.   |
| `bun run check`            | Test, build, typecheck, lint, then check formatting.          |

Commit `bun.lock` when changing dependencies. The generated
`src/routeTree.gen.ts` is also committed, so standalone type checking works after
installation. Do not edit it by hand: development and builds regenerate it from
`src/routes/`. Generated routes and build output are excluded from formatting and
linting.

Type checking uses the Go-based TypeScript 7.0.2 compiler, installed as
`@typescript/native`. The `typecheck` script invokes that package explicitly so
it cannot select the older compiler. `typescript@6.0.3` is retained only to
provide the JavaScript API required by ESLint's TypeScript parser and rules.

The Website GitHub Actions workflow runs a frozen dependency installation and
`bun run check` for relevant pull requests and pushes to `master`. Rust validation
continues through the existing workflows.

## VPS deployment

Run deployment from a Bash environment with Bun, Node.js 24, SSH and rsync 3.x
installed. On Windows, use WSL and install these tools inside WSL. On macOS, use
Homebrew's rsync rather than the older bundled version. The VPS needs
SSH, rsync 3.x and a static web server such as Nginx. Configure SSH keys, usernames
and nonstandard ports in `~/.ssh/config`; `SSH_TARGET` accepts a host alias or
`user@host`. Normal SSH host verification remains enabled.

From `website/`, configure and preview deployment:

```sh
cp .env.deploy.example .env.deploy
# Edit SSH_TARGET, DEPLOY_PATH and VITE_SITE_URL in .env.deploy.
bun run deploy --dry-run
bun run deploy
```

`.env.deploy` is ignored by Git and sourced as trusted Bash assignments. Quote
values, especially paths containing spaces. Only the public `VITE_SITE_URL`
belongs in the client build; SSH settings are deployment configuration.

Create a dedicated, writable directory on the VPS before deploying and point
your web server at it. `DEPLOY_PATH` must be absolute and cannot contain `.` or
`..` components or resolve to `/`. Every remote file absent from the local build
is deleted: keep uploads, server configuration and other sites outside this
directory. The script checks the directory over SSH even in dry-run mode.

The script resolves configuration relative to itself, so it can also be invoked
from the repository root with `bash website/deploy.sh [--dry-run]`. It runs
`bun run build` before syncing the **contents** of `dist/client/`, including
`index.html`, static release JSON, `404.html`, fonts, images, videos and assets.
`dist/server/` is a build-time artifact and is not uploaded. Dependencies must
already be installed; deployment does not run installation or the full CI checks.

Rsync preserves timestamps, makes public files readable (directories `755`, files
`644`) and does not copy local ownership. It delays updates and deletion until
the transfer completes, but the deployment is not an atomic release. A failed
build stops before SSH or rsync. `--dry-run` still builds locally and shows
proposed transfers and deletions without writing to the VPS.

### Static server and caching

For example, adapt this Nginx server block to your domain, deployment path and
TLS setup. Serve unknown paths with HTTP 404 rather than an SPA homepage fallback.

```nginx
server {
    listen 80;
    server_name explorer.example.com;
    root /var/www/explorer;
    index index.html;

    location / {
        try_files $uri $uri/ =404;
        add_header Cache-Control "public, max-age=0, must-revalidate";
    }

    location /assets/ {
        try_files $uri =404;
        add_header Cache-Control "public, max-age=31536000, immutable";
    }

    error_page 404 /404.html;
    location = /404.html {
        internal;
        add_header Cache-Control "no-cache";
    }
}
```

Enable Cloudflare proxying for the domain and configure origin HTTPS with
Full (strict) TLS. Hashed `/assets/` files can be cached for a year. HTML and
release JSON need short caching or revalidation; unversioned media also needs
revalidation when changed. To cache HTML/JSON at the edge, create a Cache Rule
making them eligible with an explicit edge TTL. Cloudflare Free has a
[minimum explicit edge TTL of two hours](https://developers.cloudflare.com/cache/how-to/edge-browser-cache-ttl/#edge-cache-ttl).
Use that minimum for HTML/JSON and keep browser caching set to respect origin
headers. Purge changed URLs after deploying when you need immediate updates,
including `/`, `/index.html` and the generated release JSON URLs under
`/__tsr/staticServerFnCache/`. DNS, TLS, web-server configuration,
Cloudflare rules and cache purges are manual; the script only builds and syncs.

See TanStack's [static prerendering guide](https://tanstack.com/start/latest/docs/framework/react/guide/static-prerendering)
and [static server functions guide](https://tanstack.com/start/latest/docs/framework/react/guide/static-server-functions).

## Deployment verification

For a local static preview and the production deployment:

- Confirm HTTP 200 and the Explorer headline, highlights and download links in
  raw HTML before JavaScript runs. The initial hero action is generic.
- Confirm stylesheet and JavaScript requests succeed.
- Confirm the desktop platform CTA updates after hydration without console errors.
- Check the mobile menu (including keyboard/Escape), section anchors, copy
  feedback, full-size image links and the six supported release asset links.
- Compare the layout at desktop, tablet and phone widths; confirm no horizontal
  overflow, readable screenshot captions and visible keyboard focus.
- Request an unknown path and confirm HTTP 404.
- Serve only `dist/client/` when testing locally; Vite's SSR preview can hide
  missing static output. Confirm the release JSON and media requests succeed and
  that navigation makes no `/_serverFn` requests.
- Run `bash tests/deploy.test.sh` to check destination validation, build failures,
  quoting, dry-run arguments and transfer failures using temporary command stubs.
  These checks do not contact a VPS and also run in CI.
- Check build logs for successful homepage prerendering, then record the deployed
  URL and commit after those checks pass.
