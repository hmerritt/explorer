# Explorer website

A TanStack Start React download website for Explorer, deployed to Netlify. The
website is independent of the Rust desktop app: all JavaScript dependencies,
source, and tooling live in this directory.

## Requirements and development

Install **Bun 1.4.2** and **Node.js 24**. Bun installs dependencies and runs scripts;
Vite and Netlify's serverless functions use Node.js. The Bun version is pinned in
`package.json`, GitHub Actions, and the root `netlify.toml`. `.node-version` records
the Node.js major version for local version managers.

From the repository root:

```sh
cd website
bun install --frozen-lockfile
bun run dev
```

Open <http://localhost:3000>. Vite uses Netlify's TanStack Start adapter to emulate
the deployment platform locally. The development port is fixed to 3000; stop any
other process on that port before starting the app.

The root Netlify configuration overrides the base to `.` for the local `dev`
context because the adapter resolves paths from this directory. Hosted builds
retain the `website` base directory.

Edge Functions emulation is disabled because this starter uses Node serverless
functions for SSR and has no edge functions. Deno is not required for development.

The homepage is server rendered, including feature copy and current downloads.
After hydration the main download action recognizes desktop operating systems;
macOS and Linux visitors choose their architecture in the download section.
Unknown paths return a 404 with a link back to the homepage.

## Design, media and release data

`DESIGN.md` records the measured PhotoCraft reference and Explorer adaptations.
The page uses local Archivo, Inter and Geist Mono fonts; their OFL licenses ship
alongside the fonts in `public/fonts/`. The site has one light theme.

The homepage loader uses an internal TanStack server function to request the
latest stable GitHub release. No separate public release API is maintained.
The server-only service has a five-second timeout, deduplicates concurrent
requests and caches successful metadata for one hour per server instance.
Failed refreshes retain the last successful release and retry after a minute;
cold failures link to GitHub releases. Only published Mac ZIPs, Linux tarballs
and Windows installer/portable downloads are shown. No API secret is required.

The icon comes from `assets/explorer.png`. The four screenshots in
`public/images/` are actual Explorer captures using the isolated README demo
fixture: overview, large icons, image viewer and image properties. PNGs are
retained for full-size links; the page uses optimized WebP versions.
`scripts/prepare-images.py` regenerates those WebPs and the 1200 × 630 social
card from the captures (requires Python and Pillow). The capture dimensions are
recorded in the page to reserve layout space. Below-the-fold gallery images are
loaded lazily. Do not substitute generated mockups or personal file listings.

The default public URL for sharing and social metadata is
`https://hmerritt-explorer.netlify.app`, defined in `src/lib/releases.ts`.

## Commands

| Command                | Purpose                                                        |
| ---------------------- | -------------------------------------------------------------- |
| `bun run dev`          | Start the development server with hot reload.                  |
| `bun run build`        | Generate routes and build client assets and Netlify functions. |
| `bun run typecheck`    | Check TypeScript without emitting files.                       |
| `bun run lint`         | Run ESLint, treating warnings as failures.                     |
| `bun run format`       | Format website source and configuration with Prettier.         |
| `bun run format:check` | Check formatting without modifying files.                      |
| `bun run test`         | Test release handling, caching, failures and platform CTAs.    |
| `bun run check`        | Test, build, typecheck, lint, then check formatting.           |

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

## Netlify deployment

- Production: <https://hmerritt-explorer.netlify.app>
- Netlify project and deploy logs:
  <https://app.netlify.com/projects/hmerritt-explorer/deploys>
- Setup pull request: <https://github.com/hmerritt/explorer/pull/4>
- Setup Deploy Preview: <https://deploy-preview-4--hmerritt-explorer.netlify.app>

The first verified preview deployed commit
`a1a421163317f9f49b25671e31aa2f07c4ebad83`. Each Netlify deploy records its Git
commit; use the project deploy log to identify the currently published revision.

The repository's root `netlify.toml` is the source of truth:

- Base directory: `website`
- Build command: `bun run check`
- Publish directory: `dist/client`, relative to the base directory
- Bun: `1.4.2`, with `--frozen-lockfile`
- Node.js: `24`

The Vite adapter `@netlify/vite-plugin-tanstack-start` generates the serverless
function and SSR routing. No application secrets are required by this starter.
Netlify authentication and local CLI state must not be committed. Local `.env`
files, dependencies, and generated build output are ignored by Git.

To connect the repository:

1. Sign in to your personal Netlify team and import `hmerritt/explorer` from GitHub.
2. Grant Netlify access to this repository if it is not already authorized.
3. Set the production branch to `master` and confirm the tracked build settings
   above. Keep pull-request Deploy Previews enabled.
4. Verify the first pull-request preview, then the production build after the
   website changes are merged into `master`.

Use the generated HTTPS `netlify.app` URL. Custom domains are a later addition;
update the public URL constant when one is configured.

See the official [TanStack Start hosting guide](https://tanstack.com/start/latest/docs/framework/react/guide/hosting#netlify-official-partner)
and [Netlify's TanStack Start guide](https://docs.netlify.com/build/frameworks/framework-setup-guides/tanstack-start/).

## Deployment verification

For both the preview and production deployment:

- Confirm HTTP 200 and the Explorer headline, highlights and download links in
  raw HTML before JavaScript runs. The initial hero action is generic.
- Confirm stylesheet and JavaScript requests succeed.
- Confirm the desktop platform CTA updates after hydration without console errors.
- Check the mobile menu (including keyboard/Escape), section anchors, copy
  feedback, full-size image links and the six supported release asset links.
- Compare the layout at desktop, tablet and phone widths; confirm no horizontal
  overflow, readable screenshot captions and visible keyboard focus.
- Request an unknown path and confirm HTTP 404.
- Check build logs for the website base directory, Bun version, frozen dependency
  installation, successful checks, and the generated serverless function.
- Record the deployed URLs and commit after those checks pass.
