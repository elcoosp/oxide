# Oxide: Technical Specification

*Working name · Draft 0.1 · 11 Oct 2026 · Status: design proposal. Performance figures are targets to validate, not measurements. Dependency versions and APIs must be re-verified before lock-in.*

## 0. Conventions

MUST / SHOULD / MAY follow RFC 2119. Architectural decisions are numbered D1 to D20 (§2). Unresolved items are marked OPEN and collected in §26.

## 1. Product definition

**Oxide is a framework-agnostic, full-stack meta-framework with a Rust core.** React, Vue, Svelte and Solid are first-class adapters over one shared core that owns routing, data, caching, bundling, serving and deployment. A feature that can be framework-neutral lives in the core, so it behaves identically on all four.

### 1.1 Goals

- **G1 Performance:** best-in-class dev start, HMR, production build, TTFB, throughput, and client bytes.
- **G2 DX:** one small typed config, a Rollup/Vite-shaped plugin API, end-to-end types, excellent errors.
- **G3 Feature fullness:** parity with the production feature sets of Next.js, Nuxt, SvelteKit and SolidStart.
- **G4 Uniformity:** the same features, config, and conventions for every framework.
- **G5 Web standards:** `Request`, `Response`, streams, `URLPattern`, Web Crypto at every boundary.
- **G6 Deploy anywhere:** single binary, Node, containers, static, edge.
- **G7 Ecosystem reuse:** use each framework's own compiler and existing Rollup/Vite plugins instead of rewriting them.

### 1.2 Non-goals

- Rewriting framework compilers or runtimes (React, Vue, Svelte, Solid stay upstream).
- Writing a type checker (delegate to `tsgo`/`tsc`), linter (`oxlint`) or formatter.
- Full emulation of Vite's plugin container (we support the hooks real plugins use; see §15).
- Native Node addons inside the embedded V8 (served by the Node/Bun compatibility backend, §4.6).
- Rust-authored UI at launch. A Leptos/Dioxus adapter comes later and serves as the neutrality test for the adapter contract.

### 1.3 MLP definition

A team can build, test and deploy a production site (marketing pages, blog, authenticated dashboard with forms, product catalog with search and images) in any of the four frameworks using only Oxide, and on the reference apps Oxide beats that framework's incumbent meta-framework on dev start, HMR latency, build time, TTFB, throughput and client JS bytes.

## 2. Decision record

| ID | Decision | Rationale |
| --- | --- | --- |
| D1 | Rust owns everything except user code and JS-only compilers | Performance and a single-binary story |
| D2 | JS host: embedded V8 via `deno_core` primary; Node/Bun worker pool as compatibility backend, behind a `JsHost` trait | Snapshots, zero-copy streaming ops, fast cold start; Node pool covers native addons |
| D3 | Bundler: Rolldown used as a library | Rust, Rollup-compatible, aligned with Vite's direction |
| D4 | Transforms: Oxc native for TS/JSX/refresh; framework compilers in a cached tooling pool | Reuse upstream compilers; cache makes cost one-time |
| D5 | CSS: Lightning CSS; Tailwind v4 through its Rust engine | Fastest CSS path, no PostCSS dependency by default |
| D6 | Incremental engine: small custom demand-driven graph with early cutoff plus a persistent content-addressed store (blake3, redb, rkyv) | Needs async, cancellation and disk persistence that `salsa` does not provide out of the box; `turbo-tasks` is not packaged for reuse |
| D7 | Server: Tokio multi-thread, Hyper 1.x, rustls, `mimalloc`; HTTP/3 via `quinn`/`h3` opt-in | Mature and fastest practical stack |
| D8 | Router: build-time compiled radix tree, rkyv manifest loaded zero-copy | No route scanning at boot |
| D9 | Client router is owned by the core (about 3 KB), not by adapters | Identical navigation, prefetch, scroll and transitions on all frameworks |
| D10 | Framework-neutral route modules (`*.server.ts`) hold load/actions/meta/cache | Server logic written once |
| D11 | Wire format: Rust-native encoder (V8-walking op) of a devalue-compatible format, about 1 KB JS decoder | Serialization without JS stringify cost; bytes feed the cache directly |
| D12 | Hydration: full-route by default, islands opt-in per route or component | Matches framework norms; islands give zero-JS pages |
| D13 | Middleware: declarative rules evaluated natively in Rust, plus JS middleware when logic is dynamic | Redirects, rewrites, headers cost no JS call |
| D14 | Config: TypeScript, statically extracted by Oxc; V8 eval only for dynamic config | No JS runtime on the boot path |
| D15 | Dev: on-demand native-ESM transforms, Rolldown-prebundled dependencies, server environment loaded straight from the module graph (no HTTP hop) | Fast start, fast HMR, parity-testable |
| D16 | Typecheck: `tsgo`; lint: `oxlint`; format: `oxfmt`/Biome | Do not reinvent |
| D17 | React Server Components: post-MLP React-only capability; extension points reserved now | Not framework-neutral; do not distort the core |
| D18 | Edge: compile a core subset (router, cache keys, head merge, serializer) to WASM, emit WinterCG-compatible JS | Core features work where Rust cannot run V8 |
| D19 | Images and fonts: pure-Rust default backends behind traits; libvips optional | Single binary by default, speed on demand |
| D20 | Conformance suite gates every release: one reference app, four adapters, same assertions | Prevents adapter drift |

## 3. System overview

```
 oxide (single binary)  dev · build · start · preview · routes · analyze · check · doctor
        │
 Config loader (Oxc static extract → V8 fallback)
        │
 ┌──────▼────────────── Core graph engine (§5) ──────────────────┐
 │ in-memory demand graph + persistent CAS (.oxide/cache)         │
 └──┬────────┬─────────┬──────────┬──────────┬──────────┬────────┘
    │Compile │Bundle   │Router    │Assets    │Server    │Cache
    │Oxc     │Rolldown │compiled  │LightningC│Tokio     │moka→disk
    │        │3 envs   │+typegen  │images    │Hyper     │→remote
    └────────┴────┬────┴──────────┴──────────┴────┬─────┴────────┘
                  │ JsHost trait                    │ streaming ops
         ┌────────▼───────────┐          ┌────────▼───────────┐
         │ Tooling pool       │          │ Render pool        │
         │ compilers, plugins │          │ V8 isolates, SSR   │
         └────────┬───────────┘          └────────┬───────────┘
                  └─────── Framework adapters ─────┘
                    react · vue · svelte · solid · (leptos…)
```

**Process model.** One binary. The main thread runs Tokio. The render pool is N isolate threads (default: physical cores). The tooling pool is M workers used in dev and build only. In production, with the tooling pool absent, memory use is the Rust server plus the isolates.

## 4. Execution model: Rust core and JS host

### 4.1 The boundary

- **Rust:** networking, TLS, routing, middleware rules, caching, serialization, bundling, file watching, asset pipelines, stream plumbing, Oxc transforms, config.
- **JS (V8):** component rendering, route modules (`load`, actions), user middleware, and the Svelte, Vue, Solid and optional React Compiler transforms.
- **Rule:** the hot request path MUST be able to complete without entering JS when the answer is cached, redirected, rewritten, or static (D13).

### 4.2 `JsHost` trait

```rust
trait JsHost: Send + Sync {
    fn render_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>>;
    fn tooling_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>>;
}
trait Pool: Send + Sync {
    async fn call(&self, entry: EntryRef, req: CallInput) -> Result<CallStream>;
    fn stats(&self) -> PoolStats;
}
```

Backends: `V8Host` (default) and `NodeHost` (child workers, same op protocol over a Unix socket with a shared-memory ring buffer for bodies). Both pass the same conformance suite.

### 4.3 Render pool

- One `JsRuntime` per OS thread; isolates are not `Send`, so each is pinned.
- **Concurrency:** each isolate interleaves up to K in-flight requests (default 64) at await points. Each request runs inside an `AsyncContext` scope so request state never leaks between interleaved renders.
- **Dispatch:** least-loaded isolate with route-affinity preference (keeps JIT and module caches warm for hot routes).
- **Lifecycle:** per-isolate heap limit; an isolate is replaced after a request-count or heap threshold. A warm replacement is created before the old one drains, so there is no latency cliff.
- **Failure isolation:** a crash, OOM or watchdog timeout kills one isolate and fails only its in-flight requests, which the supervisor retries once if they are idempotent.

### 4.4 App snapshots (cold start)

At `oxide build`, after the server environment is bundled, Oxide boots the server bundle inside a V8 snapshot builder, evaluates module top levels (framework runtime, adapter, route registry, compiled templates), and serializes a startup snapshot. At `oxide start` each isolate deserializes it in a few milliseconds instead of re-evaluating code.

- **Constraints:** no live handles (sockets, timers) at snapshot time. User modules MUST create connections lazily. `oxide build` lints top-level side effects and reports them with source locations.
- **Fallback:** if the snapshot fails or a dependency is snapshot-hostile, use a V8 code cache for the bundle (still skips parsing and compilation).

### 4.5 Streaming ops

Rust exposes a small op surface to JS, each op using `deno_core` fast calls where possible:

| Op | Purpose |
| --- | --- |
| `stream_write(id, Uint8Array) → Promise` | Push a body chunk. The promise resolves when the Rust channel has capacity, which gives real backpressure |
| `response_head(id, status, headers)` | Commit status and headers |
| `request_body_read(id) → Promise<Uint8Array>` | Pull request body chunks |
| `serialize(value) → Uint8Array` | Rust encoder walks the V8 value graph |
| `cache_get/set/tag` | Tiered cache (§11) |
| `fetch` | Hyper client with pooled connections and DNS cache |
| `ws_*` | WebSocket frames for socket routes |
| `log/trace` | Structured tracing into Rust |

Chunks cross the boundary as backing-store handoffs into `bytes::Bytes`, never copied through strings.

### 4.6 Runtime compatibility tiers

1. **Web globals in V8:** `fetch`, `URL`, `URLPattern`, streams, `crypto.subtle`, `TextEncoder`, `structuredClone`, `AsyncLocalStorage`.
2. **`node:` subset in V8** (via reusable `deno_node` modules): `path`, `buffer`, `crypto`, `stream`, `events`, `util`, `url`, `async_hooks`, read-only `fs`.
3. **Node/Bun backend:** chosen automatically when the build detects a `.node` addon or an unsupported builtin, or explicitly via `runtime: 'node' | 'bun'`. The build prints why it switched.

### 4.7 Tooling pool

Workers evaluate JS compilers and JS plugins. Requests are batched and dispatched in parallel. Every result is stored in the CAS keyed by `blake3(input, compiler version, options)`. A file compiles through a JS compiler at most once per content version across dev sessions, branches and CI (with remote cache).

## 5. Core graph engine

### 5.1 Model

Every build step is a pure function node. Two tiers:

- **Tier A (memory):** demand-driven graph with fine-grained dependency tracking and **early cutoff**: if a recomputed node yields the same output hash, dependents are not invalidated.
- **Tier B (disk):** a content-addressed store for expensive pure actions (transform, framework compile, chunk render, image transform, CSS bundle).

### 5.2 Tracked inputs

File contents (by blake3), directory listings, resolver results, env vars, config keys, plugin options, tool versions. Reading an input records the edge, so changes invalidate only the nodes that observed them.

### 5.3 Granularity

`read → parse → transform → import-scan` per module; module graph per environment; chunk graph; per-chunk output; per-asset output; per-route manifest entry; typegen per route. Granularity is small enough that editing one component re-runs one transform and re-hashes its chunk.

### 5.4 Persistence

- Index in `redb`, blobs content-addressed and `zstd`-compressed, artifacts serialized with `rkyv` (validated by hash on load).
- Cache directory `.oxide/cache`; cache key includes Oxide version, adapter version, plugin versions and option hashes, target, and tracked env.
- `RemoteCache` trait (HTTP/S3) for CI; read-through by default, write on trusted branches.
- Garbage collection by last-use time and size cap.

### 5.5 Watching

`notify` with debounce, plus content-hash confirmation so touch-without-change does nothing. Lockfile changes invalidate resolver nodes. Optional Watchman backend for very large monorepos.

### 5.6 Concurrency and cancellation

Nodes run on a work-stealing pool (CPU nodes on `rayon`, IO and JS nodes on Tokio). A newer file change cancels in-flight stale work, and the dev server never serves a result computed from stale inputs.

## 6. Compile and bundle pipeline

### 6.1 Environments

Named after Vite's Environment API for plugin familiarity: `client`, `ssr`, optional `edge`. Each has its own resolve conditions, platform, target, externals, define set and plugin list. Adapters may add environments.

### 6.2 Transform order

1. Resolve with `oxc_resolver` (tsconfig paths, package `exports`, conditions per environment).
2. Load (native loaders, then plugins).
3. Plugin transform chain, native plugins first.
4. Oxc strip TS, lower JSX, downlevel to target.
5. Framework transform (§6.3).
6. Inject HMR/refresh boundaries (dev only).
7. Import scan for the graph and preload hints.

### 6.3 Per-framework compile

| Framework | Transform path | Notes |
| --- | --- | --- |
| React | Oxc JSX (automatic runtime) and native React Refresh | Optional React Compiler via `babel-plugin-react-compiler` in the tooling pool; native port OPEN |
| Vue | `@vue/compiler-sfc` in tooling pool; script/TS lowered by Oxc | Template output cached by content hash |
| Svelte | `svelte/compiler` in tooling pool | Runes supported; HMR via adapter strategy |
| Solid | `babel-preset-solid` (dom-expressions) in tooling pool | Highest-value candidate for a native Oxc-visitor port, since cold-build speed depends on pool throughput |

Native replacements are a post-MLP optimization track ordered by measured cold-build share. The cache makes them optimization, not launch blockers.

### 6.4 Bundling

Rolldown as a library, one invocation per environment, driven by the graph engine so unchanged chunks are never rebuilt.

- Route-based code splitting, shared-chunk heuristics tuned for route trees, per-chunk CSS, dynamic-import preloads, no module-preload polyfill by default.
- Content-hashed immutable output.
- **Manifests:** client manifest (route → entry chunks, CSS, preload order, assets), server manifest (route → server module, adapter entry), and a build id.
- Tree shaking runs per environment, so server-only modules cannot leak into the client. A compile-time guard fails the build on forbidden imports (e.g. a `*.server.ts` module reachable from the client entry except via the server-function stub transform, §9.4).

### 6.5 Dev dependency prebundling

Oxc scans entry imports; Rolldown bundles each bare-import dependency graph into ESM; results are cached by lockfile hash. Subsequent starts reuse them.

### 6.6 CSS

Lightning CSS for parsing, nesting, modules, vendor prefixing, minification and source maps, with targets from browserslist or the `baseline-widely-available` default. Tailwind v4 integrates as a native plugin using its Rust scanner against graph-known files, so content scanning needs no separate glob pass.

### 6.7 Asset imports

`?url`, `?raw`, `?inline`, `?worker`, `?w=640&format=avif` image queries, and SVG-as-component per adapter. All results are CAS-cached.

## 7. Framework adapter contract

The contract is versioned (`contract: 1`) and frozen before the first public beta. An adapter is an npm package exporting a manifest plus runtime entries.

```ts
export interface FrameworkAdapter {
  contract: 1
  name: string
  extensions: string[]                    // ['.tsx'] | ['.vue'] | ['.svelte']
  jsxImportSource?: string
  capabilities: {
    suspense: 'native' | 'core'           // who owns streaming boundaries
    islands: boolean
    hmr: 'refresh' | 'component' | 'full'
    rsc?: boolean                          // reserved
  }
  plugins(opts: unknown): Plugin[]          // framework compile plugins

  // server entry
  render(input: RenderInput): ReadableStream<Uint8Array>   // layouts + page + data
  head(ctx: RenderCtx): HeadTags           // framework head model to neutral model

  // client entry
  hydrate(root: Element, input: ClientInput): Disposer
  hydrateIsland?(el: Element, mod: unknown, props: unknown): Disposer
  mount(root: Element, input: ClientInput): Disposer    // SPA mode
  update(input: ClientInput): void          // client navigation: swap route tree
}
```

**The adapter owns** components, rendering, hydration and HMR. **The core owns** routes, loaders, actions, cache, head merging, serialization, middleware, client navigation, assets, streaming protocol.

### 7.1 Cross-framework streaming

React and Solid have native Suspense streaming; Vue's is partial; Svelte renders synchronously per tree. To give every framework identical streaming, the core defines a **deferred-boundary protocol**:

- A boundary emits a placeholder `<template data-ox-b='ID'>` with fallback content.
- When data resolves, the core streams `<template data-ox-c='ID'>…</template>` plus a 300-byte inline swapper, with CSP nonce.
- Adapters with `suspense: 'native'` use their own mechanism; adapters with `suspense: 'core'` get boundaries from a `<Defer>` component the core exports per framework (generated from the adapter).
- Hydration of streamed content is the adapter's job, triggered after the swap event.

### 7.2 Adapter obligations

Streaming render with backpressure; no module-global request state; deterministic hydration markup; head extraction; HMR implementation; error mapping to the core error model; passing the conformance suite (§22). An adapter that cannot pass an assertion MUST declare the capability gap in its manifest, and the docs surface it.

## 8. Routing and route modules

### 8.1 Conventions

```
src/routes/
  layout.{tsx|vue|svelte}        UI wrapper (nested)
  layout.server.ts               load / config for the layout
  page.{tsx|vue|svelte}          route UI
  page.server.ts                 load, actions, meta, config (framework-neutral)
  loading.* error.* not-found.*  boundaries
  template.*                     remounts on navigation
  route.ts                       Request → Response handlers per method
  middleware.ts                  JS middleware (scoped by directory)
  (group)/  [param]/  [...rest]/  [[...optional]]/
  @slot/  (.)intercept/          parallel and intercepting routes (§8.5)
```

Location of `src/routes` is configurable. Conventions are lowercase, extension-agnostic, and identical across adapters. A **Next-compat mode** maps `app/**/page.tsx` and `layout.tsx` and `route.ts` for migration.

### 8.2 Matching

The route tree is compiled at build into a radix tree (static segments, params with optional regex constraints, catch-all) with priority rules: static > param > optional > catch-all. The tree is stored as an `rkyv` archive and memory-mapped. Match is allocation-free on the hot path, and `URLPattern` semantics are preserved for the edge build.

### 8.3 Typed routes

Typegen writes `.oxide/types/routes.d.ts` with params, search params (via optional schema), loader data and action I/O per route, and a typed `href()` and `<Link>`/`goto()` per adapter. Regeneration is a graph node, so types update in milliseconds on route changes.

### 8.4 Client router (core-owned)

About 3 KB. Responsibilities: link interception, History API, scroll restoration, focus management, View Transitions, prefetch on hover/viewport/intent (budgeted and cancelable), route-data fetching (`/__data` endpoint using the §9.5 format), optimistic navigation with loading boundaries, and scroll and form-state preservation on back/forward. It calls `adapter.update()` to swap the tree. Because the core owns this, navigation semantics are identical across frameworks.

### 8.5 Advanced routing

Parallel (`@slot`) and intercepting routes are post-MLP but the tree format and client router reserve slot identity from day one. Route groups, optional catch-all, i18n prefixes, redirects and rewrites (declared in config or `middleware.ts`) are MLP.

## 9. Data layer: loaders, actions, server functions

### 9.1 Route module API

`page.server.ts` and `layout.server.ts` are plain TypeScript, shared by every adapter.

```ts
import { defineLoad, defineAction, defineMeta, cache, redirect, error } from 'oxide/server'
import { z } from 'zod'   // any Standard Schema validator

export const config = { render: 'isr', revalidate: 60, tags: ['posts'] }

export const load = defineLoad(async ({ params, request, locals, cookies, defer, setHeaders }) => {
  const post = await getPost(params.slug)              // 'use cache' function, see 9.3
  if (!post) error(404)
  return { post, comments: defer(getComments(params.slug)) }   // streamed later
})

export const actions = {
  comment: defineAction({
    input: z.object({ body: z.string().min(1).max(2000) }),
    async run({ input, params, locals, revalidate }) {
      await db.comments.insert({ postId: params.slug, ...input, user: locals.user.id })
      revalidate.tag(`comments:${params.slug}`)
      return { ok: true }
    },
  }),
}

export const meta = defineMeta(({ data }) => ({ title: data.post.title, description: data.post.excerpt }))
```

Types for `params`, `data`, `input` and results are generated (§8.3), so components receive typed `data` and `form` props or composables in each framework's idiom.

### 9.2 Load semantics

- Layout and page loads for a matched route run **in parallel**; a load that needs a parent's data calls `await parent()` explicitly, so waterfalls are always visible in code.
- `defer(promise)` returns a placeholder in data and streams the resolved value via the core boundary protocol (§7.1). The client sees a normal promise-like value in each adapter (React `use()`, Vue ref, Svelte await block, Solid resource).
- `redirect()`, `error()`, `setHeaders()` and cookie access are standard Web API wrappers. Request-scoped `locals` are populated by middleware.
- Loads run in the isolate only on a cache miss at the route (L0) and data (L1) levels (§11).
- Client-side navigations re-run only the loads whose inputs changed (params, tracked search params, invalidated tags).

### 9.3 Actions and `'use cache'`

- **Actions are form-first.** `<form method='post' action='?/comment'>` works with JavaScript disabled: the server validates (Standard Schema), runs, and re-renders the page with `form` result data or redirects (POST/redirect/GET). With JavaScript, the adapter's `enhance` helper intercepts the submit, calls the action over `fetch`, updates data in place, preserves focus and scroll, and supports optimistic updates.
- Multipart uploads stream to the handler with size limits enforced in Rust before JS sees the body.
- **`'use cache'` directive.** In a `*.server.ts` module, an async function marked `'use cache'` is compiled (Oxc pass) to a cache wrapper. The key is `hash(module, export, args encoded with the §9.5 format)`, with optional `cacheTag()` and `cacheLife()` calls inside. The explicit form `cache(fn, { ttl, tags, swr })` is equivalent.

### 9.4 Server functions (typed RPC)

For client-triggered operations outside forms:

```ts
// cart.server.ts
import { serverFn } from 'oxide/server'
export const addToCart = serverFn(async (productId: string) => { /* ... */ })
```

Importing `addToCart` from client code compiles (Oxc) to a stub that calls `POST /_oxide/fn/<id>`. `<id>` is a stable hash of file and export name plus a per-build secret, so ids are unguessable and rotate. Server functions are POST-only, run origin and CSRF checks, pass through middleware, validate input if a schema is attached, and use the §9.5 format in both directions. This is the only way server modules are reachable from client code (§6.4 guard). Server functions are tree-shaken if unreferenced, so dead endpoints are never exposed.

### 9.5 Wire format (ODF)

A devalue-compatible superset: JSON primitives plus `Date`, `Map`, `Set`, `BigInt`, `URL`, `RegExp`, typed arrays, `undefined`, cyclic and shared references, `Error`, and promises (as streamed deferred slots). The encoder is a Rust op that walks the V8 value graph and writes bytes directly, so no `JSON.stringify` and no JS string allocation, and the same bytes go to the cache and to the wire. The decoder is a JS module of about 1 KB. Custom types are registered through transport hooks (for example ORM objects or class instances).

HTML embeds data in `<script type='application/oxide-data' nonce='...'>`; client navigation fetches the same format from `/__data`.

### 9.6 Endpoints, sockets, SSE

- `route.ts` exports `GET`, `POST`, etc., each `(event) => Response`. Streaming bodies are first-class.
- **WebSockets:** `export const socket = defineSocket({ open, message, close })`. Rust owns the upgrade, framing, ping/pong and backpressure; JS handlers receive messages via ops.
- **SSE:** return a `Response` with an event-stream body. Rust handles keepalive and disconnect detection and aborts the handler's `AbortSignal`.

## 10. Rendering modes and the streaming pipeline

### 10.1 Modes

| Mode | Behavior |
| --- | --- |
| `static` | Prerendered at build; served by Rust from the L0 cache, zero JS at runtime |
| `isr` | Static plus time- or tag-based background regeneration (stale-while-revalidate) |
| `dynamic` | Rendered per request; may still use data caches |
| `spa` | Empty shell prerendered; client renders |
| `islands` | Server HTML only; selected components hydrate (§10.3) |

**Inference:** a route is `static` unless its load reads request-specific values (cookies, headers, search params not declared as cache keys, `locals`), in which case it is `dynamic`. The compiler reports the inferred mode and the exact read that made it dynamic (`oxide routes` and the dev inspector show this). Explicit `config.render` always wins.

### 10.2 Request render sequence

1. Route matched and L0 cache checked (Rust). Hit means serve and finish.
2. Run route and layout loads in the isolate (data cache consulted via ops first).
3. Call `adapter.render()`; the shell streams as soon as the first flushable chunk exists.
4. Rust injects, through `lol_html`: merged `<head>` tags, CSS and modulepreload links in critical order, CSP nonces, ODF data, and hydration bootstrap.
5. Deferred boundaries stream as they resolve (§7.1). If a stream errors after the shell is committed, the error boundary content is streamed and the client is told to recover.
6. On completion, eligible responses are stored in L0 and tags are recorded.

**Early Hints (103)** are sent for known critical assets at step 1 using the route's client manifest, even before loads finish.

### 10.3 Islands

In `islands` mode a component marked `client:load | client:idle | client:visible | client:media(...)` renders on the server inside `<ox-island>` with ODF-encoded props and a chunk reference. The core's 1.5 KB loader schedules hydration, and the adapter's `hydrateIsland` performs it. Each island is its own chunk. Pages with no islands ship no framework JavaScript. Mixing frameworks within one page is designed for (island chunks are adapter-qualified) but ships post-MLP.

### 10.4 Partial prerendering (post-MLP)

Because boundaries are core-defined, PPR is framework-neutral: the static shell up to each boundary is cached in L0, and only the holes render per request and stream into the cached shell. Format and cache keys are reserved in M4; implementation lands in M7.

### 10.5 Static export

`oxide build --static` prerenders all `static` routes (including `generateParams` for dynamic ones), emits HTML plus assets and `_headers`/`_redirects` for common hosts, and fails if a `dynamic` route exists unless `static.fallback` is configured.

## 11. Caching

### 11.1 Layers

| Layer | What | Where |
| --- | --- | --- |
| L0 | Full response (HTML, headers, tags) | Rust, no JS on hit |
| L1 | Data from `cache()` / `'use cache'` (ODF bytes) | Rust tiers |
| L2 | Outbound `fetch` memoization within a request and optional cross-request | Rust fetch op |
| Build cache | Graph artifacts (§5.4) | `.oxide/cache` plus remote |

### 11.2 Tiered store

`CacheHandler` trait: `get`, `set`, `delete`, `tag_keys`, `invalidate_tag`, plus optional pub/sub for cross-node invalidation. Default tiers are `moka` (memory, TinyLFU, weighted by bytes), then local disk (`redb` index plus blob files), then a pluggable remote (Redis, S3, KV, HTTP). L0 keys: route id, params, normalized Vary dimensions (language, declared cookies, `Accept` for negotiated resources), and build id.

### 11.3 Semantics

- **TTL, stale-while-revalidate, stale-if-error** per entry.
- **Single-flight coalescing:** concurrent misses for the same key run one render.
- **Tags:** entries carry tags; `revalidate.tag('x')` marks matching entries stale (soft) or deletes (hard); `revalidate.path('/blog/a')` likewise. Soft invalidation serves stale then regenerates in the background.
- **Distributed invalidation** propagates via the handler's pub/sub (e.g. Redis) so multi-node deployments stay coherent.
- **Build seeding:** `static` and prerendered ISR pages are inserted into the cache at deploy start, so first requests hit.
- **HTTP headers** (`Cache-Control`, `ETag`, `Vary`, `Last-Modified`) are generated from route policy; conditional requests (`304`) are answered by Rust.

## 12. Server runtime

### 12.1 Request pipeline

1. Accept (`SO_REUSEPORT` listeners), TLS (`rustls`), HTTP/1.1 and HTTP/2; HTTP/3 opt-in.
2. Limits and request id; trusted-proxy handling; `Host` allow-list.
3. Static asset match: immutable hashed assets served from memory-mapped or embedded files, pre-compressed (`zstd`, `br`, `gzip`), with range and `If-None-Match` support.
4. **Native rules** (redirects, rewrites, headers, i18n locale resolution, basic auth gates) compiled from config into a table and evaluated in Rust.
5. Route match, then L0 lookup.
6. JS middleware only if the matched route chain contains it.
7. Dispatch to render, action, endpoint, or server function.
8. Streaming response with dynamic compression (`zstd` preferred, `br`, `gzip`), flush heuristics tuned for first-byte, `Server-Timing` emitted.
9. Cache fill.

### 12.2 Operational features

- Graceful shutdown with connection draining; zero-downtime reload by starting a new process on the same port via `SO_REUSEPORT`.
- Health (`/_oxide/health`) and readiness endpoints; Prometheus metrics endpoint (opt-in).
- Optional ACME (automatic TLS) for single-binary deployments.
- Global allocator `mimalloc`; per-request arenas for hot structures; connection and body limits with slow-loris protection.
- Assets can be embedded in the binary (`adapter: 'binary', embed: true`) or read from disk.

## 13. Dev server and HMR

### 13.1 Startup

Load config (static extract), open the persistent cache, start the file watcher, restore the route manifest and prebundled deps, and start listening. No bundling happens before the first request. Routes compile on demand when first requested; a background task pre-warms likely next routes after idle. Target: ready in under 300 ms on a cached 1,000-module app.

### 13.2 Serving

Client modules are served as native ESM with Oxc-transformed output and `?v=<hash>` cache busting; dependencies come from the prebundled cache. The **server environment does not use HTTP**: the isolate module loader reads transformed modules directly from the graph engine, so SSR in dev is the same adapter code as production with only the module source differing.

### 13.3 HMR protocol

WebSocket messages carry module updates, CSS updates, route-data updates, server updates, full-reload and error events, with an update boundary computed in Rust from the graph. Per-adapter strategy:

| Change | Behavior |
| --- | --- |
| Component file | Adapter HMR (React Refresh, Vue HMR, `svelte-hmr`, `solid-refresh`) preserves state where the framework allows |
| CSS | Style swap, no reload |
| `*.server.ts` loader | Re-run loader, push new ODF data, re-render in place |
| Server-only module | Swap in live isolates; next request uses new code, client reloads data without full reload |
| Config / route tree | Restart affected subsystems; only a config that changes the adapter forces process restart |

### 13.4 Error overlay and inspector

Errors are Rust `miette` diagnostics serialized as JSON (code frames, source-mapped stacks, hints with fixes) and drawn by a small overlay. The `/__oxide` inspector shows the module graph, `oxide why <module>` explanations (importers and chunks), the route table with inferred modes, cache entries and tags, per-request timing waterfalls (middleware, load, render, injection), and the isolate pool state.

### 13.5 Other dev features

Built-in HTTPS with locally trusted certificates, proxy rules, env reload, network URL display with QR code, browser open, and `dev.bundle: true` (full-bundle dev mode via Rolldown) as an opt-in for production-parity debugging once validated.

## 14. Configuration

### 14.1 Example

```ts
// oxide.config.ts
import { defineConfig } from 'oxide'
import react from '@oxide/react'          // or @oxide/vue | @oxide/svelte | @oxide/solid
import tailwind from '@oxide/tailwind'

export default defineConfig({
  framework: react({ compiler: true }),
  routes: { dir: 'src/routes' },
  server: { port: 3000, compression: ['zstd', 'br'], http3: false },
  render: { default: 'auto' },
  cache: { handler: { type: 'disk' }, l0: { maxSize: '512mb' } },
  images: { formats: ['avif', 'webp'], remotePatterns: [{ host: 'cdn.example.com' }] },
  fonts: { providers: ['google'], subset: ['latin'] },
  i18n: { locales: ['en', 'fr'], default: 'en', strategy: 'prefix-except-default' },
  security: { csp: 'strict', csrf: 'origin' },
  rules: [{ source: '/old/:slug', redirect: '/new/:slug', status: 308 }],
  budgets: { clientJs: '150kb', routeJs: '60kb' },
  build: { target: 'baseline-widely-available', sourcemap: 'hidden' },
  adapter: 'binary',                       // binary | node | static | docker | cloudflare | ...
  plugins: [tailwind()],
  modules: [],
})
```

### 14.2 Rules

- **Static extraction:** Oxc evaluates literal `defineConfig` objects, including imported adapter and plugin factory calls with literal arguments (resolved against known manifests). Anything dynamic (functions, `process.env` reads outside the allowed set, top-level await) triggers V8 evaluation, cached by hash of the file and its tracked env.
- **Layering:** defaults, then preset, then `oxide.config.ts`, then mode file (`oxide.config.production.ts`), then env (`OXIDE_*`), then CLI flags.
- **Env files:** `.env`, `.env.local`, `.env.[mode]`, `.env.[mode].local`; variables prefixed `PUBLIC_` are exposed to client code. A typed env schema validates at boot and generates types; missing required variables fail with a diagnostic naming the source.
- **Per-environment overrides:** an `environments` key (client, ssr) accepts Rolldown options as an escape hatch.
- **Presets and monorepos:** `extends` merges shared configs; workspaces are detected and linked packages are watched.
- **Typed and validated:** the config schema is the single source for TypeScript types, JSON Schema (editor support), and runtime validation with suggestions for typos.

## 15. Plugin and module system

### 15.1 Plugin kinds

1. **Native (built in):** official plugins (Tailwind, images, fonts, MDX, i18n) compiled into the binary and enabled by config. Fastest path.
2. **WASM component plugins:** third-party native-speed plugins authored in Rust, Go and others, loaded with a WASM runtime against a stable WIT interface. Safe, portable, no per-platform binaries. OPEN: confirm that interface can cover `transform` throughput targets.
3. **JS plugins:** Rollup/Vite-shaped objects executed in the tooling pool.

All three share one hook model and ordering (`enforce` set to pre or post).

### 15.2 Hooks

- **Build (Rollup/Vite-shaped):** `config`, `configResolved`, `buildStart`, `resolveId`, `load`, `transform`, `renderChunk`, `generateBundle`, `buildEnd`, `transformHtml`, `handleHotUpdate`, limited `configureServer`.
- **Oxide-specific:** `routes` (add or rewrite routes), `adapter` (hooks on build manifests), `serverMiddleware`, `renderHead`, `cacheHandler`, `typegen` (contribute declarations), `diagnostics`.

Hooks the bridge does not support fail at load with a clear message and a link to the compatibility table, not silently at runtime.

### 15.3 Compatibility targets

CI-tested against the official framework plugins' behavior (`@sveltejs/vite-plugin-svelte`, `@vitejs/plugin-vue`, `vite-plugin-solid`), plus MDX, SVGR, Tailwind, and the common `unplugin-*` family. The compatibility table is published and versioned.

### 15.4 Modules

A module (`defineModule`) is a distributable bundle of plugin hooks, routes, middleware, config defaults and typegen, installed with one line in `modules`. Auth, CMS, analytics and UI-kit integrations ship as modules. Modules declare which adapters they support.

## 16. Platform features

### 16.1 Images

Per-adapter `<Image>` component and `?w=` import queries. Build-time transforms for local assets; a runtime `/_oxide/image` endpoint for dynamic and remote sources (allow-listed by `remotePatterns`, signed URLs optional). Outputs AVIF, WebP, JPEG and PNG with `srcset`, `sizes`, LQIP or blur placeholders, intrinsic dimensions to prevent layout shift, lazy loading, and `priority` that emits preload hints. Pipeline: `image` plus `fast_image_resize` for decode and resize, `ravif` and `webp` encoders, `oxipng`, concurrency-limited worker threads, results in the CAS and L1. Optional libvips backend through the `ImageBackend` trait.

### 16.2 Fonts

Local and provider fonts are fetched at build, subset with `fontations`/`skrifa`, converted to WOFF2, self-hosted, preloaded, and given computed `size-adjust` metric overrides on fallback faces to eliminate font-swap layout shift.

### 16.3 Metadata and SEO

A neutral metadata model (title templates, description, canonical, robots, Open Graph, Twitter, JSON-LD, alternates) merged across layouts by the core and emitted into `<head>`. Generated `sitemap.xml`, `robots.txt`, and web manifest. **OG images** render from a neutral element-tree template through `taffy` layout, `parley` text shaping and `resvg`, with no headless browser, cached by input hash.

### 16.4 Internationalization

Locale-prefixed routing, negotiation from `Accept-Language` and cookies in the native rule layer, localized `href()` generation, ICU4X for plural, date, number and collation, and message catalogs (ICU MessageFormat, with a Fluent option) with typed message keys and per-route catalog splitting. Adapter bindings for each framework's idiom.

### 16.5 Content

Markdown and MDX via `markdown-rs` or `comrak` plus the Oxc MDX pipeline, syntax highlighting at build time (tree-sitter or `syntect`), **typed content collections** (Standard Schema frontmatter validation, typed queries, generated search indexes, reference checking between entries), and live reload in dev.

### 16.6 Other built-ins

Script loading strategies (`<Script>` with defer, idle or worker options), CSS modules and global CSS, SVG components, web-vitals reporting hook, optional PWA/service-worker module, cookie and session primitives (signed and encrypted cookies, key rotation), and an `oxide/test` helper that exercises the full stack in-process with `app.fetch(request)`, no network, plus `oxide/test/browser` using Playwright.

## 17. Security

### 17.1 Defaults (on unless disabled)

- **Actions and server functions:** POST only; `Origin`/`Sec-Fetch-Site` verification; SameSite cookies; optional double-submit token for cross-origin setups.
- **CSP:** strict nonce-based policy generated per response, with `strict-dynamic`; all core-injected scripts carry the nonce. Hash mode for static pages.
- **Headers:** `X-Content-Type-Options`, `Referrer-Policy`, `Permissions-Policy`, `Cross-Origin-Opener-Policy`, HSTS on TLS, frame protections.
- **Request limits:** body size, header size, URL length, multipart part counts, slow-request timeouts, enforced in Rust before JS.
- **SSRF protection:** the image optimizer and the `fetch` op block private, loopback and link-local ranges unless allow-listed, and re-validate after redirects and DNS resolution.
- **Secrets:** server env vars never enter client bundles (compile-time guard in §6.4); source maps are `hidden` by default in production.
- **ODF decoder hardening:** no prototype keys, depth and size limits, bounded reference tables.
- **Host validation:** `Host` allow-list and trusted-proxy configuration; correct `X-Forwarded-*` handling.
- **Rate limiting:** a native rule type (token bucket per key) usable without JS.

### 17.2 Supply chain and sandboxing

The cache verifies content hashes on load; WASM plugins run sandboxed with declared capabilities; JS plugins and the tooling pool run with no network by default in CI mode (`--offline-plugins`); `oxide audit` reports plugin and dependency advisories. A V8 isolate is **not** a security boundary for untrusted tenant code, and the docs state this plainly.

### 17.3 Dev server

Binds to localhost by default, checks `Host`, restricts the WebSocket origin, and refuses file serving outside the project root and allowed workspace roots.

## 18. Deployment adapters

A deploy adapter transforms build output into a deployable artifact. The portable contract is the web-standard handler `(Request) => Promise<Response>` plus the manifests.

| Target | Runtime | Notes |
| --- | --- | --- |
| `binary` (default) | Rust server, V8 isolates | One executable, optional embedded assets and snapshot; best performance |
| `node` | Node server using the `NodeHost` backend | For native addons and Node-only hosting |
| `static` | None | §10.5; plus `_headers`/`_redirects` emitters |
| `docker` | Distroless image around `binary` | Healthcheck, non-root, reproducible layers |
| `cloudflare` | Workers | Edge environment output, core-wasm subset (§2 D18), KV/R2 cache handlers |
| `lambda` / `vercel` / `netlify` | Serverless | Request/Response handler, remote cache handler, snapshot-friendly cold start |
| `deno` / `bun` | Respective servers | Via the handler contract |

A published **capability matrix** states per target which features work (L0 cache, ISR, sockets, image optimization, native rules) and how absent ones degrade. The build refuses a configuration that needs an unavailable feature rather than failing at runtime.

## 19. Developer experience

### 19.1 CLI

`oxide dev`, `build`, `start`, `preview`, `routes` (table with inferred modes and cache policy), `analyze` (bundle and budget report), `check` (types via `tsgo`, lint via `oxlint`, config and route diagnostics), `why <module>`, `doctor` (environment and config health), `add <module>`, `generate` (route, layout, action scaffolds), `cache <inspect|clear|stats>`, `audit`, `migrate <next|nuxt|sveltekit|vite>`.

### 19.2 Starters and migration

`create-oxide` offers React, Vue, Svelte and Solid starters in four shapes: minimal, blog (content collections), dashboard (auth, forms, actions), and storefront (images, search, caching). `oxide migrate` codemods convert file conventions and data APIs from Next.js, Nuxt, SvelteKit and plain Vite apps, reporting what it could not convert.

### 19.3 Editor and tooling

A TypeScript language service plugin and Vue/Svelte language tooling hooks provide route-aware autocompletion for `href()`, loader data and action names; a VS Code extension adds the route explorer, inferred render mode and cache-policy lenses; JSON Schema for config; an `oxide mcp` server exposes routes, errors, cache and graph state so coding agents see accurate project facts.

### 19.4 Error design rules

Every error has a stable code, a source-mapped code frame, the cause, and the fix when one is known (for example, a top-level DB connection that breaks snapshotting, or a server-only import reached from the client with the full import chain). Errors from tooling workers, isolates and Rust share one diagnostic format.

## 20. Observability

- `tracing` spans for every pipeline stage; OpenTelemetry export for traces and metrics; W3C trace context propagated into `fetch` op calls and user code.
- `Server-Timing` on every response (match, cache, load, render, inject), toggleable in production.
- Prometheus metrics: request rate and latency histograms per route, cache hit ratio per layer, isolate pool saturation and recycle count, heap usage, queue wait time, stream backpressure stalls.
- Structured JSON logs with request ids; redaction of configured headers and fields.
- Optional real-user monitoring beacon (web-vitals) posting to a configurable endpoint.
- Dev: per-request flamegraph in the inspector.

## 21. Performance targets and benchmarking

Targets, to be validated in P0 and tracked in CI from M1.

| Metric | Target |
| --- | --- |
| Dev server ready, cached, 1,000-module app | ≤ 300 ms |
| Dev server ready, uncached | ≤ 1.5 s (framework compilers dominate for Svelte/Vue/Solid) |
| HMR, component edit, p95 | ≤ 50 ms server-side, ≤ 100 ms to paint |
| Production build, cold, 1,000-module app | At least 2x faster than the incumbent meta-framework for each framework |
| Production build, incremental one-file change | ≤ 500 ms |
| L0 cache hit | p99 < 1 ms server time; limited by NIC, not CPU |
| Dynamic SSR, small page | At least 2x the incumbent's throughput on identical hardware; TTFB p50 within 5 ms of the framework's raw renderer |
| Cold start with app snapshot | ≤ 50 ms to first byte for the binary target |
| Idle memory, 4 isolates | ≤ 200 MB RSS |
| Client JS, hello page | Framework runtime plus ≤ 4 KB (core router and loader) |
| Client JS, islands page with no islands | 0 KB |

Benchmark method: public repository with reference apps (Hello, Blog, Dashboard, Storefront) per framework; competitors are Next.js 16, Nuxt 4, SvelteKit 2, SolidStart, and Vite for SPA; fixed hardware classes; warm and cold runs; `wrk`/`oha` load generation; Lighthouse CI; CI fails on a regression of more than 5% on tracked metrics. Competitor versions must be re-checked when benchmarks run.

## 22. Testing and conformance

- **Conformance suite (D20):** a reference app (Atlas) with a single assertion set run against all four adapters: SSR output, hydration without mismatch, streaming and deferred data, error and not-found boundaries, client navigation and prefetch, actions with JS on and off, server functions, caching and revalidation, head and metadata, islands, HMR behavior, source maps. A release is blocked if any adapter regresses.
- **Rust:** unit tests, `insta` snapshots for HTML and manifests, `proptest` for the router and graph invalidation, `cargo-fuzz` for the ODF decoder, router, HTTP parsing paths and config extractor, `criterion` for microbenchmarks, Miri and loom on the isolate pool and cache.
- **Integration:** in-process `app.fetch()` tests; cache semantic tests (coalescing, tags, SWR, distributed invalidation); chaos tests (isolate crash, OOM, slow loads, client disconnect mid-stream).
- **End to end:** Playwright across Chromium, Firefox and WebKit; JS-disabled runs for progressive enhancement.
- **Compatibility:** plugin compatibility table verified in CI; Linux, macOS and Windows matrix.

## 23. Workspace layout

```
crates/
  cli/          clap entry, commands
  config/       static extractor, schema, typegen for config
  graph/        demand graph, CAS, remote cache trait
  compiler/     Oxc passes: directives, server-fn stubs, refresh, guards
  bundler/      Rolldown integration, environments, manifests
  jshost/       JsHost trait, V8Host (deno_core), NodeHost, ops, snapshots
  router/       route tree, matcher, typegen, rules engine
  server/       hyper/tower pipeline, static assets, compression, TLS
  render/       streaming pipeline, lol_html injection, boundary protocol
  cache/        L0/L1/L2, tiers, tags, handlers
  odf/          wire format encoder/decoder (also compiled to wasm)
  assets/       lightningcss, images, fonts, og, content
  dev/          watcher, HMR, overlay, inspector
  plugin/       plugin host, JS bridge, WASM host
  adapters/     binary, node, static, docker, cloudflare, serverless
  core-wasm/    edge subset of router, odf, head, cache keys
packages/
  oxide/        server and client runtime APIs, types
  react/ vue/ svelte/ solid/   framework adapters
  create-oxide/ test/ vscode/ ts-plugin/ mcp/
conformance/    Atlas reference app and cross-adapter suite
benchmarks/     reference apps and harness
```

## 24. Roadmap to the MLP

Indicative durations assume a team of roughly 12 to 16 engineers. Total: about 18 to 22 months from start to MLP release.

| Phase | Deliverable | Exit criteria |
| --- | --- | --- |
| **P0 Spikes** (≈8 wk) | V8 render pool plus app snapshot rendering all four frameworks with streaming; Rolldown-as-library with a Rust and a JS plugin; HMR prototype; graph engine prototype with persistence; ODF encoder; JS plugin bridge against the four framework plugins | Written, measured decisions on D2, D6, D11, D15 and the plugin bridge scope; adapter contract v0 |
| **M1 Foundations** (≈14 wk) | CLI, config static extractor, graph engine, Oxc, Rolldown, Lightning CSS, tooling pool, dev server with HMR and overlay, prebundling, **all four adapters in SPA mode**, basic typegen | Dev start and HMR at or ahead of Vite per framework; contract v0.5 |
| **M2 SSR** (≈14 wk) | File router with layouts, boundaries, template; render pool; streaming SSR; core boundary protocol; hydration; core client router with prefetch; head merging; asset injection; Early Hints; static asset server | Conformance v1 passes on all four; contract frozen |
| **M3 Data and server** (≈14 wk) | Loaders, actions with progressive enhancement, server functions, endpoints, WebSocket and SSE, native rules plus JS middleware, env and secrets, cookies and sessions, security defaults, **NodeHost backend** | Auth-plus-CRUD sample works with JS disabled and enabled on all four; security checklist complete |
| **M4 Rendering and cache** (≈12 wk) | Mode inference, static and ISR, L0/L1/L2, tags and revalidation, single-flight, `'use cache'`, deferred streaming, static export, cache handlers (disk, Redis, S3) | Blog with on-demand revalidation; load-test numbers published |
| **M5 Platform** (≈14 wk) | Images, fonts, metadata and OG, i18n, content collections and MDX, Tailwind plugin, islands mode, script and asset features, plugin API v1 | Lighthouse-clean starter per framework |
| **M6 Deploy and hardening** (≈12 wk) | Adapters (binary, node, static, docker, Cloudflare, one serverless), app snapshots in production, persistent and remote build cache, budgets, analyzer, inspector, `oxide migrate`, docs site, starters, benchmark suite vs incumbents, security review | MLP gate (below) then public beta |

### 24.1 MLP release gate

1. Conformance green on all four adapters on Linux, macOS and Windows.
2. The four reference apps build and deploy through at least three adapters.
3. Published benchmarks show the §21 relative targets met or documents each miss with a plan.
4. No open data-loss, cache-poisoning or security issue; external security review completed.
5. Docs cover every public API, every config key, every convention, and migration from Next.js, Nuxt, SvelteKit and Vite.
6. Plugin compatibility table published; at least ten third-party plugins verified.

### 24.2 Post-MLP

M7: partial prerendering, React Server Components capability (D17), parallel and intercepting routes. Then: mixed-framework islands, WASM plugin ecosystem, native Solid and Vue transforms, Leptos/Dioxus adapter, more edge and serverless targets, full-bundle dev as default, visual devtools.

### 24.3 Scope discipline

If scope bites, cut in this order: HTTP/3, non-Cloudflare edge targets, OG image generation, content search indexes, WASM plugins, i18n catalog tooling. **Never cut:** the conformance suite, security defaults, cache correctness, or any of the four adapters.

## 25. Risks

| Risk | Impact | Mitigation |
| --- | --- | --- |
| JS plugin bridge fidelity (Rolldown's JS plugin story assumes Node) | Plugin ecosystem gaps | Scope to used hooks; test against real plugins from P0; Node pool fallback for plugins that need it |
| JS compiler throughput (Svelte, Vue, Solid) limits cold builds | Misses the build-speed pitch for three of four frameworks | Tooling pool plus CAS; measure in P0; prioritize native Solid and Vue ports by measured share |
| Embedded V8 compatibility gaps | Apps fail at runtime | Tiered compat (§4.6), automatic Node backend selection, NodeHost in M3 |
| V8 snapshot restrictions | Slower cold start than targeted | Code-cache fallback; top-level side-effect lint |
| Adapter drift | Feature parity erodes | Conformance gate (D20); contract frozen at M2 |
| Dependency churn (Rolldown, Oxc, `deno_core`) | Rework, API breakage | Each behind a trait; pinned versions; upgrade cadence with CI |
| Scope | Late or shallow MLP | Phase gates, §24.3 cut order |
| Cache correctness bugs | Stale or leaked content | Semantic tests, Vary rules by construction, fuzzing, no cross-user caching without explicit keys |
| Windows support | Late surprises | Windows in CI from M1 |

## 26. Open questions

1. **Name:** Oxide collides with an existing hardware company's brand; choose a final name and package scope before 0.1.
2. **Snapshot scope:** can full app snapshots coexist with common ORMs and SDKs, or does code cache become the default?
3. **WASM plugin interface:** does a WIT interface reach transform throughput near native plugins?
4. **Svelte streaming:** whether current Svelte versions offer native async SSR streaming that lets Svelte move from `suspense: 'core'` to `native`.
5. **React Compiler native path:** status of a native Rust implementation versus the Babel plugin.
6. **Rolldown plugin API in embedded hosts:** whether a stable non-Node plugin host API exists, or whether the bridge needs a custom adapter.
7. **Environment API stability:** how closely to track Vite's Environment API as it evolves.
8. **Licensing and governance:** license choice (MIT or Apache-2.0 dual), contribution model, and adapter ownership for community frameworks.

## Appendix A. Dependency map

| Area | Choice | Isolation |
| --- | --- | --- |
| Async, HTTP, TLS | `tokio`, `hyper`, `tower`, `rustls`, `quinn`/`h3` (opt-in) | Stable, direct use |
| JS parse, transform, resolve | `oxc`, `oxc_resolver` | Behind `compiler` crate |
| Bundling | `rolldown` | Behind `Bundler` trait |
| CSS | `lightningcss` | Behind `assets` crate |
| JS host | `deno_core`, `rusty_v8`, `deno_node` subset | Behind `JsHost` trait |
| HTML rewrite | `lol_html` | Behind `render` crate |
| Cache, storage, hashing | `moka`, `redb`, `rkyv`, `blake3`, `zstd` | Behind `CacheHandler` and CAS traits |
| Watching | `notify` | Behind `graph` crate |
| Images, fonts, OG | `image`, `fast_image_resize`, `ravif`, `webp`, `oxipng`, `fontations`/`skrifa`, `taffy`, `parley`, `resvg` | Behind backend traits |
| i18n, content | `icu` (ICU4X), `markdown-rs`/`comrak`, `syntect` | Behind platform plugins |
| Observability, errors | `tracing`, `opentelemetry`, `miette`, `thiserror` | Direct |
| CLI, config | `clap`, `serde`, `figment` | Direct |
| WASM plugin host | `wasmtime` (component model) | Behind `plugin` crate |

Versions, maturity and APIs of Rolldown, Oxc, `deno_core`, `h3` and the Environment API change quickly; each entry must be re-verified at the start of P0.
