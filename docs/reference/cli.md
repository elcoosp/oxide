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

Migrate from next/nuxt/sveltekit/vite (§19.2)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |
| `<source>` |  |
| `--write <WRITE>` | Actually write the converted files. Default is dry-run (plan only) |
| `write = true` |  |
| `write = false` |  |

### `oxide test`

In-process test helper: boot the app and expose `fetch` (§16.6 oxide/test)

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |

### `oxide mcp`

MCP server over stdio (§19.3): exposes routes, cache, graph and diagnostics as MCP tools so coding agents see accurate project facts

| argument | description |
| --- | --- |
| `--root <ROOT>` |  |

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

### `oxide verify`

Verify an attested build (§V2-F20): the canonical form of the provenance, its ed25519 signature (with `--pubkey`) and every subject digest against the dist on disk. `--rebuild` re-runs the build in a scratch copy and compares the dist digests

| argument | description |
| --- | --- |
| `--root <ROOT>` | Project directory (default: cwd) |
| `--dist <DIST>` | The dist to verify (default: `<root>/dist`) |
| `--pubkey <PUBKEY>` | The ed25519 verifier key (SPKI PEM, 64-char hex or 32-byte base64). Without it the signature check is skipped loudly and the digests still verify |
| `--rebuild <REBUILD>` | Rebuild in a scratch copy of the project and compare the dist digests (the §V2-F20 reproducibility check) |
| `rebuild = true` |  |
| `rebuild = false` |  |
