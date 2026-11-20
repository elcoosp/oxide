<!-- GENERATED FILE — `oxide docs generate` (spec v0.2 §V2-E11). DO NOT EDIT.
Regenerate: cargo run -p oxide-fw-cli -- docs generate
Drift gate: crates/cli/tests/docs_tdd.rs -->

# CLI reference

Generated from the `oxide-fw-cli` clap command tree — the same about/flag text `oxide --help` prints.

## Commands

### `oxide dev`

Start the dev server (on-demand ESM, HMR, §13)

| argument | description |
| --- | --- |
| `--root <ROOT>` | Project directory (default: cwd) |
| `--port <PORT>` |  |
| `--host <HOST>` | Bind all interfaces + print the LAN URL (§13.5) |
| `host = true` |  |
| `host = false` |  |
| `--https <HTTPS>` | Serve over HTTPS with a self-signed cert (§13.5) |
| `https = true` |  |
| `https = false` |  |
| `--qr <QR>` | Print the network URL as a QR code (§13.5) |
| `qr = true` |  |
| `qr = false` |  |
| `--open <OPEN>` | Open the browser on start (§13.5) |
| `open = true` |  |
| `open = false` |  |
| `--proxy <PROXY>` | Reverse-proxy rule, repeatable: `PREFIX=TARGET` (e.g. `--proxy /api=http://localhost:4000`). Longest prefix wins (§13.5). Add a leading `!` to strip the prefix: `--proxy '!/api=http://x'` |
| `--no-esm <NO_ESM>` | §2.9: use the bundler path instead of the native ESM dev server (equivalent to `dev: { esm: false }` in `oxide.config.ts`). Serves `/client/*.js` from `dist/client/` instead of `/src/**` transformed modules |
| `no_esm = true` |  |
| `no_esm = false` |  |
| `--include-secrets <INCLUDE_SECRETS>` | V2-F15: when `record:` is on, keep the secret header trio (`authorization`, `cookie`, `set-cookie`) verbatim in the records instead of the `sha256:<12 hex>` redaction — a replay can then exercise authenticated paths. Dev-only; the production stack never reads it |
| `include_secrets = true` |  |
| `include_secrets = false` |  |
| `--log-feed-level <LEVEL>` | V2-F28: the dev-plane log feed's level floor, beside the `OXIDE_LOG_FEED_LEVEL` env var (the flag wins). One of the five `tracing` level names; the console stays byte-identical while `GET /__oxide/logs` captures the events the console never renders (V2-F26's widening composition, opted in from the boot flag) |
| `--trace-log <TRACE_LOG>` | V2-F22 polish tail: the dev-plane trace rows' export arm, beside the `OXIDE_TRACE_LOG` env var (the flag wins when present). Set, the boot attaches the `TracingTraceSink` beside the ring — every completed request's row ALSO leaves as one `tracing::info!` event (console, `OXIDE_LOG=json`, and with `OTEL_EXPORTER_OTLP_ENDPOINT` the OTLP stream), while the endpoint keeps serving the ring. Absent, the env var keeps its spelling (the falsy spellings `""`/`0`/`false`/`off` stay off). Default off either way: the console output stays byte-identical |
| `trace_log = true` |  |
| `trace_log = false` |  |
| `--cache-hot-key-top <N>` | V2-F42: the §20 scrape's hot-key top window, beside the `OXIDE_CACHE_PLANE_HOT_KEY_TOP` env var (the flag wins). The scrape's top-n is operator-tunable at the boot — the value rides pre-runtime into the override slot (the `log_feed_level` shape), the F41 resolver keeps its falsy and clamp contracts, and every scrape renders the boot's window |
| `--cache-plane-coverage <CACHE_PLANE_COVERAGE>` | V2-F46: the §20 scrape's coverage arm opt-in, beside the `OXIDE_CACHE_PLANE_COVERAGE` env var (the flag wins). The hot-key ledger's F44 coverage exposition — the unattributed score's counter, the occupancy and bound gauges — renders beside the labeled window family when the boot opts in; the value rides pre-runtime into the override slot (the `cache_hot_key_top` shape). Default off: the scrape's shape stays byte-identical |
| `cache_plane_coverage = true` |  |
| `cache_plane_coverage = false` |  |
| `--cache-plane-health <CACHE_PLANE_HEALTH>` | V2-F48: the §20 scrape's health arm opt-in, beside the `OXIDE_CACHE_PLANE_HEALTH` env var (the flag wins). The tier's F32 posture exposition — the nine `oxide_cache_plane_*` gauge families the `TieredCache::health()` pull samples — renders after the labeled and coverage families when the boot opts in; the value rides pre-runtime into the override slot (the `cache_plane_coverage` shape). Default off: the scrape's shape stays byte-identical |
| `cache_plane_health = true` |  |
| `cache_plane_health = false` |  |
| `--cache-posture-path <PATH>` | V2-F50: the posture probe's path, beside the `OXIDE_CACHE_POSTURE_PATH` env var (the flag wins). The F49 probe is a production surface (the F48 handoff's recorded tail), so the flag rides the dev boot beside the serve boot (the `cache_plane_health` shape). A `/`-led path relocates the probe; the falsy spellings (`""`/`0`/`false`/`off`) spell the surface OFF; garbage degrades loudly to the recorded default (`/-/health`) |
| `--cache-plane-mount <CACHE_PLANE_MOUNT>` | V2-F59: the F57 cache plane's mount opt-in, beside the `OXIDE_CACHE_PLANE_MOUNT` env var (the flag wins). The boot composes `HttpCacheService` over its own tier — the §11.2 shared plane answers `/entry/`, `/tag/`, `/invalidate/` and (the tier doubling as the F58 provider) `GET /health` on the site's own socket; the value rides pre-runtime into the override slot (the `cache_plane_coverage` shape). Default off: the boot's shape stays byte-identical |
| `cache_plane_mount = true` |  |
| `cache_plane_mount = false` |  |

### `oxide build`

Build for production (all environments + manifests, §6)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `--static <STATIC>` | Static export (§10.5) |
| `static = true` |  |
| `static = false` |  |
| `--explain <EXPLAIN>` | V2-R02 (spec §V2-R02): print the chosen target's §18 capability matrix row at the end of the build — which capabilities are Full, which Degraded (with the documented degradation string), which Unsupported |
| `explain = true` |  |
| `explain = false` |  |
| `--mode <MODE>` |  |
| `--offline-plugins <OFFLINE_PLUGINS>` | §17.2: deny outbound network from plugin/tooling workers |
| `offline_plugins = true` |  |
| `offline_plugins = false` |  |
| `--attest <ATTEST>` | §V2-F20: write `dist/build-id` and `dist/attest/` (the CycloneDX SBOM, the in-toto provenance and its ed25519 signature) for the dist this build emitted |
| `attest = true` |  |
| `attest = false` |  |
| `--key <KEY>` | §V2-F20: the ed25519 signing key for `--attest` (PKCS#8 PEM or DER). Overrides `OXIDE_SIGNING_KEY`; the build fails without a key rather than attesting unsigned |

### `oxide start`

Start the production server (binary target, §12)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `--port <PORT>` |  |
| `--no-warmup <NO_WARMUP>` | PERF-23 (readiness): skip the renderer warm-up. Warm-up costs ~1 s of startup (a synthetic render) so the first REAL request never pays the cold path; short-lived benchmark/CI runs and pre-scaled fleets use this to boot instantly |
| `no_warmup = true` |  |
| `no_warmup = false` |  |
| `--cache-hot-key-top <N>` | V2-F42: the §20 scrape's hot-key top window, beside the `OXIDE_CACHE_PLANE_HOT_KEY_TOP` env var (the flag wins). The scrape is a production surface, so the flag rides the serve boot too (the `root`/`port` multi-command shape) |
| `--cache-plane-coverage <CACHE_PLANE_COVERAGE>` | V2-F46: the §20 scrape's coverage arm opt-in, beside the `OXIDE_CACHE_PLANE_COVERAGE` env var (the flag wins). The scrape is a production surface, so the flag rides the serve boot too (the `root`/`port` multi-command shape). Default off: the scrape's shape stays byte-identical |
| `cache_plane_coverage = true` |  |
| `cache_plane_coverage = false` |  |
| `--cache-plane-health <CACHE_PLANE_HEALTH>` | V2-F48: the §20 scrape's health arm opt-in, beside the `OXIDE_CACHE_PLANE_HEALTH` env var (the flag wins). The scrape is a production surface, so the flag rides the serve boot too (the `root`/`port` multi-command shape). Default off: the scrape's shape stays byte-identical |
| `cache_plane_health = true` |  |
| `cache_plane_health = false` |  |
| `--cache-posture-path <PATH>` | V2-F50: the posture probe's path, beside the `OXIDE_CACHE_POSTURE_PATH` env var (the flag wins). The probe is a production surface, so the flag rides the serve boot too (the `root`/`port` multi-command shape). A `/`-led path relocates the probe; the falsy spellings spell the surface OFF; garbage degrades loudly to the recorded default |
| `--cache-plane-mount <CACHE_PLANE_MOUNT>` | V2-F59: the F57 cache plane's mount opt-in, beside the `OXIDE_CACHE_PLANE_MOUNT` env var (the flag wins). The mount is a production surface (the site AND the shared plane on one node), so the flag rides the serve boot too (the `root`/`port` multi-command shape). Default off: the boot's shape stays byte-identical |
| `cache_plane_mount = true` |  |
| `cache_plane_mount = false` |  |

### `oxide preview`

Preview the production build locally

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |

### `oxide routes`

Print the route table with inferred modes and cache policy (§10.1, §19.1)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |

### `oxide analyze`

Bundle and budget report (§21 budgets)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |

### `oxide check`

Typecheck (tsgo), lint (oxlint), config + route diagnostics (§19.1)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |

### `oxide fmt`

Format with `oxfmt`, falling back to `biome format` when absent (§V2-C11). No config key — the formatter is a CLI concern only

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `--check <CHECK>` | Check only: report files whose formatting differs and exit 1 instead of writing |
| `check = true` |  |
| `check = false` |  |

### `oxide why`

Explain importers and chunks of a module (§13.4), or — with `--route` — why a route is dynamic, with OX-DYN diagnostics (§V2-F16)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `<module>` | Module path whose importers are reported (the §13.4 surface) |
| `--route <ROUTE>` | Route id (e.g. `routes/dashboard`) whose dynamic reads are reported (the §V2-F16 surface). Mutually exclusive with the module argument |

### `oxide doctor`

Environment and config health (§19.1)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |

### `oxide secrets`

Manage the per-project build secret (§9.4 / BUG-05): `rotate` deletes `.oxide/secret` so the next build mints a fresh one (server-fn ids change — every cached client bundle referencing an old id is stale)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `<op>` |  |
| `op = rotate` | Delete `.oxide/secret`; the next build generates a new one |

### `oxide add`

Install a module (§15.4)

| argument | description |
| --- | --- |
| `--root <ROOT>` | Project directory (default: cwd) |
| `<module>` |  |

### `oxide generate`

Scaffold route, layout or action files

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `--kind <KIND>` |  |
| `<path>` |  |

### `oxide cache`

Cache inspection (§19.1)

| argument | description |
| --- | --- |
| `--root <ROOT>` | Project directory (default: cwd) |

### `oxide audit`

Plugin and dependency advisories (§17.2)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `--json <JSON>` | Emit the advisory report as JSON (a stable, machine-parseable schema: root, package_count, advisories, fatal, exit_code). Exit code is unchanged — a caller checking CI still gets 1 on HIGH/CRITICAL |
| `json = true` |  |
| `json = false` |  |

### `oxide migrate`

Migrate to oxide from a detected source framework (V2-E03): a dry run prints the plan; `--apply` performs the mechanical halves (renames + import rewrites) and requires a clean git tree unless `--force`. The framework is detected from the project's manifest and config signatures; an explicit `source` must match it

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `<source>` | The expected source framework (`sveltekit\|next\|nuxt\|vite`) — optional; given, it must match the detection |
| `--apply <APPLY>` | Apply the plan's mechanical halves (renames + import rewrites). A dry run never writes |
| `apply = true` |  |
| `apply = false` |  |
| `--force <FORCE>` | Apply even when the git tree is dirty (the clean-tree gate exists so a half-migration can be reverted) |
| `force = true` |  |
| `force = false` |  |

### `oxide replay`

V2-F15: re-execute a captured request deterministically and compare against its record (`.oxide/records/<id>.json`)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `<id>` | The request id the record is stored under (the file stem) |
| `--diff <DIFF>` | Compare the replayed response against the recorded one (nonce-normalized) |
| `diff = true` |  |
| `diff = false` |  |
| `--json <JSON>` | For agents: emit `{ ok, divergences: [..] }` and nothing else |
| `json = true` |  |
| `json = false` |  |

### `oxide test`

In-process test helper: boot the app and expose `fetch` (§16.6 oxide/test)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |

### `oxide mcp`

MCP server over stdio (§19.3): exposes routes, cache, graph and diagnostics as MCP tools so coding agents see accurate project facts. Read-only by default; `--allow-write` enables the write tools (V2-E07, the SEC-09 CLI-flag shape)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `--allow-write <ALLOW_WRITE>` | Enable the registry's write tools (`build.run`). Read-only by default — a write tool called without this flag is refused with a named refusal, not a protocol error |
| `allow_write = true` |  |
| `allow_write = false` |  |

### `oxide schema`

Print the JSON Schema for `oxide.config.{ts,json}` to stdout (§19.3). Editors (VS Code, JetBrains) consume this via a `$schema` field


### `oxide config`

Load oxide.config.{ts,mjs,js}, evaluate it in a Node worker, and print the framework adapter descriptor (spec §7). This is the load path the build/dev pipeline uses; today it proves end-to-end wiring of the §6.3 / §7 FrameworkHost

| argument | description |
| --- | --- |
| `<root>` | Project root that contains oxide.config.ts (default: cwd) |
| `--json <JSON>` | Emit the descriptor as JSON (machine-readable for CI / tooling) |
| `json = true` |  |
| `json = false` |  |

### `oxide render`

Render a route through the framework adapter and print the HTML (spec §7). Loads oxide.config.{ts,mjs,js}, dispatches a render call to the FrameworkAdapter the config exports, and writes the resulting HTML to stdout. This is the read-path proof for §6.3 / §7

| argument | description |
| --- | --- |
| `<route>` | Route id, e.g. `routes/index` or `routes/blog/[slug]` |
| `--root <ROOT>` | Project root containing oxide.config.ts (default: cwd) |
| `--url <URL>` | URL the request would have (default: derived from route) |
| `--param <KEY=VALUE>` | URL params as `k=v` pairs (repeatable). Arrays via `k=v1,k=v2` |
| `--data <DATA>` | Loader data as JSON (default: `{}`) |
| `--stream <STREAM>` | Stream the render to stdout as chunks arrive, instead of collecting first. Useful for large pages and for verifying that a framework adapter's streaming path works. Output bytes are identical to the non-streaming mode |
| `stream = true` |  |
| `stream = false` |  |

### `oxide perf`

Performance tooling (§V2-F18): `oxide perf bisect` hunts the commit that regressed a metric between two revisions

| argument | description |
| --- | --- |
| `--root <ROOT>` | Project directory (default: cwd) |

### `oxide docs`

Generated reference docs (§V2-E11): `oxide docs generate` writes `docs/reference/` from the crates the binary links — the §18 capability matrix, the config schema, the clap command tree — and `--check` verifies the on-disk pages (the CI `docs-check` gate)

| argument | description |
| --- | --- |
| `--root <ROOT>` | Project directory (default: cwd; the pages land under `<root>/docs/reference`) |

### `oxide deploy`

Verify an attested build (§V2-F20): the canonical form of the provenance, its ed25519 signature (with `--pubkey`) and every subject digest against the dist on disk. `--rebuild` re-runs the build in a scratch copy and compares the dist digests. Deploy the last build's `dist/` to a target (V2-F11): upload only what changed, swap the `current` pointer atomically. Targets: `dir:/path` (every build), `s3://bucket/prefix` / `r2://...` (S3 API; release builds with the `deploy-s3` feature)

| argument | description |
| --- | --- |
| `<target>` | The deploy target |
| `--root <ROOT>` |  |
| `--prune <PRUNE>` | Age off the store's pending deletions (`deploy.skewWindow`) and delete the superseded releases (the pointer's release is never pruned — the rollback-safety rule) |
| `prune = true` |  |
| `prune = false` |  |
| `--rollback <ROLLBACK>` | Swap the pointer back to the previous build (the one-write rollback) |
| `rollback = true` |  |
| `rollback = false` |  |

### `oxide verify`

| argument | description |
| --- | --- |
| `--root <ROOT>` | Project directory (default: cwd) |
| `--dist <DIST>` | The dist to verify (default: `<root>/dist`) |
| `--pubkey <PUBKEY>` | The ed25519 verifier key (SPKI PEM, 64-char hex or 32-byte base64). Without it the signature check is skipped loudly and the digests still verify |
| `--rebuild <REBUILD>` | Rebuild in a scratch copy of the project and compare the dist digests (the §V2-F20 reproducibility check) |
| `rebuild = true` |  |
| `rebuild = false` |  |
