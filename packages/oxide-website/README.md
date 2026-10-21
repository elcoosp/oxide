# oxide.dev — the Oxide website, built with Oxide

The marketing site, docs and blog for [Oxide](https://github.com/elcoosp/oxide) —
rendered by Oxide itself, on every feature the framework ships today. The
dogfooding is the pitch: the site's social cards come from `/_oxide/og`, the
stats counter is a real island, blog posts stream through the real SSR
pipeline, and the whole thing deploys as one binary.

## What this app dogfoods

| Feature | Where |
|---|---|
| Real streaming SSR (React adapter, `ssr: 'real'`) | `oxide.config.ts` `componentFor` map |
| Root + section layouts, layout chaining (§8.2) | `routes/layout.tsx`, `routes/docs/layout.tsx` |
| Loaders (`defineLoad`), `error()`, ISR + cache tags | `routes/blog/[slug]/page.server.ts` |
| `generateParams` prerender enumeration | blog + docs server modules |
| `defineMeta` head merging (§16.3) | every `page.server.ts` |
| Islands with `visible` strategy, ODF-encoded props | `routes/index/page.tsx` + `islands/stats.tsx` |
| `loading.tsx` client-nav boundary | `routes/blog/[slug]/loading.tsx` |
| `not-found.tsx` / `error.tsx` | `routes/` |
| `route.ts` endpoints (RSS feed) | `routes/rss/route.ts` |
| Content collections + typed content (§16.5) | `content/blog`, `content/docs` |
| OG social cards (S-09) | `/_oxide/og?title=…&subtitle=…` |
| Disk L2 cache + L0 response cache | `cache.handler` config |
| Auto CSS pipeline (Lightning CSS, hashed, injected) | `routes/layout.tsx` imports `styles/global.css` |

Deliberately **not** used (documented gaps, see the repo's
"built, wiring in progress" list): partial prerendering, `content.*` runtime
queries (loaders read the markdown directly via `src/lib/md.server.ts` until
the typed query surface lands), `defineAction`/`enhance` forms (next up: the
newsletter signup).

## Run it

The app resolves `oxide` / `@oxide/react` through the pnpm workspace, so it
lives **inside the Oxide monorepo** as `website/`:

```sh
cd oxide
# one-time: register the folder in pnpm-workspace.yaml if needed ("website")
pnpm install

cargo build --release -p oxide-cli
target/release/oxide-cli routes --root website     # scan + typegen
target/release/oxide-cli dev    --root website     # dev server + HMR
target/release/oxide-cli build  --root website     # bundle, prerender, artifacts
target/release/oxide-cli start  --root website     # serve :3000
```

Check the wire: `curl -s localhost:3000/_oxide/health`,
`curl -s "localhost:3000/_oxide/og?title=Oxide&subtitle=hello" -o card.png`,
`curl -s localhost:3000/rss`.

## Notes / known deviations

- **`componentFor` is config-side** — that is the current SSR reality, not an
  accident. The spec-conformant shape (routes as a *path* + build-time
  collection, spec §6.4/D8) silently degrades to the IR renderer today; this
  is audit finding **P0-10** and is on the fix list. The config file is
  written so the migration is a delete-and-shrink.
- **Cache key is nested** `cache.l0.maxSize` (§14.1). The create-oxide
  scaffolder still emits a flat `l0MaxSize`, which `CacheConfig`'s
  `deny_unknown_fields` rejects — don't copy it from there.
- **HTTP/3** is off in the committed config for plain local dev; flip
  `server.http3` to `true` when serving over TLS (Alt-Svc advertisement is
  automatic on the TLS listener).
- `preview/` is a static design snapshot for people without the Rust
  toolchain — it is not part of the app and is not deployed.

## Deploy

`adapter: 'binary'` builds the single-binary target (full capability row).
For a container: switch to `adapter: 'docker'`, or ship the binary in a
distroless image with the `dist/` folder — the build emits everything the
server needs next to it.

## Content workflow

- Blog: `content/blog/*.md` — flat frontmatter (`title`, `description`,
  `date`, `tags`), slug = filename stem. The same files are the canonical
  source for the dev.to / Medium cross-posts (see the launch kit folder).
- Docs: `content/docs/*.md` — frontmatter `title`, `description`, `order`
  (sidebar sort). `/docs` renders `getting-started`; `/docs/<slug>` the rest.
- Markdown is rendered by `src/lib/md.server.ts` (headings, lists, tables,
  fenced code, quotes, inline code/bold/links) — keep posts inside that
  subset and everything renders identically on the site and in cross-posts.
