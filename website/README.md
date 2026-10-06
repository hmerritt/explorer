# Explorer website

A minimal TanStack Start React website for Explorer, deployed to Netlify. The
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

The homepage is server rendered. Its counter starts at zero and increments in the
browser, providing a small check that hydration works. Unknown paths return a 404
with a link back to the homepage.

## Commands

| Command                | Purpose                                                        |
| ---------------------- | -------------------------------------------------------------- |
| `bun run dev`          | Start the development server with hot reload.                  |
| `bun run build`        | Generate routes and build client assets and Netlify functions. |
| `bun run typecheck`    | Check TypeScript without emitting files.                       |
| `bun run lint`         | Run ESLint, treating warnings as failures.                     |
| `bun run format`       | Format website source and configuration with Prettier.         |
| `bun run format:check` | Check formatting without modifying files.                      |
| `bun run check`        | Build, typecheck, lint, then check formatting.                 |

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

Use the generated HTTPS `netlify.app` URL for this initial milestone. Custom
domains and the marketing page are later additions.

See the official [TanStack Start hosting guide](https://tanstack.com/start/latest/docs/framework/react/guide/hosting#netlify-official-partner)
and [Netlify's TanStack Start guide](https://docs.netlify.com/build/frameworks/framework-setup-guides/tanstack-start/).

## Deployment verification

For both the preview and production deployment:

- Confirm HTTP 200 and `Hello, Explorer!` in the raw HTML, before JavaScript runs.
- Confirm stylesheet and JavaScript requests succeed.
- Click the counter and check the browser console for hydration errors.
- Request an unknown path and confirm HTTP 404.
- Check build logs for the website base directory, Bun version, frozen dependency
  installation, successful checks, and the generated serverless function.
- Record the deployed URLs and commit after those checks pass.
