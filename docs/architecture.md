# Oxide architecture — spec § → implementation map

## Pipeline (§3, §12.1)

```
request → hyper (oxide-server) → limits/id → host check → static assets →
native rules (oxide-router RulesEngine, D13) → L0 cache (oxide-cache) →
JS middleware (only when present) → dispatch: render (oxide-render + adapter via
JsHost) | action | endpoint | server fn → streaming response + compression →
cache fill + tags
```

The hot path completes without entering JS when the answer is cached, redirected,
rewritten or static (§4.1, D13) — enforced by `Pipeline::native_prefix` ordering.

## Crates

| Crate | Owns | Key spec sections |
| --- | --- | --- |
| `diagnostics` | One diagnostic format (code, frame, cause, fix) shared by Rust, tooling workers, isolates | §19.4 |
| `odf` | Wire-format encoder/decoder, shared/ref tables, deferred slots, hardening limits; F06 patch substrate (`patch::diff`/`apply`, `ODP1` envelope, truncated-SHA-256 base ids, cross-language goldens) | §9.5, §17.1, D11, V2-F06 |
| `router` | Route scanning (§8.1), radix matcher with priority rules (§8.2), rkyv archive (D8), typegen (§8.3), native rules engine, per-route `live` opt-in (`RouteEntry.live`, §V2-F05) | §8, §12.1, D8, D13, V2-F05 |
| `graph` | Demand-driven incremental engine, early cutoff (§5.1), tracked inputs (§5.2), CAS (blake3+redb+zstd, §5.4), cancellation (§5.6), RemoteCache trait | §5, D6 |
| `config` | Typed config, layering (defaults→preset→file→mode→env→CLI), env files, `PUBLIC_` split, static extraction with V8 fallback flag, typo diagnostics, `live` transport knobs + DSP-111 (§V2-F05), `experiments` + DSP-110 (§V2-F04), `stream.resume` bounds + DSP-112 (§V2-F07), `brownout` thresholds + DSP-113 (§V2-F12), `interactionReplay` + DSP-114 (§V2-F03) | §14, D14, V2-F03, V2-F04, V2-F05, V2-F07, V2-F12 |
| `cache` | L0/L1/L2 tiers, weighted LRU memory tier, redb disk tier, tags, TTL/SWR/stale-if-error, single-flight, build seeding, HTTP conditional semantics, canonical `build_id_of_dist` (A04) | §11, V2-A04 |
| `compiler` | Transform passes behind `Transformer` (D4): import scan, `'use cache'` (§9.3), server-fn stubs (§9.4), forbidden-import guard (§6.4), HMR injection, why-dynamic reason engine (`infer_reasons`, §V2-F16) | §6.2–6.4, §9, V2-F16 |
| `bundler` | `Bundler` trait (D3) + functional ESM bundler: resolution with `exports` conditions, route-based splitting, shared-chunk heuristic, content hashing, client/server manifests; the V2-E04 SRI pass (`sri_digest`/`integrity_map` — the chunk path digests the bytes in hand, the rolldown path reads the persisted files back) recording `client.manifest.json`'s `integrity: { file: value }` | §6.4, V2-E04 |
| `jshost` | `JsHost`/`Pool` traits (§4.2), NodeHost with real child workers speaking the op protocol (§4.5), ScriptedHost, V8Host (deno_core, full dispatch + `compile_module`) | §4, D2 |
| `plugin` | Hook model, pre/post ordering, JS bridge messages, WASM plugin manifest validation, module manifests | §15 |
| `dispatch` | V8-free request orchestration: URL/query helpers, stage `Flow` protocol, §8.5 middleware chain + unified locals (A05), §9.2 parallel load graph (A06), §15.4 experiments assignment engine (F04), build id + generations substrate (A04) with the live route on the data plane (`/__oxide/live` build query contract, §V2-F05) and the F07 reattach route (`/__oxide/stream/<id>` + `stream_resume_id`) | §4.1, §8.4, §8.5, §9.2, §9.3, §15.4, V2-A02, V2-A04, V2-F05, V2-F07 |
| `render` | Streaming pipeline, head merge, HTML injection (lol_html), deferred-boundary protocol (§7.1), islands markup (§10.3), hydration bootstrap (+ the §V2-A04 `build` field), error recovery; the V2-E04 `Injector.integrity` map (modulepreload/stylesheet links whose URL resolves to a manifest digest emit `integrity` + `crossorigin="anonymous"`) | §7.1, §10, V2-A04, V2-E04 |
| `server` | Hyper 1.x pipeline, static assets (range/304/precompressed), dynamic compression, security defaults (§17.1), CSRF/SSRF/host checks, rate limiting, cookies, RFC6455 codec, graceful shutdown; §V2-F05 live substrate: SSE framing, tag hub (`LiveHub`/`Sub`/`Guard`), snapshot stream, rerun runner, same-origin gate; §V2-F06 delta policy (`delta_payload`, per-connection patch base); §V2-F07 resumable-stream substrate (`stream_store`: `StreamBuf`/`StreamStore`, `read_from` replay, `stream_id`, Range parse, 206/200/410 reattach responses, `tee_stream`); §V2-F12 brownout substrate (`governor`: `Level`/`Signals`/`Thresholds`, `raw_level`/`next_level` hysteresis, `Governor` atomic, `serve_decision` × route `Priority`, `shed_response`, `ready_status`); §V2-A07 prerender error-status sidecars (`static_status`: `StatusSidecars::scan` serve-start walk, `status_for`, `write_status_sidecar`, the `serve` 200-override); the V2-E04 CSP half (`csp_headers_for` — the one definition site for the strict/hash/report-only × Trusted Types header matrix — and `POST /_oxide/csp-report`, the rate-limited violation sink over both browser formats) | §12, §17, D7, V2-F05, V2-F07, V2-F12, V2-A07, V2-E04 |
| `assets` | CSS pipeline (nesting/modules/minify/prefix), image backend (resize/encode/LQIP), font bundles, metadata/SEO, sitemap/robots, markdown/content, OG model | §6.6–6.7, §16, D19 |
| `dev` | Hash-confirmed watcher (§5.5), HMR message protocol + per-kind behavior table (§13.3), the V2-E01 boundary planner (`plan`, the breadth-first walk-up over the module graph's importers, plus `source_self_accepts` and the `DEV-HMR-001` reload diagnostic) and its phase-2 CSS boundary substrate (`plan_classified` — the `kind: 'js'|'css'|'data'` classification with the §13.3 CSS terminal, `css_self_accepts`, the `DEV-HMR-002` orphan-stylesheet warning) and its phase-4 state-preservation substrate (`state::Preservation` on the v1 `update` frame, the injectable `StatePolicy` seam, `update_frame`), inspector model (§13.4), prebundle keys | §13, V2-E01 |
| `adapters` | Deploy targets + the V2-R02 capability-matrix v2 (`support` as a total `(Class, Capability)` function, `Capability::ALL` with the v0.2 rows, the §18 refusal rule over the v0.2 `requires` keys AND over the config keys — the `ConfigFeatureReport` seam + `check_config` the config layer maps and the CLI build consults, with F09's `search` arm live) + the E11 generated docs table (`capability_markdown_table`) + `_headers`/`_redirects`, Dockerfile, Worker/node/serverless entries | §18, V2-R02, V2-F09 |
| `core-wasm` | Edge subset: cache keys, head merge, ODF re-encode (D18) | §2 D18 |
| `schema` | JSON Schema generation for `oxide.config.ts` — editor autocomplete (`$schema`), the `oxide schema` stdout surface, CI config-surface diffing | §14, §19.3 |
| `cli` | All §19.1 commands: dev/build/start/preview/routes/analyze/check/why/doctor/secrets/add/generate/cache/audit/migrate/test/mcp/schema/config/render/docs/verify; `why --route` prints the V2-F16 OX-DYN reasons and `routes`/`build` write `.oxide/routes.reasons.json`; `perf bisect` is the V2-F18 performance-bisect engine (spec-verbatim search over git worktree candidates, shared `OXIDE_CACHE_DIR`); `build --attest` + `verify` are the V2-F20 attestation engine (CycloneDX SBOM, in-toto/SLSA provenance, ed25519 signatures, subject digests, `--rebuild`); `docs generate` is the V2-E11 reference-docs engine (the `ReferenceGenerator` registry over `docs/reference/`, `--check` is the docs gate); the V2-E04 SRI wiring (`boot_script_tag` stamps the boot `<script type=module>` with the manifest digest on both the prerender and the live-SSR paths, `security.sri`-gated) | §19, §V2-F16, §V2-F18, §V2-F20, §V2-E11, §V2-E04 |

## What changed since 2026-10-12

- **`bundler` + `config` + `render` + `server` + `cli`**: the V2-E04
  security surface (`feat/security-sri-manifest` + `feat/
  csp-report-endpoint`). The SRI half: `client.manifest.json` gains
  `integrity` — `sha384-<base64>` per emitted JS chunk and CSS file
  (the chunk path digests memory, the rolldown path reads the persisted
  files back); the CLI stamps the boot `<script type=module>` tag with
  the digest (`integrity` + `crossorigin="anonymous"`) on both the
  prerender and the live-SSR paths, `security.sri`-gated (default on);
  `render`'s `Injector` carries the map for modulepreload/stylesheet
  links. The CSP half: `server::csp_headers_for` is the one definition
  site for the strict/hash/report-only × Trusted Types header matrix,
  and `POST /_oxide/csp-report` is the rate-limited violation sink over
  both browser formats. The typed `security.sri`/`security.trustedTypes`
  keys are schema'd and `CFG-SEC`-validated. Deltas and deferrals — see
  `docs/deviations.md`.

- **`cli`**: the E11 docs generator (`feat/docs-generate-seam` + `feat/
  errors-registry-page`) — `oxide docs generate` writes `docs/reference/`
  from the crates the binary links (capabilities.md from the §18 matrix
  fold, config.md from `config_schema()` with the DOC-001
  every-key-described refusal, cli.md from the live clap tree,
  errors.md from the new `docs/errors.toml` registry), `--check` is the
  gate the new CI `docs-check` job runs, and
  `tests/errors_registry_tdd.rs` holds the registry against every
  production code literal (the V2.K call-site gate). Deltas and
  deferrals — see `docs/deviations.md`.

- **`adapters` + `config` + `cli`**: F09's `search` config key + the §18
  refusal's `AST-SRC-001` arm — the fourth and last open code of the
  V2-R02 config-key rule is live. The key is typed over the full V2.L
  shape (`SearchConfig`: collections, field boosts, snippet, languages),
  schema'd and typo-suggestion-validated, inert until the engine lands;
  `ConfigFeatureReport::search_configured()` (a required method) maps
  `search.collections` and `check_config` consults the `Search` row in
  capability declaration order — edge refuses with the registry fix
  text, Node/serverless/static warn with the documented degradation
  string, and the §16.5 collection indexes (static JSON) never set the
  surface. Plus the E11 docs-table seam: `capability_markdown_table()`
  renders the matrix as a deterministic markdown table the crate README
  embeds byte-identical, pinned by `doc_table_matches_the_function`.
  Deltas and deferrals — see `docs/deviations.md`.

- **`dev` + `cli` + `packages/core`**: the state-preservation substrate
  (V2-E01 phase 4) — `dev::state` (`Preservation` — the `preserve`
  verdict on the v1 `update` frame, `keep` or `clear` with the planner's
  reason; the injectable `StatePolicy` seam with `DefaultStatePolicy`;
  `update_frame`, the single frame-construction seam), the CLI fan-out
  stamping every `update` frame through it (a full reload planned in the
  same tick downgrades the batch verdict to `clear` with the same reason
  the `full-reload` frame carries), and the TS client half
  (`packages/core/src/client/hmr.ts`: snapshot → apply → restore around
  boundary-scoped updates, the sessionStorage stash across a fallback
  reload, `boot.ts` restoring the stash on the ESM dev path). The spec's
  third HMR acceptance — *type into an input, edit the component's text,
  the input value survives* — is now a wire contract, not an adapter
  accident. Deltas and deferrals — see `docs/deviations.md`.

- **`adapters`**: the capability-matrix v2 seam (V2-R02) — the fixed-size
  `row([...])` table becomes `support(target, capability)`, a total
  function over the private `Target → Class` mapping, exhaustive on both
  dimensions (the compiler proves every capability has a cell for every
  target class); `Capability::ALL` gains the v0.2 rows (`MixedIslands`,
  `Resume`, `Skew`, `DeltaDeploy`, `RustFns`, `Search`, `Offline`,
  `Brownout`, `Permissions`, `Attestation`) with every new `Degraded`
  cell documented in `degradation`, and the §18 refusal rule extends to
  the v0.2 server-manifest `requires` keys. The v0.1 rows are pinned
  byte-identical by the golden test. Deltas and deferrals — see
  `docs/deviations.md`.

- **`server` + `cli`**: the prerender error-status sidecar substrate
  (V2-A07) — `server::static_status` (`StatusSidecars::scan`, the spec's
  serve-start walk over `dist` with malformed content ignored;
  `status_for`; `write_status_sidecar`, the write half of the contract —
  non-200 writes the decimal sidecar, a 200 write clears a stale one),
  the `StaticAssets` integration (the scan rides `scan_filtered`; `serve`
  applies the sidecar status to 200 responses and hard-denies `*.status`
  paths alongside the walk skip) and the CLI write sites (`write_prerender`'s
  four emit paths + both incremental-reuse branches). A prerendered error
  page — a loader short-circuit or an adapter error-status marker — is now
  served as its real status by `oxide start` from disk, closing the gap
  the L0 seed could not cover (cold starts, non-default-locale pages).
  The criterion bench (`status_sidecars_scan`, `status_for`) behind
  `bench-util` and the `status_sidecars` walkthrough example ride along.
  Deltas and deferrals — see `docs/deviations.md`.

- **`dev` + `cli` + `packages/core`**: the message-schema v1 wire flip
  (V2-E01 phase 3) — `HmrMessage` speaks the spec's v1 shapes behind a
  `{"v":1}` envelope: `update` (N `UpdateEntry`s — `kind: 'js'|'css'|'data'`,
  `acceptedPath`, a publish-time `timestamp` and a blake3 `hash`; the §3.2
  singular/batch wire distinction dissolves into the array), `full-reload`,
  `error`, `ox:data` (the pushed loader payload) and `connected{build}`.
  The decode side gains the typed gate (`try_from_json` +
  `HmrWireError::Schema` + the `DEV-HMR-003` fix-text diagnostic) and
  `from_json`'s silent `.ok()` swallow becomes a loud warn. The CLI fan-out
  publishes stamped entries (`UpdateEntry::stamped`); the tailwind sheet's
  inline `code` payload is retired (no client ever read it — the swap
  refetches through the cache-busted URL) and `server-module-swap` is
  retired (it was a client no-op). The TS client applies the v1 shapes
  through one code path with a legacy read-compat normalizer for stale
  tabs; `hot.ts`'s worker socket rides `acceptedPath`. Pin counts move
  1,750→1,756 Rust and 304→314 TS; the criterion bench (`hmr_wire`) and
  the `hmr_wire` walkthrough example ride along. Deltas (base64 `data`,
  kind = the client's update mechanism, the compat window) and deferrals
  (state preservation, Vue/Solid refresh) — see `docs/deviations.md`.

- **`cli` + `cache`**: the attested-reproducible-builds substrate (V2-F20)
  — `cli::attest` (`canonical_json` with the explicit recursive key sort,
  the strict `Cargo.lock` / `pnpm-lock.yaml` line parsers, the CycloneDX
  1.5 SBOM builder, the in-toto Statement v1 / SLSA v1 provenance with
  the spec's `buildDefinition` facts, `subtree_digest` + `dist_subjects`
  over root-relative slash-normalized paths, the fixed-seed ed25519
  sign/verify round trip, and the `ATT-*` diagnostics with fix lines)
  behind `oxide build --attest` (writes `dist/build-id` — the A04 id —
  and `dist/attest/{sbom.cdx.json, provenance.intoto.json,
  provenance.sig}`) and `oxide verify` (canonical gate, `--pubkey`
  signature check, subject walk, `--rebuild` scratch-copy digest
  comparison). The composer closes part of the A04 deferral: the
  `dist/build-id` write now happens under `--attest`. Deltas (empty
  cargo section for lock-less projects, no manifest filter on the pnpm
  side yet, no signature check without `--pubkey`) and deferrals — see
  `docs/deviations.md`.

- **`dev` + `cli`**: the HMR boundary substrate (V2-E01) —
  `dev::boundary::plan` (`Plan::Update`/`Plan::FullReload`, the
  spec-verbatim breadth-first walk-up over `importers` that stops at
  self-accepting boundaries on every path, cycle-safe, sorted + deduped),
  `source_self_accepts` (the `import.meta.hot.accept` probe), the
  `DEV-HMR-001` full-reload diagnostic with the fix help text for the
  §13.4 overlay, `hmr_boundary` tracing spans, the criterion bench behind
  `bench-util`, and the walkthrough example. The CLI's adapter-HMR fan-out
  is planner-driven (`build_hmr_graph` harvests `importers`/`accepting`/
  `entries` in one walk): a non-boundary chain now updates its top
  component boundary instead of leaving the import binding stale — the
  one-hop `update_boundary` simplification is retired. Phase 2 (the CSS
  boundary substrate) adds the classified planner:
  `dev::boundary::plan_classified` (`UpdateKind` `js|css|data` + the walk
  in one call, the §13.3 CSS terminal — a stylesheet never reloads),
  `css_self_accepts` (the harvest's style-injection probe; stylesheets
  join the graph), `css_orphan_diagnostic` (`DEV-HMR-002`), three more
  criterion groups and the `css_boundary` example. Deltas (the
  changed file always re-imports; orphans pre-filtered to a quiet
  self-update; the CSS terminal lives in `plan_classified`, not `plan`)
  and deferrals (state preservation, Vue/Solid refresh — the
  message-schema v1 wire flip landed as phase 3 above) — see
  `docs/deviations.md`.

- **`cli` (the cache root + `perf`)**: the performance-bisect substrate
  (V2-F18) — `perf_bisect` (`Metric` build-ms/client-js-bytes/ttfb-ms, the
  spec-verbatim `bisect` search, the injectable `Probe` seam with the
  median-of-`runs` contract, the `ceil(log2(n))+1` acceptance bound, the
  `PERF-*` diagnostics with fix lines) behind `oxide perf bisect`, the
  criterion bench behind `bench-util`, and the F18 environment row:
  every cache-root resolution in the CLI (build fingerprints, L1 disk
  tier, precompile CAS, audit/mcp/doctor/cache surfaces) now honours
  `OXIDE_CACHE_DIR` so a bisect's candidate worktrees share one cache.

- **`compiler` + `cli` + `diagnostics`**: the why-dynamic substrate
  (V2-F16) — `compiler::infer_reasons` (`DynamicReason`/`ReasonKind`, the
  collect-all sibling of `infer_render_mode`'s first-hit visitor, plus the
  `UncachedFetch` kind for `fetch(` calls outside a `'use cache'`
  function), the `OX-DYN` diagnostics with code frames and the fix help
  text per reason kind, `oxide why --route <id>` (mutually exclusive with
  the module surface), and the `.oxide/routes.reasons.json` side-car
  written by `oxide routes`/`oxide build` (never the rkyv archive the hot
  path reads). Codemods (`--fix`) are deferred — see
  `docs/deviations.md`.

- **`config` + `render` + `cli` + `jshost`**: the F03 Rust config
  surface — `interactionReplay` boolean key with DSP-114 validation, the
  hydration bootstrap advertising the opt-out to the client, the CLI
  threading the key through its call sites, and the loader host adapted
  to the endpoint outcome shapes. Landed with v0.6.0.

- **`packages/core`**: the interaction-replay substrate (V2-F03) —
  `src/client/replay.ts` (capture-phase listeners for `click`/`input`/
  `change`/`focusin` against pending `<ox-island>` hosts, the 100-record
  drop-oldest log, host-relative child paths, replay with node-identity
  verification), wired into `hydrateIsland` (replay right after
  `data-ox-hydrated` becomes `done`) and both boot paths (capture starts
  before any island hydrates; payload `interactionReplay: false` opts
  out). Spec deltas and the deferred Rust config surface — see
  `docs/deviations.md`.

- **`server` + `config` + `router`**: the brownout-mode substrate
  (V2-F12) — `server::governor` (the spec-verbatim `Level`/`Signals`/
  `Thresholds` model, the pure `raw_level` breach scan and `next_level`
  hysteresis transition, the `Governor` atomic the request path reads),
  the enforcement seam (`Priority` with the `low|normal|critical` grammar,
  `serve_decision` level × priority → `Decision`, `shed_response` with
  `retry-after: 5` + `x-oxide-brownout`, `ready_status` flipping only at
  `Emergency`), the `oxide_brownout_level` Prometheus gauge, the
  `brownout` config section (`enabled`, `staleCeiling`, the three
  threshold ladders, the opt-in `hedge`; DSP-113 + `rules[].priority`
  validation, JSON schema regenerated), and `NativeRule.priority` (router
  + config mirror). Sampler/enforcement-stage wiring deferred — see
  `docs/deviations.md`.

- **`server` + `dispatch` + `config` + `packages/core`**: the
  resumable-streams substrate (V2-F07) — `server::stream_store`
  (`StreamBuf`/`StreamStore` with the ttl/`max_each`/`max_total` bounds,
  `read_from` replay, `stream_id`, `parse_range_offset`, the 206/200/410
  reattach responses, `tee_stream`), the `/__oxide/stream/<id>` reattach
  route joining the A04 data plane (the flip `build.rs` reserved at A04),
  the `stream.resume` config section (DSP-112 + CFG-SIZE validation, JSON
  schema regenerated), and the client `resumableFetch`
  (`packages/core/src/client/resume.ts`: Range reattach, the spec's 5-try
  backoff ladder, `stream expired`/`stream lost`). One spec deviation: the
  progress wait is a `watch` channel, not `Notify::notify_waiters` (lost
  wakeups) — see `docs/deviations.md`. Tee + endpoint wiring deferred —
  see `docs/deviations.md`.

- **`odf` + `server` + `packages/core`**: the ODF patch deltas substrate
  (V2-F06, D30) — `odf::patch` (`Seg`/`Op`, structural `diff` with
  prefix/suffix array trimming, `apply`, the `ODP1` envelope around an
  ordinary ODF1 `{ b, o }` payload, `base_id` = truncated SHA-256 shared
  with the client), the 12 committed cross-language goldens
  (`crates/odf/tests/fixtures/patch/`), the sending policy on the live
  runner path (`server::live::delta_payload` + `Sub::prepare_rerun`: ship
  `ODP1` only while `patch.len()*2 < full.len()` and `ops.len() <= 64`), and
  the client patch transport (`decodeOdfOrPatch` as `connectLive`'s default
  decode; a refused base reconnects for a fresh snapshot). Spec deltas and
  deferrals — see `docs/deviations.md`.

- **`server` + `dispatch` + `router` + `render` + `config` + `adapters` +
  `packages/core`**: the live-loaders substrate (V2-F05) — the SSE
  framing/tag-hub/snapshot-stream/rerun-runner in `server::live`, the
  per-route `live` opt-in on `RouteEntry` (scanner + rkyv archive), the
  `live` config knobs (DSP-111), the `/__oxide/live` data-plane build
  contract (query `build` param), the five-field hydration bootstrap
  (`live:true`), the `Live` capability row, and the client
  `connectLive` (`packages/core/src/client/live.ts`). Endpoint + rerun +
  client-router wiring deferred — see `docs/deviations.md`.

- **`dispatch` + `cache` + `render` + `packages/core`**: the build id +
  generations substrate (V2-A04) — `cache::build_id_of_dist` computes the
  canonical id (blake3 over the sorted `path<TAB>hash` manifest of
  `dist/client/**`, `dist/server/**` and `.oxide/routes.manifest`),
  `dispatch::build` holds the `GenerationSet` acceptance window, the
  `stage_build_check` data-plane 409 gate and the `advertise_build` step,
  `render` advertises the id as the fourth `window.__oxide` field, and the
  core client (`build.ts`) stamps `x-oxide-build` on the `/__data`,
  action and server-fn fetches and reloads once per generation on a stale
  409. CLI wiring (computing the id at the end of `phase_emit`, installing
  the stage, promoting generations on reload) deferred — see
  `docs/deviations.md`.

- **`config`**: the `experiments` config section (V2-F04, §15.4) — typed
  `ExperimentConfig`/`StickyConfig` (internally-tagged `sticky`:
  `cookie`/`header`/`ip-hash`), DSP-110 load validation (traffic 0–100, ≥2
  variants), JSON schema + checked-in `schemas/oxide-fw-config.schema.json`
  regenerated. `config` also adopted the workspace pedantic-lints policy.
- **`dispatch`**: the §15.4 experiments assignment engine
  (`experiments::assign`/`resolve`) — deterministic blake3 buckets, sticky
  cookie (mint on miss)/header/ip-hash, route-prefix eligibility, and the
  id-sorted vary suffix the L0 key appends only when non-empty;
  `RequestLocals::set_experiments` feeds `Resolved.assigned` into the
  pipeline-owned slot (DSP-101 still guards it). CLI stage wiring deferred
  (`docs/deviations.md`).

- **`jshost`**: V8Host is no longer a fail-fast stub — `compile_module` runs
  the Oxc passes in-process (through `transform_handler`) so the V8 backend
  reaches the prerender path without a Node worker. See
  `crates/jshost/src/v8_ops.rs::compile_module_impl` and
  `packages/react/src/real-client.ts::mountReactPageRoot`.
- **`router`**: `RouteEntry.needs_parent` gates the §9.2 parallel layout-page
  load. A page whose `page.server.ts` never calls `parent()` runs its loader
  concurrently with the last layout's; pages that do stay sequential.
- **`cache`**: Redis and S3 L2 handlers are feature-gated (`oxide-cache/redis`,
  `oxide-cache/s3`). Both are wired through `remote_cache_handler` in
  `framework_shim` so `'use cache'` and the L0 response cache see the same
  remote tier.
- **`server`**: `ws_socket_upgrade` (the §9.6 WebSocket route handler) now
  treats `close: null` from the worker as "no close" instead of the previous
  implicit `Some((1000, ""))` — a client send after `open` was previously
  hitting a Close frame before the read loop was entered.
- **`cli` + TS client**: `/__data` carries the layout-id chain and
  `RealPageRoot.update` takes the new Component (not props only), so a
  client-side navigation actually swaps to the new route (previously it
  re-rendered the original page).
- **`tests/playwright`**: real-browser E2E over `oxide start` now covers
  smoke, middleware, island hydration, loading boundary, WebSocket, and SSE
  (14 specs).

## Route component resolution (§6.4, §7)

`oxide.config.ts` declares `routes.dir` and the adapter — nothing else. The
build's route scan owns the component graph:

- `RouteKind::{Page, Layout, Error, NotFound, Template}` all enter the SSR and
  client entry scans; canonical ids use the directory form
  (`routes/blog/[slug]`, no `/page`).
- `oxide build` emits `dist/server/ssr/index.js`
  (`export const routes = { "<id>": moduleNamespace }`) and
  `server.manifest.json`. The server manifest and the client manifest carry
  the same canonical keys; rows for error/not-found/template have
  `render_mode: null`.
- The worker imports the entry once per `moduleGeneration` and builds
  `input.resolve(id)`; the adapters (`real-ssr.ts`) call it for the page,
  layout chain and error route. A missing page is `OX-500`, never a silent
  skip.
- Dev has no bundle, so `cmd_dev` builds a canonical-id → source-file map
  (precompiling `.vue`/`.svelte`/Solid JSX into `.oxide/components/`) and
  hands it to the same resolver. HMR bumps the generation, which re-imports.
- `componentFor`/`layoutFor` remain as explicit advanced overrides; they are
  not part of the config surface and the smoke apps do not use them.

## Testing beyond unit tests

- `cargo test --workspace` — Rust crates
- `node --test packages/*/test/*.test.ts` — TS adapter + runtime packages
- `node --test conformance/test/*.test.ts` — 24 assertions × 4 adapters
- `pnpm --filter @oxide/playwright test` — real-browser E2E over
  `oxide start` (requires `pnpm --filter @oxide/playwright install-browsers`
  once, then a built `examples/smoke-app-react`)

## TypeScript packages

| Package | Owns |
| --- | --- |
| `oxide` | Server APIs (`defineLoad/Action/Meta`, `cache`, `serverFn`, `defer`, `redirect/error`), ODF JS decoder (byte-compatible with Rust fixtures), core client router (§8.4), island loader (§10.3) + interaction replay (§V2-F03), form `enhance`, `createTestApp` (§16.6) |
| `@oxide/{react,vue,svelte,solid}` | FrameworkAdapter implementations (§7): capabilities manifest, IR-based SSR with streaming boundary protocol, hydration, HMR strategy |
| `create-oxide` | Starters: 4 frameworks × minimal/blog/dashboard/storefront (§19.2) |
| `conformance` | Atlas reference app + one assertion set run against all four adapters (§22, D20) |

## Cross-cutting invariants

1. **Content addressing everywhere**: blake3 hashes CAS blobs (§5.4), chunks (§6.4),
   L0 keys (§11.2), server-fn ids (§9.4), etags, image cache keys, OG cache keys.
2. **Determinism**: sorted keys/segments before hashing; manifests and typegen are
   byte-stable across runs (tested).
3. **Errors are diagnostics**: every subsystem returns `OxideError(Diagnostic)` with a
   stable code; the same JSON shape crosses the JS boundary (§19.4).
4. **Trait isolation** (Appendix A): V8 (JsHost), Rolldown (Bundler), Oxc (Transformer),
   Lightning CSS (CssBackend), libvips (ImageBackend), fontations (FontBackend),
   wasmtime (WASM plugins) each have a working default engine or fail-fast stub behind
   the spec's own isolation point — swaps don't touch call sites.
