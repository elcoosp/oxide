# Oxide implementation deviations & decisions

Cross-crate log of deliberate deviations from the stub/spec letter, with rationale.
Append per crate; never rewrite entries.

## oxide-cache (Task ID: 5, Agent: cache)

- **moka replaced by an in-house weighted LRU** (spec §11.2 says "moka, TinyLFU").
  moka is not in the pinned workspace dependency set, so `MemoryTier` implements a
  byte-weighted LRU directly: `DashMap` for data + per-key weights + an `AtomicU64`
  total + a `Mutex<VecDeque>` access-order log that is compacted to each key's last
  occurrence (front = true LRU) before every eviction pass and whenever the log
  outgrows `4 × entries + 64`. Entry weight = `body.len() + Σ(header key+value bytes)
  + 32`. Entries heavier than the cap are refused. Design notes live in the
  `crates/cache/src/tiers.rs` module header.
- **Single-flight publishes after removing the in-flight entry** (rather than the
  naive "publish, then cleanup"): a follower subscribes while holding the DashMap
  shard read lock, and the owner's removal needs the write lock on that shard, so
  nobody who found the entry can miss the broadcast value. Acquirers arriving after
  removal become the next owner (bounded thundering-herd window, documented in
  `crates/cache/src/single_flight.rs`).
- **Soft tag invalidation marker scheme** (§11.3 "marks stale"): the entry is kept,
  `policy.ttl` is zeroed, `policy.swr` is extended to `original_swr + original_ttl`,
  and `stored_at` is rewound 1 ms so the zeroed TTL takes effect immediately at
  millisecond resolution. An entry already past its whole TTL+SWR window is deleted
  instead of resurrected.
- **HTTP-date parsing** (§11.3): RFC 9110 IMF-fixdate only (`Sun, 06 Nov 1994
  08:49:37 GMT`); bare ASCII digits are additionally accepted as unix milliseconds
  (test/dev convenience). RFC 850/asctime forms are not supported.
- **`stale_if_error` is not consulted by `TieredCache`** — per the task split, the
  render layer owns serve-stale-on-error; the cache exposes `Entry::stale_until()` so
  the caller can make that decision.
- **Error codes** introduced: `CAC-IO`, `CAC-DB`, `CAC-JSON`, `CAC-ZSTD` (§19.4).

## oxide-jshost (Task ID: 7, Agent: jshost)

- **`V8Host` is a fail-fast stub**: deno_core is not in the pinned workspace dependency set, so both
  pool constructors return `JSH-V8-UNAVAILABLE` (diagnostic names the missing backend) instead of a
  working §4.2 default. `NodeHost` + `ScriptedHost` carry the conformance load.
- **In-house base64 decoder** (no base64 crate pinned; CONVENTIONS #7): RFC 4648 std alphabet with
  padding, used to move binary chunks through the JSON protocol; invalid input → `JSH-B64`.
- **stdio framing instead of "Unix socket + shared-memory ring"** (§4.2 backend note): both backends
  speak the same length-prefixed JSON op protocol; the Node transport runs over stdin/stdout with
  base64 binary chunks. Binary bodies are copied through base64, not zero-copy handed off.
- **`ScriptedHost` converted from a unit struct to `{ handler: Option<ScriptedHandler> }`** (task
  brief allows it; no other crate constructs it). `ScriptedHost::default()` preserves the old
  behavior — every call fails `JSH-NOSCRIPT` and bumps `failed`; additive constructors
  `ScriptedHost::with_handler`, `ScriptedPool::new(handler, cfg)` / `without_handler(cfg)`.
- **Recycle semantics**: the dispatch that crosses `recycle_after_requests` replaces the worker
  (kill+respawn), but only when the worker is idle (`in_flight == 0`) — otherwise the recycle is
  deferred to a later dispatch. §4.3's "warm replacement before the old one drains" needs the V8
  isolate model; child workers restart cold. Crash replacement shares the same code path without
  counting a recycle (crashes are counted at stdout EOF).
- **Route-affinity is `None` for `NodePool` dispatch** (per task brief); `choose_worker` implements
  the full least-loaded + affinity rule (§4.3) for callers that have a route.
- **drain() waits at most 1s** for in-flight calls (10ms poll) — task brief "bounded 1s in tests";
  `abort_request` resolves pendings with `JSH-ABORT` and late worker replies are dropped.
- **Chunked fan-out**: worker→host `CallChunk`s resolve the pending call as `Chunked(rx)` on first
  chunk and forward into a bounded(16) mpsc; a full/dropped collector removes the stream sender
  (single reader task per worker, no head-of-line blocking).
- **Error codes** introduced: `JSH-FRAME`, `JSH-JSON`, `JSH-NOSCRIPT`, `JSH-NODE`, `JSH-TIMEOUT`,
  `JSH-CRASH`, `JSH-ABORT`, `JSH-DRAIN`, `JSH-V8-UNAVAILABLE`, `JSH-B64`. The worker's
  `JSH-BOOM`/`JSH-NOKIND`/`JSH-WORKER-ERR` replies are test-only worker semantics.
- **Worker script** (`crates/jshost/worker/oxide-worker.mjs`, 77 lines, pure Node ESM, no deps):
  sends `Ready{pid}` on start, handles `render` → `{html}`, `tooling:compile` → uppercase echo,
  `boom` → `CallErr`, `die` → exit(1) (crash drill), `sleep` → delayed reply, `payload.chunked` →
  `CallChunk*` + `CallEnd`; exits cleanly on stdin EOF / stdout EPIPE.

## oxide-plugin (Task ID: 7, Agent: jshost)

- **Dedupe = silent in-place replace** (§15.2 load-failure behavior kept separate): re-registering a
  name replaces the earlier descriptor in its original ordering slot; no `PLG-DUP` diagnostic.
- **Error codes** added beyond the brief's `PLG-CAP`/`PLG-HOOK`: `PLG-WIT` (empty `wit_world`) and
  `PLG-HOOKS` (empty hook list) for the remaining `WasmPluginManifest::validate` failures.

## Cross-crate summary (integrator, Task 13)

The per-task details above are authoritative. Major program-level deviations from the
spec's *exact* external dependencies — all behind the spec's own isolation traits
(Appendix A) — summarized for review:

| Spec choice | Status here | Isolation point |
| --- | --- | --- |
| `deno_core` / V8 render pool (D2, §4.3) | `NodeHost` (real Node child workers over the same op protocol, crash/recycle/abort tested) + `ScriptedHost` for tests; `V8Host` fails fast with `JSH-V8-UNAVAILABLE` | `oxide_jshost::JsHost` |
| Rolldown as a library (D3) | functional ESM bundler: resolution w/ `exports` conditions, route splitting, shared chunks, content hashing, manifests | `oxide_bundler::Bundler` |
| Oxc passes (D4) | tolerant scanner transformer: import scan, `'use cache'`, server-fn stubs, guard, HMR inject | `oxide_compiler::Transformer` |
| Lightning CSS (D5) | in-house CSS engine: nesting, modules, minify, prefix subset | `oxide_assets::CssBackend` |
| moka (§11.2) | in-house byte-weighted LRU (details above) | — |
| AVIF encode (§16.1) | `AST-AVIF` diagnostic; PNG/JPEG/WebP-lossless via `image`; libvips behind `ImageBackend` (D19) | `oxide_assets::ImageBackend` |
| fontations/skrifa (§16.2) | pass-through bundling + metric-override model | `oxide_assets::FontBackend` |
| resvg/taffy OG rendering (§16.3) | model + cache key only | `oxide_assets::OgBackend` |
| brotli dynamic compression (§12.1) | precompressed `.br` serving supported; runtime encode returns `SRV-NOENC` | `oxide_server::compress_body` |
| wasmtime component plugins (§15.1) | manifest/capability validation; execution behind the WASM host | `oxide_plugin::WasmPluginManifest` |
| lol_html (§10.2 step 4) | **used as specified** (render crate) | — |

Deviations in the TypeScript layer: adapters render a neutral element-tree IR with
framework-flavored markers (real framework renderers slot in when npm deps are
installed — react-dom/server, @vue/server-renderer, svelte, solid-js); the wire shapes
(boundary protocol, islands, head model) match the Rust render crate and the spec §7/§10.


## oxide-cache follow-up: moka restored (spec §11.2 alignment)

- **In-house weighted LRU replaced by `moka::sync::Cache`** with a `u32` weigher of
  `entry_weight` and `max_capacity = max_bytes`. Entries heavier than the cap are
  refused by moka's own admission rule, matching the previous policy. Eviction is
  forced to run synchronously on `insert`/`len`/`is_empty`/`total_bytes` via
  `run_pending_tasks()` so the observable behavior stays deterministic for tests.
  The `MemoryTier` public API (`new`, `get`, `insert`, `remove`, `clear`, `len`,
  `is_empty`, `total_bytes`, `max_bytes`) is unchanged; the `map` field is now
  private and holds `moka::sync::Cache<String, Arc<Entry>>`.
- `moka` added to `[workspace.dependencies]` with features `["future", "sync"]`.


## oxide-jshost follow-up: V8Host restored (D2 alignment)

- **`V8Host` is now a real `deno_core`-backed pool** (spec D2: "embedded V8 via
  `deno_core` primary"). One `JsRuntime` per worker thread, each pinned (JsRuntime
  is `!Send`); calls cross the thread boundary via a bounded `tokio::sync::mpsc`
  channel plus a shared pending map. Dispatch handles `render` and
  `tooling:compile` with the same response shape as the Node worker, plus the
  `boom` / `die` special cases (both rejected Rust-side). `abort_request` and
  `drain` use the same call-shape as `NodePool`. `stats()` reports the same
  counters.
- **`NodeHost` is retained** (§4.6 tier 3: chosen automatically when the build
  detects a `.node` addon or an unsupported builtin, or explicitly via
  `runtime: 'node' | 'bun'`). Both backends speak the same op surface, so the
  conformance suite exercises both (§4.2).
- **`JSH-V8-UNAVAILABLE` is no longer emitted**; the diagnostic is retired.
- `deno_core = { version = "0.412.0" }` added to `[workspace.dependencies]`.


## oxide-bundler follow-up: Rolldown wired (D3 alignment)

- **`RolldownBundler` added** (`crates/bundler/src/rolldown_backend.rs`): a real
  Rolldown-backed implementation of the [`Bundler`] trait (D3: "Rolldown used as
  a library"). It constructs `rolldown::BundlerOptions` from Oxide's
  `BundleOptions`, drives `rolldown::Bundler::write()` on a dedicated
  current-thread Tokio runtime (Rolldown is async), and adapts the returned
  `rolldown::BundleOutput` to Oxide's `BundleOutput` / `Chunk` shapes plus the
  client/server manifests.
- **`ChunkBundler` (the in-house ESM bundler) is retained** and still the
  default in `oxide-cli`. It is a valid implementation of the same trait; the
  two coexist so the swap is a per-call-site choice rather than a rewrite. See
  `docs/architecture.md` for the crate's role.
- **Per-module metadata gap**: Rolldown exposes module-level info via its
  plugin hooks (`buildStart`/`resolveId`/`load`/`transform`), not through the
  `BundleOutput` surface. `RolldownBundler` currently returns an empty
  `modules` list. Capturing it requires a Rolldown plugin registered with
  `Bundler::with_plugins`; the plugin bridge (spec §15.1) will cover it.
- **External / define / tsconfig_paths mapping**: `BundleOptions::externals`,
  `define`, and `tsconfig_paths` are not yet threaded into Rolldown's
  `IsExternal` / `define` / `TsConfig::Manual` options. `ChunkBundler` still
  honours them. Wiring is a mechanical follow-up once a call site actually
  passes non-empty values through to `RolldownBundler`.
- `rolldown = { version = "1.2.13" }` and `rolldown_common = { version = "1.2.13" }`
  pinned in `[workspace.dependencies]`.
