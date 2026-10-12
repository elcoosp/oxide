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
| `odf` | Wire-format encoder/decoder, shared/ref tables, deferred slots, hardening limits | §9.5, §17.1, D11 |
| `router` | Route scanning (§8.1), radix matcher with priority rules (§8.2), rkyv archive (D8), typegen (§8.3), native rules engine | §8, §12.1, D8, D13 |
| `graph` | Demand-driven incremental engine, early cutoff (§5.1), tracked inputs (§5.2), CAS (blake3+redb+zstd, §5.4), cancellation (§5.6), RemoteCache trait | §5, D6 |
| `config` | Typed config, layering (defaults→preset→file→mode→env→CLI), env files, `PUBLIC_` split, static extraction with V8 fallback flag, typo diagnostics | §14, D14 |
| `cache` | L0/L1/L2 tiers, weighted LRU memory tier, redb disk tier, tags, TTL/SWR/stale-if-error, single-flight, build seeding, HTTP conditional semantics | §11 |
| `compiler` | Transform passes behind `Transformer` (D4): import scan, `'use cache'` (§9.3), server-fn stubs (§9.4), forbidden-import guard (§6.4), HMR injection | §6.2–6.4, §9 |
| `bundler` | `Bundler` trait (D3) + functional ESM bundler: resolution with `exports` conditions, route-based splitting, shared-chunk heuristic, content hashing, client/server manifests | §6.4 |
| `jshost` | `JsHost`/`Pool` traits (§4.2), NodeHost with real child workers speaking the op protocol (§4.5), ScriptedHost, V8Host fail-fast stub | §4, D2 |
| `plugin` | Hook model, pre/post ordering, JS bridge messages, WASM plugin manifest validation, module manifests | §15 |
| `render` | Streaming pipeline, head merge, HTML injection (lol_html), deferred-boundary protocol (§7.1), islands markup (§10.3), hydration bootstrap, error recovery | §7.1, §10 |
| `server` | Hyper 1.x pipeline, static assets (range/304/precompressed), dynamic compression, security defaults (§17.1), CSRF/SSRF/host checks, rate limiting, cookies, RFC6455 codec, graceful shutdown | §12, §17, D7 |
| `assets` | CSS pipeline (nesting/modules/minify/prefix), image backend (resize/encode/LQIP), font bundles, metadata/SEO, sitemap/robots, markdown/content, OG model | §6.6–6.7, §16, D19 |
| `dev` | Hash-confirmed watcher (§5.5), HMR message protocol + per-kind behavior table (§13.3), update boundaries, inspector model (§13.4), prebundle keys | §13 |
| `adapters` | Deploy targets + capability matrix + refusal rule, `_headers`/`_redirects`, Dockerfile, Worker/node/serverless entries | §18 |
| `core-wasm` | Edge subset: cache keys, head merge, ODF re-encode (D18) | §2 D18 |
| `cli` | All §19.1 commands: dev/build/start/preview/routes/analyze/check/why/doctor/add/generate/cache/audit/migrate/test | §19 |

## TypeScript packages

| Package | Owns |
| --- | --- |
| `oxide` | Server APIs (`defineLoad/Action/Meta`, `cache`, `serverFn`, `defer`, `redirect/error`), ODF JS decoder (byte-compatible with Rust fixtures), core client router (§8.4), island loader (§10.3), form `enhance`, `createTestApp` (§16.6) |
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
