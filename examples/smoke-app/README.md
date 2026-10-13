# smoke-app — the framework-adapter reference project

This directory is a **working** Oxide project, not just a fixture. It's
what a user gets from `pnpm create oxide` (modulo the actual template),
and it exercises every capability the framework adapter contract
promises:

| Capability | Where it lives |
| --- | --- |
| `.ts` config, real adapter discovery | `oxide.config.ts` |
| Workspace-linked `@oxide/react` | `package.json` |
| TSX pages compiled by the worker | `src/routes/**/page.tsx` |
| Loaders threaded into components | `src/routes/blog/[slug]/page.server.ts` |
| Runtime `generateParams` | same file |
| Static and dynamic routes side by side | `src/routes/index` / `blog/[slug]` |

## What the routes do

- **`/`** — static, no server module. `oxide build` prerenders it to
  `dist/prerendered/index.html`.
- **`/blog/[slug]`** — dynamic route with a loader that returns
  `{ title, body }` from the URL slug, plus a runtime `generateParams`
  that enumerates `['hello-prod', 'second-post']`. `oxide build`
  evaluates the params through the worker, runs the loader for each,
  and prerenders both paths.

## Running it

From the workspace root:

```sh
# install workspace deps (creates node_modules/@oxide/* symlinks)
pnpm install

# CLI subcommands (in-process, no HTTP server)
cargo run -p oxide-cli --bin oxide-cli -- config  examples/smoke-app --json
cargo run -p oxide-cli --bin oxide-cli -- render  routes/index --root examples/smoke-app
cargo run -p oxide-cli --bin oxide-cli -- render 'routes/blog/[slug]' \
  --root examples/smoke-app --url /blog/hello --param slug=hello

# build (writes dist/)
cargo run -p oxide-cli --bin oxide-cli -- build --root examples/smoke-app

# dev / start servers (Ctrl-C to stop)
cargo run -p oxide-cli --bin oxide-cli -- dev   --root examples/smoke-app
cargo run -p oxide-cli --bin oxide-cli -- start --root examples/smoke-app
