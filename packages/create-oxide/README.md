<div align="center">
  <img src="../../docs/logo.png" alt="Oxide" width="140"/>
  <p>
    <strong>Scaffold an Oxide starter.</strong><br/>
    One command generates a runnable project for any of the four
    framework adapters, in one of four shapes — minimal, blog, dashboard,
    or storefront. Zero dependencies; the generated project is ready to
    <code>oxide dev</code>.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Frameworks-React%20%C2%B7%20Vue%20%C2%B7%20Svelte%20%C2%B7%20Solid-9D4EDD?style=flat-square" alt="Four frameworks"/>
    <img src="https://img.shields.io/badge/Templates-minimal%20%C2%B7%20blog%20%C2%B7%20dashboard%20%C2%B7%20storefront-00BFFF?style=flat-square" alt="Four templates"/>
    <img src="https://img.shields.io/badge/Deps-zero-228B22?style=flat-square" alt="Zero dependencies"/>
    <img src="https://img.shields.io/badge/Node-%E2%89%A524-339933?style=flat-square&logo=node.js" alt="Node >=24"/>
    <img src="https://img.shields.io/badge/Licence-MIT%20OR%20Apache--2.0-228B22?style=flat-square" alt="Licence"/>
  </p>
  <p>
    <strong><a href="#quick-start">Quick start</a> · <a href="#templates">Templates</a> · <a href="#frameworks">Frameworks</a> · <a href="#what-gets-generated">What gets generated</a> · <a href="#licence">Licence</a></strong>
  </p>
</div>

## Quick start

```bash
pnpm create oxide my-app
```

The CLI asks two questions — which framework, which template — and writes
a complete project. Or pass them as flags for a non-interactive scaffold:

```bash
pnpm create oxide my-app --framework react --template blog
```

Then:

```bash
cd my-app
pnpm install
pnpm dev
```

The generated project pins `oxide`, the framework adapter, and the
framework runtime. `pnpm dev` boots the Rust-core dev server with HMR,
route-table rescan, and a live inspector.

## Templates

Each template is a self-contained directory that ends up in the project
as `src/routes/` plus a matching `oxide.config.ts`. The four shapes:

| Template | Routes | What it demonstrates |
| --- | --- | --- |
| `minimal` | `/` | The smallest runnable project: one page, one layout, one loader |
| `blog` | `/`, `/blog`, `/blog/[slug]` | Content collections, dynamic routes, MDX, streaming deferred comments |
| `dashboard` | `/`, `/dashboard`, `/dashboard/[tab]` | Nested layouts, per-route JS budgets, parallel loaders, islands |
| `storefront` | `/`, `/products`, `/products/[slug]`, `/cart` | Actions (add-to-cart as a form POST with a cookie), ISR on the catalog, image variants |

The template content is inline in `src/cli.ts` — the CLI itself is one
file. There is no template-download step; a scaffold works offline.

## Frameworks

| Flag | Adapter | Suspense |
| --- | --- | --- |
| `--framework react` | `@oxide/react` | `native` (React `<Suspense>`) |
| `--framework vue` | `@oxide/vue` | `core` (`<Defer>` from the adapter) |
| `--framework svelte` | `@oxide/svelte` | `core` (`<Defer>` from the adapter) |
| `--framework solid` | `@oxide/solid` | `native` (Solid's own `<Suspense>`) |

The choice determines the adapter package installed, the
`jsxImportSource`-equivalent written into the config, and the component
file extension (`.tsx` for React and Solid, `.vue` for Vue, `.svelte` for
Svelte). Everything else — routing, data loading, actions, forms,
caching, streaming — is identical.

## What gets generated

A scaffolded project is a normal pnpm project with:

```
my-app/
  oxide.config.ts         # framework + routes dir + deploy target
  package.json            # oxide, @oxide/<framework>, framework runtime
  tsconfig.json           # strict, moduleResolution: bundler
  src/
    routes/
      layout.tsx          # the outermost layout
      index/page.tsx      # the home route
      ...                 # template-specific routes
```

No `vite.config.ts`, no `webpack.config.js`, no `next.config.js`. The
build is the Rust core; the config is one object; the routes are the
directory structure.

## Licence

MIT OR Apache-2.0 — same as the rest of Oxide.
