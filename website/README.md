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

Click any screenshot or the looping preview video to enlarge it over a dark
backdrop. Clicking anywhere, scrolling, or pressing Escape returns to the page.
The viewer preserves scroll position and keyboard focus, respects reduced motion,
and keeps original screenshot links available for modified clicks or without
JavaScript. Enlarged videos continue muted from the preview's playback position.

`VITE_SITE_URL` defaults to `http://explorer.mrrtt.me/` in development and production.
Set it explicitly to override the default, for example `https://explorer.mrrtt.me`.
It supplies the normalized origin for sharing, social metadata,
the homepage canonical URL, software JSON-LD, `robots.txt` and `sitemap.xml`.
Production builds reject configured origins without a public domain or with
credentials, a path, query or fragment. HTTP and HTTPS are supported. Trailing
slashes are normalized; the homepage URL retains its final slash.
This value is public and is embedded in the build. For PowerShell:

```powershell
$env:VITE_SITE_URL = 'https://explorer.mrrtt.me'
bun run check
```

For Bash, use `VITE_SITE_URL=https://explorer.mrrtt.me bun run check`.
Configure the same variable in your hosting provider's build environment;
`.env.deploy` is only loaded by the VPS deployment script.

## Commands

| Command                    | Purpose                                                         |
| -------------------------- | --------------------------------------------------------------- |
| `bun run dev`              | Start the development server with hot reload.                   |
| `bun run build`            | Generate routes, prerender HTML and build static assets/JSON.   |
| `bun run deploy`           | Build and mirror the static site to the configured VPS.         |
| `bun run deploy --dry-run` | Build and preview transfers/deletions without remote writes.    |
| `bun run typecheck`        | Check TypeScript without emitting files.                        |
| `bun run lint`             | Run ESLint, treating warnings as failures.                      |
| `bun run format`           | Format website source and configuration with Prettier.          |
| `bun run format:check`     | Check formatting without modifying files.                       |
| `bun run test`             | Test release handling, caching, failures and platform CTAs.     |
| `bun run test:seo`         | Verify SEO metadata and crawler files in the production output. |
| `bun run test:deploy`      | Test Bun SSH/SFTP deployment against a local fixture server.    |
| `bun run check`            | Test, build, verify SEO output, typecheck, lint and formatting. |

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

Run deployment with Bun and Node.js 24 on Windows, macOS or Linux. SSH and SFTP
run inside Bun through `ssh2`; no local Bash, OpenSSH, rsync or WSL is required.
The dependency install allowlist enables only esbuild's hook, so ssh2's optional
native addons are not built; deployment uses its JavaScript implementation.
The VPS needs OpenSSH with SFTP and its POSIX rename extension, a POSIX shell for
destination checks, and a static web server such as Nginx.

From `website/`, configure and preview deployment:

```powershell
Copy-Item .env.deploy.example .env.deploy
# Edit the SSH settings, DEPLOY_PATH and VITE_SITE_URL in .env.deploy.
bun run deploy --dry-run
bun run deploy
```

On macOS/Linux, copy the example with `cp .env.deploy.example .env.deploy`, then
use the same Bun commands. `.env.deploy` is ignored by Git and parsed as dotenv
data; shell commands and expansions are not executed. Environment variables
override file values. Quote paths containing spaces, and use forward slashes in
Windows paths, such as `C:/Users/you/.ssh/id_ed25519`.

Set `SSH_HOST`, `SSH_USER`, and optionally `SSH_PORT` (default `22`). Configure
`SSH_PRIVATE_KEY` and optional `SSH_PASSPHRASE`, or `SSH_PASSWORD`. When both key
and password are supplied, key authentication is tried first. Relative key paths
are resolved against `website/`; `~/` expands to your home directory. SSH aliases,
agents and `~/.ssh/config` are not used. Passwords and passphrases can be supplied
through environment variables instead of storing them in the file.

Set `SSH_HOST_FINGERPRINT` to the server host key's OpenSSH SHA256 fingerprint.
Obtain it through a trusted channel: an administrator can run
`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub -E sha256` on the VPS and provide
the `SHA256:...` value. Use the fingerprint of the key presented by the server;
the script rejects missing or mismatched fingerprints before authentication.
Only the public `VITE_SITE_URL` belongs in the client build; SSH settings are
deployment configuration.

Create a dedicated, writable directory on the VPS before deploying and point
your web server at it. `DEPLOY_PATH` must be absolute and cannot contain `.` or
`..` components or resolve to `/`. Every remote file absent from the local build
is deleted: keep uploads, server configuration and other sites outside this
directory. The script checks the directory over SSH even in dry-run mode.

The script resolves configuration relative to itself, so it can also be invoked
from the repository root with `bun website/deploy.ts [--dry-run]`. It runs
`bun run build` before syncing the **contents** of `dist/client/`, including
`index.html`, static release JSON, `404.html`, fonts, images, videos and assets.
`dist/server/` is a build-time artifact and is not uploaded. Dependencies must
already be installed; deployment does not run installation or the full CI checks.

The SFTP mirror compares file sizes and whole-second modification timestamps,
preserves timestamps, makes public files readable (directories `755`, files
`644`) and does not copy local ownership. Changed files upload into a temporary
directory inside the destination. All uploads complete before live files are
deleted or replaced; `index.html` is promoted last. Deployment is not an atomic
release: a promotion failure can leave a partially updated site. Staging cleanup
is attempted on success and failure; if it fails, the script reports the path.
Allow only one deployment or other writer to modify this directory at a time.

Local symlinks and special files are rejected. Remote symlinks are never
traversed, and conflicting or obsolete links are removed as links. Remote special
files are rejected. A failed build stops before SSH. `--dry-run` still builds
locally, checks the destination and lists uploads, deletions, new directories
and permission changes without writing to the VPS. No live deployment runs in CI.

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

## Search discovery and rollout

The production build generates crawler files alongside `index.html`; the sitemap
lists only the homepage, not section anchors, release JSON or downloads. The
homepage includes software metadata describing the free app. No ratings or
reviews are invented; Google's software rich results require genuine review
information in addition to this metadata.

After deploying:

- Confirm `/`, `/robots.txt` and `/sitemap.xml` return HTTP 200, HTTP redirects
  to HTTPS, and unknown paths return HTTP 404. If `/index.html` is accessible,
  confirm its canonical points to `https://explorer.mrrtt.me/`.
- Inspect the served robots.txt, not only the build artifact: Cloudflare can
  prepend content policies. Confirm the allow rule and sitemap reference survive
  and that crawler protection does not block verified search crawlers.
- In GitHub repository Settings, set the website field to
  `https://explorer.mrrtt.me/`, matching the prominent README link.
- Add the domain to Google Search Console, verify ownership using the DNS TXT
  record supplied by Google, submit `https://explorer.mrrtt.me/sitemap.xml`, and
  use URL Inspection on the homepage to inspect the rendered page and request
  indexing. Check indexing reports for crawl errors or exclusions.
- Capture mobile and desktop Lighthouse reports before and after deployment.
  Target 100 for automated SEO checks; compare performance separately. A high
  audit score does not guarantee indexing or search rankings.
- Review indexing after deployment, then impressions and clicks over the next
  four weeks. Compare branded Explorer queries and relevant file-manager queries.

These hosting, DNS, GitHub and Search Console steps are separate from building
the site. See Google's [sitemap guidance](https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap)
and [software structured-data requirements](https://developers.google.com/search/docs/appearance/structured-data/software-app).

### Live audit baseline — 11 October 2026

The [PageSpeed report](https://pagespeed.web.dev/analysis/https-explorer-mrrtt-me/rly67dzqo4?form_factor=mobile)
was captured before deploying these SEO improvements, using Lighthouse 13.5.0.

| Metric                   | Mobile | Desktop |
| ------------------------ | ------ | ------- |
| SEO                      | 100    | 100     |
| Performance              | 97     | 100     |
| Accessibility            | 91     | 96      |
| Best practices           | 100    | 100     |
| First contentful paint   | 1.8 s  | 0.3 s   |
| Largest contentful paint | 2.3 s  | 0.5 s   |
| Total blocking time      | 0 ms   | 0 ms    |
| Cumulative layout shift  | 0.003  | 0.001   |

No real-user field data was available. These are single lab runs, not evidence
of indexing or rankings. The homepage already passes automated SEO audits;
canonical URLs, sitemap discovery and structured data improve aspects beyond
that score. The existing WebP hero is 75,378 bytes versus 199,815 bytes for the
PNG (62% smaller); any loading-time improvement needs a new deployed audit.
The audit also identified contrast and video-caption accessibility issues,
plus small touch targets on mobile, for a separate accessibility review.

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
- Run `bun run test:deploy` to check destination validation, build failures,
  authentication, host verification, dry runs and mirror failures against a local
  SSH/SFTP fixture. These checks do not contact a VPS and run on Windows and Linux
  in CI, as well as through `bun run check`.
- Check build logs for successful homepage prerendering, then record the deployed
  URL and commit after those checks pass.
