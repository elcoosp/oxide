# Oxide v0.2 — Feature Designs: All 20
## Concrete implementation designs grounded in the `4633124` tree

*Date: 2026-10-16 · Ground truth: repo @ `4633124` (scorecard: overall v0.1 compliance ~70%), five code inventories + two verification agents (all in `worklog.md`), seam spot-checks run today (worker op table has 14 dispatch kinds incl. `plugin:load/call`; `libloading` and `ring` already in `Cargo.lock`; no SSE exists yet; `encodePropsB64` ODF-props path live in all four adapters; `tantivy` not yet a dependency).*
*Every design names: what it builds on (verified seams), where new code lands (crate/file level), the wire/protocol shapes where relevant, spec impact, an acceptance test, and honest risks. Effort tags are the owner's (S = weeks, M = a milestone, L = research risk); "Verify" lines record where my reading of the code agrees or pushes back.*

---

## 0. The shared substrate (read this first)

Six of the twenty designs (#1, #3, #5, #6, #13, #14) sit on top of the **user-visible live-path bucket** that 138 commits did not touch: router boot, nested layouts, real hydration for Vue/Svelte/Solid, real streaming SSR, defer values, and `route.ts`/middleware dispatch. **That bucket is Wave 0 for this plan.** Nothing in this document re-designs it; the designs below assume it landed and build on its artifacts:

- **Boot runtime** (`packages/oxide/src/client/boot.ts`, generated `island-boot.mjs` at `crates/cli/src/lib.rs:642-693`) — will gain router boot, HMR connect, and the adapter-registry refactor described in #1.
- **Island protocol** — `<ox-island data-ox-island data-ox-chunk data-ox-strategy data-ox-props>` byte-locked between `crates/render/src/lib.rs:303-331` and `protocol.ts islandElement()`; props ride ODF via `encodePropsB64` (PERF-01, byte-identity proven).
- **ODF** — 17 tags, 16 golden fixtures, REF tables, DEFERRED tag, hardened decoder, JS encoder now byte-identical. Features #5, #6, #8, #15 all encode new payloads through it rather than inventing wire formats.
- **Endpoint conventions** — de facto already: `/__oxide/*` = framework data plane (`/__data`, inspector, stream ids), `/_oxide/*` = ops plane (vitals, image, fn, hmr, metrics). New endpoints in #5/#7/#9/#15 follow it.
- **Worker op table** (`oxide-worker.mjs:1189-1206`) — 14 kinds with `id`-correlated call frames; V8 parity via `v8_dispatch` (`jshost/lib.rs:1748-1958`). New ops in #8/#15 add one dispatch arm each on both hosts.
- **Build id** — introduced in #10; #11, #13, #14 key off it. Designed once, reused four times.

---

## 1. Mixed-framework islands (M)

**Goal:** React, Vue, Svelte and Solid islands on one page, each hydrated by its own adapter, proving the core is framework-neutral.

**Verify:** feasible now at protocol level — the island protocol is byte-identical across adapters (`protocol.ts`/`ir.ts` diff exit = 0), props cross the wire framework-neutrally via ODF, and the scheduler is core-owned. The two real blockers are (a) only React hydrates for real, (b) the boot entry statically imports every island into one chunk and assumes a single `clientAdapter`.

**Builds on:** island protocol (byte-locked), ODF props, island scheduler strategies (`islands.ts:45-204`), rolldown chunking.

**Design:**
1. **Manifest change.** The islands registry (config `islands: {Counter: 'src/islands/counter.tsx'}`) gains framework inference at scan time — from file extension (`.svelte`, `.vue`) and import analysis (`.tsx` disambiguated by the precompile pool's transform detection already keyed per framework). Build emits a boot manifest instead of hardcoded imports:
   ```jsonc
   // .oxide/islands.json (new build artifact)
   { "Counter": { "chunk": "islands/counter.abc123.js", "framework": "react" },
     "Price":    { "chunk": "islands/price.def456.js", "framework": "vue" } }
   ```
2. **Adapter registry.** `installBootRuntime()` accepts `adapters: Record<Framework, Adapter>`; the generated boot imports only the client adapters for frameworks actually present (tree-shaken). `islands.ts` picks `adapters[entry.framework]` per island and calls its `hydrateIsland` — the existing per-island dispatch point (`islands.ts` hydrate loop) is where the current single-adapter assumption lives.
3. **Real hydration prerequisite.** Vue/Svelte/Solid `hydrateIsland` become real (packages/{vue,svelte,solid}: replace `clientAdapter = adapter` IR replay with `createSSRApp().mount` / `hydrate()` / `hydrate()` from `solid-js/web`). This is Wave 0; #1 is its consumer, not its owner.
4. **Chunking.** Per-island dynamic import (replacing the static boot import — this also fixes the bundle-diet item) with per-framework shared chunks via rolldown `advancedChunks`: `react-vendor`, `vue-vendor`, `svelte-vendor`, `solid-vendor` loaded once on first island of that framework. The renderer emits `data-ox-fw` on `ox-island` for devtools/debuggability.
5. **Conformance.** New `examples/mixed-islands` fixture (4 islands, one per framework, one shared ODF prop shape) + a conformance assertion: all four hydrate and respond to interaction in one document; Playwright spec mirrors it against the real build.

**Spec impact:** §7 — promote "mixed-framework islands" from the post-MLP sketch into the adapter contract section (the owner already flagged this spec change).

**Acceptance:** Playwright: click each of the four islands on one page → each increments its own state; document contains no IR-replay marker; route JS budget reports four vendor chunks loaded on demand.

**Risks:** four runtimes on one page = JS weight (mitigate: strategy-gated download is already core-owned; document the cost in the capability matrix). React 19 + Vue + Solid concurrent-mode interactions on one document are uncharted — the mixed fixture is specifically designed to flush this out early.

---

## 2. Auto-islands inference (L — spike first)

**Goal:** the compiler detects components with no client-side behavior on full-hydration routes and drops them from the client bundle — Astro-style savings without rewriting the app.

**Builds on:** the existing Oxc AST render-mode inference engine (`crates/compiler/src/lib.rs:931-1311` — it already walks route modules for dynamicity; this adds a second pass over component exports).

**Design (spike scope, one week, kill criteria at the end):**
1. **Interactivity scan.** For each component exported from route/island files, score "client behavior" per framework: React (`useState/useEffect/useReducer/useContext-write/any on* prop defined`), Vue (`ref/reactive/computed/watch/onMounted/emit`), Svelte (`$: assignments, on:*, bind:, stores`), Solid (`createSignal/createEffect/createStore/on*`). Pure = no hits and no children of hit components.
2. **Manifest.** Emit `.oxide/island-manifest.json` `{file → {interactive, reasons[]}}`. Non-interactive components: SSR render unchanged (they already render server-side); the boot manifest simply omits them from the hydration set, and the bundler tree-shakes them from client chunks (the boot-manifest filter from #1 is the mechanism).
3. **Safety rails.** Explicit `islands` config registration always wins (opt-in beats inference). `experiments.autoIslands: true` gates the whole feature. Dev-only warning when a component marked pure receives props matching `on*` (the most common false-negative shape).
4. **Kill criteria (spike).** If the scan's false-negative rate on the four example apps + conformance fixtures is > 0 after the `on*`-props guard, or the savings on the examples are < 15% of route JS, file the findings and defer to v0.3 — a wrong inference here silently breaks interactivity, which is worse than shipping bigger bundles.

**Acceptance (if spike passes):** fixture component loses its `useState` → disappears from client chunk (byte budget proves it); re-adding state restores it; `oxide why <route>` shows the inference decision.

**Risks:** L-rated honestly. Deep props-flow analysis (handlers passed through three wrappers) is a research rabbit hole — the design deliberately scopes to *direct* detection only.

---

## 3. Interaction replay (S)

**Goal:** clicks, inputs and focus that land between SSR paint and hydration completion are captured and replayed — the "dead button" window disappears, for all four frameworks, without framework code.

**Builds on:** island scheduler lifecycle (`data-ox-hydrated` pending/done/error guard in `islands.ts`), boot runtime, the `ox-island` wrapper element as the region marker.

**Design:**
1. **Capture** (`packages/oxide/src/client/replay.ts`, ~300 lines, core-owned like `scripts.ts`): a document-level capture-phase listener set (`click`, `keydown` Enter, `input`/`change`, `focusin`) active from boot until hydration settles. Events whose composed path intersects an `ox-island` **without** `data-ox-hydrated="done"` are recorded: `{type, path: nth-child chain from island root, value?, ts}` — bounded ring (100 events, drop oldest).
2. **Replay:** the scheduler's existing hydrate-done transition emits a hook; replay dispatches synchronously before the next macrotask: `click` → `element.click()` (a real, trusted-path dispatch against the now-attached listeners); `input/change` → set `.value` then dispatch the event; `focus` → `.focus()`.
3. **No-double-fire invariant:** capture sets a `replayed` marker; if the island actually was hydrated between dispatch and handler-run (race), the marker suppresses the recorded copy. Events targeting the defer-fallback (`template[data-ox-b]` content) are dropped — the element they targeted no longer exists post-swap.
4. **Wiring:** enabled by default for `visible`/`idle`/`media` islands (where the window is real); `load` islands hydrate fast enough that the window is negligible but the cost is ~1KB, so it ships on uniformly. Config opt-out: `interactionReplay: false`.

**Acceptance:** Playwright: block the island chunk request via route interception, click the button in the SSR'd island, release the block, assert the handler ran exactly once. Repeat for a text input (value + input event) and focus.

**Risks:** minimal; the only subtlety is synthetic-vs-trusted event semantics — `element.click()` and real `dispatchEvent` both work for framework synthetic event systems (React attaches at root; Vue/Svelte/Solid attach at elements). Verify in the mixed-islands fixture so all four frameworks prove it at once.

---

## 4. Cache-safe personalization (M)

**Goal:** experiments/flags evaluated in the native layer become L0 cache variant keys — A/B tests without forcing routes `dynamic`.

**Builds on:** the request pipeline order (static → rules → middleware → dispatch, SEC-11), L0 `key_for` static helper (`l0_cache.rs`), the rules engine's slot in the pipeline, `locals` threading (designed here; also required by the middleware work — do them together).

**Design:**
1. **Config:** `experiments: [{ id, traffic: 50, stickiness: 'cookie' | 'header' | 'ip-hash', cookieName?, headerName?, variants: ['control','treatment'] }]`.
2. **Resolver (Rust, pipeline stage):** after rules, before L0 lookup: deterministic assignment = `blake3(experiment_id || sticky_value) % 100 < traffic` then bucket → variant. Sets an internal `x-oxide-exp: expA=1;expB=0` header on the request and threads variants into `locals.experiments` (the locals plumbing introduced for middleware carries it the rest of the way: loaders, actions, render input).
3. **L0 variant keys:** `key_for` gains an optional variant suffix, included **only** for routes whose experiment config touches them (keys stay byte-stable for everyone else). Tag invalidation is variant-independent — `revalidate('cart')` busts all variants (per-variant tags opt-in via `tag:'expA'`).
4. **Serving:** each variant's HTML is its own L0 entry — a loader-driven page stays ISR-cached per variant. Personalization inside an otherwise-static page that *shouldn't* fragment the cache belongs in a server island (#1's protocol + the L0 nonce-splice machinery) — the docs draw that line explicitly.
5. **Observability:** Prometheus counter `oxide_experiment_assignment{experiment,variant}`; `oxide why <route>` shows variant dimensions in the cache key breakdown.

**Acceptance:** two identities (different cookie values) in different buckets get different cached HTML; L0 hit-rate metric unchanged vs no-experiments baseline; removing the config returns keys to the pre-experiment byte form.

**Risks:** low. Cache-size multiplication (variants × routes) is bounded by `cache.l0MaxSize` and documented.

---

## 5. Live loaders (M)

**Goal:** `liveLoad` re-runs a loader when a tag is revalidated and pushes the fresh ODF payload to subscribed clients — dashboards with zero hand-written WebSocket code.

**Builds on:** tags + `revalidate` (live since BUG-01/02), ODF encoding of loader payloads (`encodePropsB64` / `Op::Serialize`), the `/__data` GET path, single-flight, the worker pool's route affinity (PERF-02), Prometheus.

**Design:**
1. **API:** `export const loader = liveLoad(handler, { tags: ['posts'] })` — typed sugar over `defineLoad` with `live: true` (typegen's LoaderData path applies unchanged).
2. **Transport: SSE, not WebSocket.** No SSE exists in the tree yet; it's ~60 lines on the existing hyper response path (headers `text/event-stream`, heartbeat 25s) and needs no auth handshake beyond the existing origin checks. `GET /__oxide/live?route=<id>&params=<enc>` — the existing RFC6455 codec stays for HMR; SSE is deliberately the boring choice for one-way data.
3. **Server loop:** on connect — run loader now (single-flight per route+params), emit `event: data` with base64 ODF; register the subscription in an in-process tag bus (`HashMap<tag, Vec<Subscriber>>` behind `Arc<Mutex>` — the local sibling of SPEC-12's `HttpCacheHandler` pub/sub, which becomes the distributed backend via the same trait). `revalidate(tag)` → bus publish → per subscriber: re-run via pool → ODF-encode → push. Caps: 100 subscribers/route (config), 5-min idle TTL, bounded payload size.
4. **Client** (`packages/oxide/src/client/live.ts`, core-owned): `EventSource`, ODF decode (decoder already handles the magic), dispatch `ox:live {route, params, data}`; binding helper `bindLive(name)` re-renders the subscribed route's dataCache entry and notifies islands via the existing adapter `update()` contract. Reconnect with `Last-Event-ID` (seq) → server resumes with a delta if #6 is enabled, else full.
5. **Sequencing note:** lands on the defer-glue value-delivery fix (Wave 0) — both are "server has a value, client awaits a promise" shapes, and the client glue is already written for that contract.

**Acceptance:** dashboard example: action → `revalidate('stats')` → second tab updates < 200 ms; gauge `oxide_live_subscribers`; disconnect/reconnect does not double-push (seq dedupe).

**Risks:** subscriber re-run storms on hot tags — single-flight per (route, params) at the bus level, plus max-pushes/second per subscriber (token bucket, the primitive exists in the rules engine).

---

## 6. Structural ODF deltas (M)

**Goal:** on revalidation, send a patch against the client's last version instead of the full payload — the win case is big lists/tables.

**Builds on:** ODF (its REF tables already model shared structure — the patch format is the natural extension), the #5 live channel and `/__data` GET, the golden-fixture testing culture.

**Design:**
1. **Envelope:** new ODF container tag `PATCH` (next free tag in `wire.rs`'s 17-tag table): header `{base_seq: u32, base_hash: [u8;16]}` + op stream, where ops are themselves ODF values: `SetPath(path: String, value)`, `DelPath(path)`, `ArrSplice(index, delete_count, items: Array)`, `MapSet(key, value)`. Paths use the existing canonical key ordering (object keys are already sorted — the encoder guarantees a canonical tree, which makes diffing well-defined).
2. **Rust diff** (`crates/odf/src/diff.rs`): structural diff of two decoded Values against the canonical ordering; list edits diff by common prefix/suffix then LCS on the remainder (bounded: bail to `SetPath` of the whole array when edit distance is trivially large).
3. **Adaptive fallback:** if `patch_bytes > 0.5 × full_bytes` or the client's `base_hash` mismatches → send full. The server keeps the last-sent payload per subscriber (#5's bounded per-subscriber state); `/__data` gains `if-odf-seq`/`if-odf-hash` request headers for the non-live revalidation path.
4. **Client apply** (`odf-decode.ts` gains `applyPatch(prev, bytes)` — reuses the existing decoder for op values; the REF table interacts only with op values, never the base, keeping the decoder's hardening intact).
5. **Fixtures:** 4 golden patch fixtures added to the existing 16 (nested object edit, array splice, map set, fallback-to-full) — byte-exact both directions, matching the project's testing culture.

**Acceptance:** 10k-row list, mutate 1 row → wire bytes < 2% of full; wrong `base_hash` → clean full fallback; both encoder and decoder round-trip the golden bytes.

**Risks:** low-medium; the only protocol design on this list. The canonical-ordering guarantee is what makes it tractable — it already exists.

---

## 7. Resumable streams (M)

**Goal:** Rust buffers in-flight streams so a disconnected client can reattach by id and continue — no duplicate or missing bytes. The LLM-streaming pain point.

**Builds on:** the streaming render path (`render_stream_with_recovery`, SPEC-11 — recovery boundaries already exist), binary chunk frames (PERF-12), the L0 tee-fill pattern (streaming tee into a store is solved there), hyper response handling.

**Design:**
1. **Stream identity:** every streamed response (defer shell, live loader, streaming server-fn) carries `x-oxide-stream-id` (16-byte blake3-random hex) and `x-oxide-stream-offset: 0`.
2. **StreamStore** (CLI request path, where the render stream already passes): per stream `{chunks: Vec<Bytes>, done: bool}` — LRU 64 streams (config), 8 MiB/stream cap, 5-min TTL after `done`. The tee is the same shape as L0's streaming fill (`cli/lib.rs:3719-3794`), so the pattern and its backpressure handling are proven.
3. **Resume:** `GET /__oxide/stream/:id?offset=N` — replays buffered chunks from byte N; if the stream is still open, attaches live (pipe) after replay; if N is behind the buffer's pruned start → `410` + `x-oxide-stream-expired: true` (client falls back to a fresh request — correctness over cleverness).
4. **Client glue:** the defer-glue and live-client fetch wrappers gain one retry wrapper: on network error mid-stream, exponential backoff (250ms × 4), resume with `offset = bytesConsumed`. Page navigation aborts and releases the subscription.
5. **Ops integration:** server-fn streaming responses register the same way — the op dispatch layer tags them, which is what makes LLM-style token streams resumable without user code.

**Acceptance:** integration test: abort mid-stream at byte K, reconnect with `offset=K`, output equals the never-interrupted stream byte-for-byte (checksum); TTL expiry → 410; 65th stream evicts the oldest (LRU proof).

**Risks:** memory pressure is the only real one — caps + LRU + the existing hardening limits culture cover it; brownout (#12) can shrink the store under load (noted there).

---

## 8. Native Rust server functions (M)

**Goal:** `*.server.rs` files become typed ops callable from TypeScript with generated bindings — the escape hatch (image work, crypto, search) no JS-first framework can offer.

**Builds on:** the server-fn dispatch route (`POST /_oxide/fn/<id>` with CSRF — §9.4 is live), ODF both directions (rich types cross the boundary: `Date`, `Map`, typed arrays), `libloading` (already in `Cargo.lock`), the macros/codetooling pattern from typegen, CAS for the cargo target dir.

**Design:**
1. **Authoring:** optional `server/` crate in the app workspace:
   ```rust
   // server/src/hash.server.rs
   #[oxide::server_fn]
   pub fn hash_password(pw: String) -> Result<String, FnError> { ... }
   ```
   The `oxide-macros` crate (new, small) generates: a JSON schema of params/returns (from `syn` types), and registration via `linkme` (no init function to forget).
2. **Two build models.** Model A (default, dev + self-hosted): `oxide build` cargo-builds `app-server` as a **cdylib**, loaded at serve start via `libloading`; dev rebuilds swap the dylib when the watcher sees `server/**` change (pool restart — the pool is already restart-friendly). Model B (static/edge targets): feature-gated fat-binary recompile — declared in the capability matrix as `rustFns: Full | Unsupported` (per the adapters' honest-degradation pattern).
3. **Wire:** `POST /_oxide/rust/<fn-name>` through the existing dispatcher arm pattern (one new dispatch branch next to the server-fn branch, `cli/lib.rs` dispatch closure): body ODF or JSON, result ODF-encoded. CSRF + rate-limit rules apply identically — it *is* a server function, not a side door.
4. **Types:** build generates `server-rust.d.ts` from the macro schema; `fn.ts` gains `rustFn('hash_password')` returning a typed caller. LoaderData-style inference applies if a loader returns one.
5. **Capabilities hook:** Rust fns consult the same per-route capability allow-lists as #19 (net/env) — enforced at the dispatcher for JS-host fns, at compile-time features + runtime config for Rust fns.

**Acceptance:** example with an argon2/blake3 `hash_password` fn: called from a form action, types flow into TS (`rustFn` is fully typed), works on both `node` and `v8` backends (Rust fns bypass the JS hosts entirely — dispatcher-level, identical on both — which is why this design is simpler than it looks).

**Risks:** toolchain presence detection + cargo cold-build cost (mitigate: CAS the `target/` dir — the precompile CAS pattern applies verbatim); dylib ABI is same-session-only (documented; model B exists for frozen deploys). This is the largest *new-surface* feature on the list; the dispatcher seam it plugs into is genuinely small.

---

## 9. Built-in search (M)

**Goal:** `tantivy` indexes content collections at build time and serves queries natively; static exports keep a client-mode fallback.

**Builds on:** content collections (scan/validate/reference-check already real; search-index JSON is the current output), the dispatch-endpoint pattern, the capability matrix for the static-mode boundary.

**Design:**
1. **New crate `oxide-search`** (feature-gated — tantivy is a heavyweight dep and must not tax compile times for non-users): schema from the collection definition — `search: { collections: ['posts'], fields: { title: 'text', body: 'text', tags: 'facet' } }` in config; `SchemaKind::Unchecked` collections are indexed with a default schema (title/body) and a warning.
2. **Build:** collections are already scanned and validated; indexing runs after validation into `.oxide/search/<collection>/` (tantivy segments) + schema JSON. Index build time appears in the build report (the budgets culture).
3. **Serve:** `GET /__oxide/search?q=&collection=&limit=` — native query, top-k with snippets, JSON response (ODF optional later). Read-only GET: no CSRF; rate-limit rules and host allow-list apply like any route.
4. **Static mode (honest boundary):** static exports keep the current search-index JSON path (upgraded to include term frequencies if needed); native tantivy is the server mode. The capability matrix states `search: Full | Static(sharded-json)` — the same honesty the adapters already practice, rather than a WASM-tantivy moonshot in v0.2.

**Acceptance:** docs example, 10k docs: query p99 < 20 ms (bench harness), snippet correctness asserted; budget shows index build cost; a static export serves the JSON fallback.

**Risks:** low. The only design decision is the static/server boundary, and the capability matrix exists precisely for that call.

---

## 10. Skew protection with atomic deploys (M)

**Goal:** the server keeps the previous build's server functions and data endpoints alive so old tabs keep working after a deploy; rollback is instant.

**Builds on:** byte-stable builds (`.oxide/secret`, BUG-05), content-hash chunk filenames (old client chunks remain valid forever), the worker pool's route affinity (PERF-02 — generations are a natural extension), the `dist/server/ssr/index.js` per-generation layout.

**Design:**
1. **Build id:** `OXIDE_BUILD_ID = blake3(dist manifest digests)` written to `.oxide/build-id`, embedded in the boot script, and sent by every data-plane request (`/__data`, `/_oxide/fn/*`, `/__oxide/live`) as `x-oxide-build`.
2. **Two generations on serve:** `oxide start` retains the previous generation's `dist/server` module alongside the current one; the pool hosts both, tagged by build id. Dispatch: request's `x-oxide-build` selects the generation for **data-plane ops only** — HTML always renders from current. Unknown/expired build id → `409` + `x-oxide-build-stale: true` (client boot reloads once, then the error boundary if it loops).
3. **Window:** `deploy.skewWindow: '15m'` default; a background timer drops the old generation when expired (and on pool pressure — brownout #12 may shrink it).
4. **Rollback:** previous `dist/` is retained (`.oxide/generations/<id>/` symlink farm): `oxide start --generation previous` or a symlink swap for process managers — instant, no rebuild.
5. **Adapters:** vercel/netlify/cf outputs plumb the build id as env; static chunks need nothing (content-hash names). Capability matrix gains `skew: Full | Window | Unsupported` per target.

**Spec impact:** §12 gains "build generations and skew" (the owner flagged this one) — it changes what "serving a build" means: serving a *pair* of builds with a policy.

**Acceptance:** e2e: deploy v2 while a v1 tab is open → old tab's server-fn hits the v1 pool and succeeds; new tab gets v2 HTML; after the window, the old tab gets 409 and recovers via single reload.

**Risks:** double server-module memory (bounded: two generations max, documented); worker-pool generation tagging is the only nontrivial plumbing and PERF-02's affinity map is the right home for it.

---

## 11. Delta deploys (M)

**Goal:** upload only chunks whose content hash changed, using the CAS already everywhere in the build.

**Builds on:** dist manifest with per-chunk blake3 digests (content-hash filenames are already the bundler's output), the CAS/redb mental model, the SPEC-04 `HttpRemoteCache` driver (an internal content-addressed transfer client that can be reused for self-hosted targets), the adapter trait + capability matrix.

**Design:**
1. **Adapter trait extension:** `upload_delta(manifest: &DistManifest, remote: &dyn RemoteState)` — `RemoteState` is the adapter's fetch of the last-deployed manifest (each target implements `last_manifest()` against its own blob API: S3 `GET manifest.json`, CF KV, Netlify, or a self-hosted blob endpoint backed by `HttpRemoteCache`).
2. **Diff + PUT:** for each artifact in the new manifest, `PUT` only when `remote.hashes` lacks the digest; unchanged → 0 bytes on the wire. Delete orphans only when the target supports atomic-ish swaps (CF pages/vercel handle this platform-side; the binary/docker targets are untouched by definition).
3. **CLI:** `oxide deploy --delta` per target; the capability matrix gains a `delta` row (yes/partial/no) with the same build-time refusal honesty used for the 7-capability matrix today.
4. **Manifest pinning:** the remote manifest write is the last step (single round of PUTs → then swap manifest) so a failed deploy leaves the old manifest authoritative — atomicity comes from the manifest swap, not from per-blob transactions.

**Acceptance:** integration test against a local blob server: second deploy after a one-component change uploads exactly the changed chunk bytes (assert PUT count and byte totals); third deploy with zero changes uploads nothing.

**Risks:** minimal; the only care point is manifest-swap ordering, specified above.

---

## 12. Brownout mode (M)

**Goal:** when queue depth or latency SLOs trip, the server degrades on purpose: serve stale, shed low-priority routes, hedge slow renders — instead of falling over.

**Builds on:** pool gauges already in Prometheus (PERF-02: `oxide_pool_workers/_in_flight/_queue_wait_p99_us`), L0 stale-serve (`Hit::Stale` machinery), the token-bucket primitive (rules engine), per-stage Server-Timing (PERF-06).

**Design:**
1. **Config:** `server.brownout: { queueDepth: 32, latencyP99Ms: 800, minDwell: '30s', actions: ['stale','shed','hedge'] }` (all optional, feature off unless configured).
2. **Controller:** a tokio task in the server crate samples the gauges at 1 Hz with hysteresis: enter when queue_depth ≥ N or p99 ≥ M sustained 3 samples; exit at 0.5× threshold after `minDwell`. State is an `Arc<AtomicU8>` on the shared pipeline state — the dispatch path reads it with one atomic load (no lock on the hot path).
3. **Actions:** (a) **stale** — L0 lookup gains `allow_stale_over_ttl` when brownout is active: expired entries serve with `Warning: 110 - stale-while-brownout` instead of triggering a worker render; (b) **shed** — routes with `priority: 'low'` (route config/frontmatter) get `503 + Retry-After: 10` at dispatch entry, before pool submission; (c) **hedge** — a render exceeding the hedge threshold (p95 latency from the Server-Timing histogram) is duplicated onto a second pool worker, first response wins, loser cancelled (pool calls are already id-correlated and cancelable — the nav-race guard pattern applies).
4. **Observability:** `oxide_brownout_active` gauge + `oxide_brownout_action_total{action}` counters; the JSON logs' stage spans (SPEC-24) gain a `brownout: true` field so postmortems are greppable.

**Acceptance:** load test with an artificially slow loader: brownout enters within ~3 samples, stale HTML served for tagged routes, low-priority route sheds with 503, queue drains, exits after dwell; gauges prove each action fired.

**Risks:** low — every mechanism it composes already exists and is individually tested; the new code is the controller and the three dispatch hooks.

---

## 13. Learned prefetch (M)

**Goal:** a privacy-preserving per-route navigation graph drives idle prefetch and Speculation Rules output — beyond hover-only.

**Builds on:** the client router's existing budget-5 cancelable prefetch (hover/focusin/viewport), the history-state navigation record, the beacon pattern (`/_oxide/vitals`), Prometheus counters, the build id (#10).

**Design:**
1. **Client recording:** router `goto` appends `{from, to}` to a local buffer; every 10 transitions (or pagehide) sends one aggregated `POST /_oxide/nav` — route ids only, no URLs/params/timing; opt-in `prefetch.learn: true`; rate-limited like vitals. Nothing leaves the device until the buffer aggregates.
2. **Server aggregation:** counter pairs `oxide_nav_transition_total{from,to}`; a snapshotter exports the top-k edges per source route into a cacheable JSON, `/_oxide/nav-graph.json` (max-age 1h, keyed by build id). ODF not needed — this is ops-plane JSON.
3. **Prefetch consumption:** boot fetches the nav graph once; the router's idle scheduler prefetches top-2 predicted transitions from the current route (the existing budget system absorbs them — predictions compete with hovers for the same budget-5, predictions yield to hovers).
4. **Speculation Rules:** server-side emission of `<script type="speculationrules">` from the same graph, `eagerness: 'conservative'`, per-config `prefetch.speculation: 'conservative' | 'off'`. Emitted only for routes the graph has seen, so first deploys behave exactly like today.

**Acceptance:** simulation: 30 navigations create graph edges; a fresh session sitting on `/blog` idle-prefetches the predicted post (network-level assert); beacon volume ≤ 1 request per 10 transitions; speculationrules absent when the graph is empty.

**Risks:** low. Cold-start behavior (empty graph) is the honest failure mode and it degrades to today's hover-only.

---

## 14. Offline-first module (M)

**Goal:** route data cached in IndexedDB; actions queue offline and replay on reconnect.

**Builds on:** the PWA service worker emission (SPEC-21: `dist/sw.js` + `pwa-register.js`), `enhance()` (optimistic hook + rollback + result events), ODF decoding on the client (boot already decodes payloads), the build id (#10 keys the cache), L1's idempotency-shaped storage.

**Design:**
1. **Data cache:** sw.js gains a runtime strategy for `GET /__data*`: serve-from-IndexedDB-then-revalidate (store ~50 entries LRU, keyed `route|params|buildId` — the build id makes stale generations self-invalidate). A ~1KB inline IDB helper in `packages/oxide/src/client/offline.ts` (no dependency).
2. **Action queue:** `enhance()` already intercepts submits; on fetch failure of type offline (`TypeError`/`navigator.onLine === false`), enqueue `{seq, url, method, body, csrf, idempotencyKey}` to IDB (cap 50, config). Replay triggers: `online` event + `sync` (where supported) + boot. Replay order = seq; per-item retry ×3 with backoff; `ox:form-result` fires per replayed item so UI rolls forward.
3. **Server idempotency:** actions accept `x-oxide-idempotency`; the dispatcher stores `{key → result}` in the L1 tier (24h TTL) — duplicate replays return the stored result instead of re-executing. This is one branch in the action dispatch path and a config default (`actions.idempotencyTtl: '24h'`).
4. **Offline navigation:** boot's existing ODF decode path serves IDB-cached data for a route the SW has seen, with a UI-visible `ox-offline` attribute so apps can badge it honestly.

**Acceptance:** Chromium offline e2e: navigate two cached routes offline (render from IDB), submit a form (queued, `ox-offline` visible), reconnect → action applied once (idempotency proves replay dedupe), loader data refreshes; queue survives a full reload.

**Risks:** medium-low. The SW/IDB surface is standard; the care points are CSRF-on-replay (token rotation while offline — replay fetches a fresh token first, which is why replay is online-only) and queue ordering under partial failure.

---

## 15. Record and replay (M)

**Goal:** `oxide replay <request-id>` re-runs a captured request against the same inputs and frozen upstreams — deterministic bug reproduction, agent-friendly by construction.

**Builds on:** the worker `fetch` shim + `ssrf_safe_fetch` (PERF-05 — the interception point already exists), CAS (bodies addressed by digest), persisted diagnostics (BUG-06 — erroring requests already leave a trail), trace_context (W3C id = the recording id), JSON logs (SPEC-24), MCP.

**Design:**
1. **Capture** (`record: true` config; dev defaults on): per request — always on error, on `?_oxide_record=1` otherwise. Payload `.oxide/recordings/<trace-id>.json` (ring buffer, 200): `{route, params, headers-subset (cookies hashed), loader inputs, parent chain, action payload, upstreams: [{url, method, status, reqBodyHash, respRef: CAS address}]}`. Upstream bodies go into the CAS — they're content-addressed already, so freezing them is an address, not a copy.
2. **Replay:** `oxide replay <id>` boots the pool with a `replayFetch` table (one new worker-op field, not a new op: the fetch shim consults the recording before the network); runs the loader/action/render, diffs the outputs against the recording, exit 1 on mismatch. `--json` emits the structured diff (agents); MCP tool `oxide_replay(id)` wraps it (the MCP server gains its first write-adjacent tool — still sandboxed: replay only).
3. **Honest determinism boundary:** replay freezes upstream responses and inputs; wall-clock inside user code is still real (recorded `Date` outputs are *expected* values, and the diff flags exactly that class of mismatch instead of hiding it).
4. **Volume control:** recordings are capped and auto-evicted; `record.errorsOnly` mode for CI.

**Acceptance:** fixture with a flaky upstream: record once, replay twice → byte-identical outputs; introduce a regression in the loader → replay exits 1 with a pointed diff; MCP round-trip returns the same diff.

**Risks:** low. The one design rule to hold: recordings capture **inputs and frozen upstreams**, never the framework's own outputs as truth — replay re-derives those.

---

## 16. "Why dynamic?" with quick fixes (S)

**Goal:** the §10.1 diagnostic grows a reason, a location, and a codemod that un-dynamics the route.

**Builds on:** the Oxc AST dynamicity inference engine (`compiler/lib.rs:931-1311` — it already finds the offending reads; it just doesn't say why out loud), routes manifest (rkyv archive + JSON side-car), `oxide why`, the tooling pool (the codemod execution substrate), SPEC-09's `searchSchema` (the remaining typegen piece completes this loop).

**Design:**
1. **Reason capture:** the inference pass records `DynamicReason { kind: CookieRead | HeaderRead | SearchParamRead | UncachedFetch | DynamicImport, span, snippet }` per route into the manifest. This is instrumentation of an existing pass — the walk already visits these nodes to set the dynamic bit.
2. **Surfaces:** `oxide why <route>` prints reasons with code frames (the diagnostics model already renders `code_frame`); the dev inspector route table shows a "why" column; diagnostics code `OX-DYN` persisted for MCP.
3. **Codemods** (TS transforms run in the tooling pool — the same pool that compiles SFCs): `cookie-in-render → move read into loader (event.cookies) so render stays static`; `header-read → loader setHeaders path`; `search-param-read → declare searchSchema (route stays dynamic-by-param but render becomes static/ISR-able)`; `uncached-fetch → 'use cache'`. `oxide why <route> --fix` applies the codemod, re-runs the inference, and prints the before/after mode (dynamic → isr/static) — the fix is verified by the same engine that found the problem.
4. **Scope guard:** one reason per route is auto-fixable per run; multi-reason routes get an ordered plan and fix them one at a time (each fix re-runs inference).

**Acceptance:** four fixtures (one per reason kind) each auto-fix and flip to static/ISR; `--fix` is idempotent (second run: no reasons, no changes).

**Risks:** lowest on the list. Codemods are textually narrow (move a read across a boundary), and the verify-by-re-inference loop catches bad transforms before they ship.

---

## 17. Component cost overlay (M)

**Goal:** in dev, see a component's JS bytes, its share of server render time, and its cache status — hover-level, not dashboard-archaeology.

**Builds on:** per-chunk module ids + route-true bytes (PERF-09 `Chunk.modules`), V3 source maps (`build.sourcemap`), island hydrate timing on the client, L0 hit/stale/miss counters, the HMR client transport (completing SPEC-14's client injection is a prerequisite — same work item), the dev inspector.

**Design:**
1. **Bytes per component:** `Chunk.modules` gives module↔chunk; source-map `sources` gives module↔source-file; the island manifest (#1) gives source-file↔island. Join → per-island/per-route JS bytes, computed at build and stored in the boot manifest (zero client cost at runtime).
2. **Render share:** client-side marks around `hydrateIsland` (`performance.mark/measure`) — hydrate cost per island; server-side: per-route render total (Server-Timing exists) with per-island *server* share only where the framework exposes it (React Profiler-on-server; honest scope: hydration-weight is the primary metric in v0.2, server per-component timing is labeled best-effort per framework in the capability table).
3. **Cache status:** the L0 per-route counters (hit/stale/miss/coalesced — already counted for single-flight) surface in the inspector's route table; the overlay shows the current route's status line.
4. **Transport:** the in-page overlay rides the HMR WebSocket (already the dev-only push channel) — hover an element, resolve owning island/route module, tooltip with `{bytes, hydrateMs, cache}`. No new connection, no prod surface (dev-token guarded like the inspector).

**Acceptance:** example app: hover the counter island → `4.2 KB · 12 ms hydrate · L0 hit 87%`; route table matches the numbers from the budgets/manifest artifacts (cross-checked, not vibes).

**Risks:** low. Source-map joins are the fiddly part; the overlay degrades to route-level numbers when a component isn't an island.

---

## 18. Performance bisect (S)

**Goal:** `oxide perf bisect` binary-searches commits for a bundle-size or latency regression, using the CAS to skip rebuilding unchanged inputs.

**Builds on:** the whole-build fingerprint skip + precompile CAS (warm builds 22–41 ms measured), route-true byte accounting (PERF-09, the budgets source), the bench harness, `build.sourcemap` artifacts.

**Design:**
1. **CLI:** `oxide perf bisect --metric routeJs|clientJs|ttfb [--route <id>] [--good <sha>] [--bad <sha>]` (defaults: last release tag → HEAD).
2. **Probe:** for each probe commit: git worktree checkout → `oxide build` (CAS makes warm rebuilds near-free; `--remote-cache` reuses SPEC-04 when configured) → measure: `routeJs/clientJs` read straight from the budgets accounting (instant), or `ttfb` via the bench harness against a single route (N=20, p50/p99).
3. **Search:** standard git-log binary search on the metric (byte count regressed? latency regressed > 5% — the CI gate's own threshold, reused). Output: first-bad commit, metric delta, and the route/chunk attribution via `oxide why` integration.
4. **Cost control:** commits are probed in worktrees (never disturb the user's tree); builds share the CAS dir, so bisecting a CSS-only change across 8 commits costs ~2 real builds.

**Acceptance:** synthetic regression commit in an example repo found in ≤ 8 builds; the reported chunk is the one the synthetic change touched.

**Risks:** minimal. Latency bisects are noisier than byte bisects — the design keeps both but the byte metrics are the deterministic default.

---

## 19. Per-route capability permissions (L — spike first)

**Goal:** routes/modules declare `net`, `fs`, `env` allow-lists enforced at the op layer — blast-radius limiting for a compromised dependency.

**Builds on:** the op-layer architecture itself (all worker I/O is already mediated by host ops — there is no other path out), `ssrf_safe_fetch` (the fetch op's guard, gains an allow-list parameter), the env prelude assembly, the capability-matrix honesty pattern, ODF hardening precedent.

**Design (spike scope, one week, V8 backend first):**
1. **Declaration:** route-file comment `/* oxide:cap net(api.example.com) env(PUBLIC_*) */` or config `capabilities: { '/api/*': { net: [...], env: ['PUBLIC_*'] } }`. Default (undeclared) = the status quo in v0.2 (deny only what's denied today), with a `permissions: 'strict'` mode flipping default-deny.
2. **Enforcement — V8 backend (hard boundary):** the v8 op layer (`v8_ops.rs`) consults the calling route's allow-list: `fetch` op → host check against `net` (wrapping `ssrf_safe_fetch`'s existing DNS/redirect validation); `env` → the prelude only materializes declared keys; `fs` → no fs op exists (already denied by architecture). Denial = op error surfaced as `CMP-CAP` diagnostic + 403 on fn routes.
3. **Enforcement — node backend (advisory):** real Node workers are not a security boundary and the design doesn't pretend: a best-effort shim over `globalThis.fetch`/`process.env` documents the gap, and the capability matrix states `permissions: Enforced(v8) | Advisory(node)` — the same honest degradation language the deploy adapters use.
4. **Kill criteria (spike):** if op-dispatch overhead exceeds 1 ms p99 with allow-list checks, or a bypass is found in the v8 op surface, file findings; the feature is worthless with holes.

**Acceptance (spike):** backend=v8: fixture route without `net` declaration → fetch to undeclared host denied with CMP-CAP; declared host allowed; overhead measured < 1 ms p99 across the bench suite.

**Risks:** L honestly. The v8 op surface is the right enforcement point and it's small (that's the good news from the dispatch-parity work) — the risk is completeness proof, which is exactly what the spike buys.

---

## 20. Attested reproducible builds (S)

**Goal:** deterministic output + SBOM + SLSA provenance + optional signing from `oxide build --attest`, using the content hashes the build already tracks.

**Builds on:** byte-stable builds per project (`.oxide/secret`, BUG-05), content-hash chunk naming + dist manifest digests, `ring` (already in `Cargo.lock`) for ed25519, blake3 everywhere, the audit command.

**Design:**
1. **Determinism completion:** sort all generated manifest JSON canonically; pin generated-file timestamps to a constant epoch; assert no absolute paths in artifacts (extend the existing audit checks). `--reproducible` mode requires the secret to be pinned (committed or `--secret`), turning "byte-stable per project" into "byte-stable across machines".
2. **SBOM:** CycloneDX JSON assembled offline — Rust deps from `Cargo.lock`, npm deps from `pnpm-lock.yaml` (both parses, no network).
3. **Provenance:** SLSA v0.2 in-toto statement: `{builder: oxide@version, materials: [{git-sha, lockfile digests}], artifacts: [dist manifest digests]}` — all inputs the build already computes.
4. **Signing + verify:** ed25519 signature over the canonical manifest JSON; key from `.oxide/keys` or env. `oxide audit --verify <attest.json>` re-derives every digest and checks the signature — CI-friendly, offline.

**Acceptance:** two CI runs, same commit, different machines → identical dist manifest digests; verify passes; a tampered `dist/` fails with the exact artifact named.

**Risks:** low — this is discipline and assembly, not invention; the only real work is flushing nondeterminism out of generated artifacts (the audit-extend loop handles it incrementally).

---

## 21. Cross-cutting decisions (design once, reuse everywhere)

1. **Build id** (#10): blake3 of dist manifest digests. Consumers: #11 (delta state), #13 (graph cache key), #14 (IDB cache invalidation), boot script, logs. One artifact, four consumers — land it first among the ops features.
2. **ODF PATCH envelope** (#6): the only new wire format on the list. Consumers: #5 (live deltas), `/__data` revalidation. Everything else reuses existing ODF as-is. Golden fixtures are non-negotiable — the 16-fixture culture is why ODF is trustworthy.
3. **Locals threading** (#4 + middleware Wave-0 work): one plumbing change carries `locals` (middleware), `experiments` (#4), and the currently-dropped `locale` (i18n bug found in verification) through loaders/actions/render. Do it once, not three times.
4. **Endpoint table discipline:** new endpoints — `/__oxide/live` (#5), `/__oxide/stream/:id` (#7), `/__oxide/search` (#9), `/_oxide/nav` (#13), `/_oxide/rust/<fn>` (#8) — each is one dispatch arm in the existing closure, each inherits rate-limit rules, host allow-lists, and logging spans automatically. No endpoint gets bespoke auth; they use the plane's rules.
5. **Conformance + capability matrix updates are part of each feature's definition of done:** #1 adds the mixed-fixture assertion; #8/#19 add matrix rows (rustFns, permissions); #9 adds `search: Full|Static`; #10 adds `skew`; #11 adds `delta`. A feature without a matrix row or conformance assertion isn't done — that's the house rule, applied to new features.
6. **Everything dev-only stays dev-token-guarded** (#17 overlay, replay recordings in dev) — the SEC-09 token pattern extends unchanged.

---

## 22. Sequencing and dependencies

```
Wave 0 (prereq — the live-path bucket, already planned):
  router boot · real hydration ×3 frameworks · layouts/parent · defer values
  route.ts + middleware + locals threading · loading/error/not-found
        │
        ├── #3 interaction replay (S)            [needs boot + hydration lifecycle]
        ├── #1 mixed islands (M)                 [needs real hydration ×3]
        └── #5 live loaders (M) ──► #6 ODF deltas (M)   [needs defer-glue value path]

Parallel, no Wave-0 dependency:
  #16 why-dynamic fixes (S)     — standalone, compiler + CLI
  #18 perf bisect (S)           — standalone, CLI + CAS
  #20 attested builds (S)       — standalone, build + audit
  #10 build id + skew (M) ──► #11 delta deploys (M), #13 learned prefetch (M), #14 offline (M)
  #12 brownout (M)              — standalone, server crate
  #7 resumable streams (M)      — needs defer/stream paths (Wave 0 for streaming SSR)
  #8 rust server fns (M)        — dispatcher seam; capabilites hook awaits #19 spike outcome
  #9 built-in search (M)        — standalone, new crate

Spikes (timeboxed 1 week each, kill criteria written above):
  #2 auto-islands (L)           — inference pass + savings measurement
  #19 capabilities (L)          — v8 op-layer enforcement + overhead measurement

Owner's picks as Wave 1: #1, #3, #5, #8, #10, #16 — the designs above make
each of them a bounded slice: #16 and #3 are S-sized and standalone; #5 rides
tags+ODF+SSE; #8 rides the dispatcher seam; #10 is the build-id keystone; #1
is the Wave-0 hydration work's proof and payoff.
```

**One honest note on ordering:** #1 (mixed islands) is the demo that *is* the pitch, but it's also the last thing that becomes true — it consumes real hydration for three frameworks. If marketing needs the demo early, the intermediate milestone is "mixed islands where React + one other framework hydrate for real" — the adapter-registry design above supports shipping it framework-by-framework.

---

## 23. Spec diffs to draft with the v0.2 spec

1. **§7 (adapter contract):** promote mixed-framework islands from post-MLP into scope — the adapter registry (`adapters: Record<Framework, Adapter>`) and the per-island `framework` field become contract, and `hydrateIsland` must be real per adapter (IR replay loses conformance status).
2. **§12 (server runtime):** new subsection "Build generations and skew" — build id, two-generation serving window, `x-oxide-build` semantics, rollback contract.
3. **ODF appendix:** the PATCH envelope tag, op set, canonical-ordering guarantee, and fallback rule.
4. **§9 (data layer):** `liveLoad`, SSE subscription semantics, idempotency keys on actions.
5. **§16 (platform):** search (native + static boundary), offline module, nav-graph.
6. **§17 (security):** capability declarations and the Enforced/Advisory matrix semantics.

---

*Bottom line: all 20 are buildable without new research except #2 and #19, which are designed as one-week spikes with kill criteria. Eleven of the twenty ride infrastructure that already exists and is tested (tags, ODF, CAS, pool affinity, op table, dispatch endpoints, L0 stale machinery, PWA SW, enhance, HMR transport, Prometheus gauges). The plan's center of gravity is unchanged from the scorecard: land Wave 0, then these designs each become days-to-weeks of wiring rather than months of invention.*
