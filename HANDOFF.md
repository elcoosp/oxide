# Handoff — V2-E04's security surface landed in both halves (`feat/security-sri-manifest`: the bundler records `sha384-` digests for every emitted client JS/CSS file in `client.manifest.json`'s `integrity` map, the CLI stamps the boot `<script type=module>` tag and render's `Injector` carries the map for preload/stylesheet links, `security.sri` on by default; `feat/csp-report-endpoint`: `server::csp_headers_for` is the one definition site for the strict/hash/report-only × Trusted Types header matrix and `POST /_oxide/csp-report` is the rate-limited violation sink over both browser formats); the typed `security.sri`/`security.trustedTypes` keys are schema'd and `CFG-SEC`-validated; all seven new TDD files are pedantic-clean

## Where things stand

**Branch:** `main` (the milestone ships as uncommitted changes plus
`ship.toml` / `.github/pr-body.md` at the repo root, both gitignored;
`ship.toml` groups the diff into conventional commits — one per file).
**Verified this session (rustc 1.99.1 stable, 2-core/4 GB/10 GB-disk
sandbox, `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0`, nasm in
`~/.local/bin` for rav1e, benches at dev profile — the release profile
does not fit the disk, the A07/E11-session precedent):**
- `cargo fmt --all --check` clean; the CI bar
  (`cargo clippy --workspace --all-targets --all-features -- -D warnings`)
  ZERO findings; the pedantic sweep over the milestone crates
  (`cargo clippy -p oxide-fw-bundler -p oxide-fw-config -p
  oxide-fw-render -p oxide-fw-server -p oxide-fw-cli --all-targets
  --all-features -- -W clippy::pedantic`) — the two lints-opted crates
  (`config`, `render`) print ZERO findings and every file this session
  added (`bundler/src/sri.rs`, the bench, the example,
  `config/tests/security_surface.rs`, `server/tests/csp_tdd.rs`, the
  render TDD/snapshot files) prints ZERO findings; the pre-existing
  bundler/server/cli pedantic debt still prints under the sweep,
  unchanged (those crates have not opted into `[lints] workspace =
  true` yet — the documented hygiene-pass order).
- Runtime suites green (all-features): `oxide-fw-config` 169 (24 lib +
  10 brownout + 7 experiments + 26 integration + 10 interaction_replay
  + 10 live + 9 refusal + 5 schema_stability + the six new
  `security_surface` pins + suites + doctests), `oxide-fw-render` 81
  (12 lib + 5 live_tdd + 5 interaction_replay + 46 render_tdd — the
  three new SRI injector pins — + 13 snapshots incl. the new
  `inject_into_html_with_sri`), `oxide-fw-server` 250 (56 lib + 18
  csp_tdd — the full V2-E04 CSP half matrix — + 18 server_tdd + 21
  status_sidecars + 27 integration + 83 governor + 9 inject + 18
  stream), `oxide-fw-bundler` 78 (31 lib incl. the four `sri` tests +
  3 refusal + 3 jsx + 16 rolldown + 24 bundler_tdd + integration),
  `oxide-fw-cli` 281 lib (0 failed, 12 ignored — the three new
  `boot_script_tag`/`manifest_integrity` pins included); the
  V2-E11 gates re-validated after the schema/registry growth:
  `docs generate --check` over the regenerated `docs/reference/`
  pages — "up to date (4 pages)" — and `errors_registry_tdd` 9 (the
  coverage test holds against the new `CFG-SEC` row).
- Downstream crates re-run green after the config/render/server
  surface changes (all-features): diagnostics 5+2, schema 15, graph
  27, router 79+1, odf 19+25+1+2, dispatch 26+16+5, adapters 48+56+14,
  compiler 28+22, cache 129+9+3+11, dev 38+9+35, assets 54+21+7+2.
- The zero-`#[allow]` gate: `grep -rnE '#!\[(allow|expect)\(|cfg_attr\([^\)]*_allow\('`
  over `crates/ tests/ benches/ examples/` — empty.
- Benches run under the dev profile (`--quick`): the new
  `sri_digest/{4096,65536}` (~331 µs / ~5.1 ms) + `integrity_map/10`
  (~15 ms, the rolldown read-back walk) in `crates/bundler/benches/sri.rs`
  behind `bench-util`; `governor` and `bootstrap` re-run for the
  touched crates. The cli benches compile-verified by the
  `--all-targets` clippy check (their release-profile test binaries do
  not fit the sandbox disk — the A07-session precedent).
- Doc-drift fixes IN this diff: the spec's V2.O table (the E04 partial
  row), `docs/architecture.md` (the bundler/render/server/cli rows +
  the what-changed bullet), `docs/deviations.md` (the E04 record: the
  link-integrity wiring state, the client TT half, the mismatch-case
  documentation, the strict-vs-lenient rejection posture),
  `docs/errors.toml` (the `CFG-SEC` row), the regenerated
  `docs/reference/{config,errors}.md` pages, and `HANDOFF.md` (this
  header). `README.md`'s badge re-measure is deferred to the next
  session that runs `scripts/count-tests.sh` end-to-end (the sandbox
  could not rebuild the docs site; the count is unchanged by the Rust
  suites listed above).

## What landed

**M1 — the SRI manifest seam** (`feat/security-sri-manifest`):
`crates/bundler/src/sri.rs` — `sri_digest` (the `sha384-<standard
base64>` form, known-answer-pinned against an independent vector) and
`integrity_map` (the rolldown path's read-back walk; a file that fails
to read is skipped with a `tracing::warn!`). `ClientManifest.integrity`
(`#[serde(default)]` keeps pre-E04 manifests parsing) rides both
backends: the chunk bundler digests the bytes it already holds (no
re-read, byte-exact with what was written), the rolldown backend reads
the persisted files back. `oxide-fw-render`'s `Injector` gained the
`integrity` map (`with_integrity`): modulepreload/stylesheet links
whose URL resolves to a digest emit `integrity` + `crossorigin=
"anonymous"`, everything else stays byte-identical (snapshot-pinned).
The CLI's `boot_script_tag` is the one definition site for the boot
tag on the prerender and live-SSR paths — with a manifest digest and
`security.sri` on (the default) the tag gains `integrity` +
`crossorigin`; the dev `/@boot/` virtual module and `sri: false` keep
the pre-E04 bytes. Pins: `render_tdd` 3 + the snapshot, `cli/tests`
3, `sri.rs` 4. Bench: `crates/bundler/benches/sri.rs`. Example:
`crates/bundler/examples/sri_integrity.rs` (runs: known answer → a
real §6.4-fixture bundle → the manifest's integrity map → an
independent recompute).

**M2 — the CSP report-only half** (`feat/csp-report-endpoint`):
`server::tt_directives` + `server::csp_headers_for` — the one
definition site for the whole header matrix: `strict`/`hash` emit the
enforced policy (TT `enforce` appends the directives; TT `report`
keeps the enforced header byte-identical and rides an additional
Report-Only header); `report-only` sends the strict policy +
`report-to` on `Content-Security-Policy-Report-Only` plus the
`Reporting-Endpoints` group the sink backs (TT `enforce` adds the
minimal enforced TT-only header; TT `report` a second Report-Only
one); `off` emits nothing unless TT is on. `server::csp_report_response`
+ the `/_oxide/csp-report` dispatch arm: both browser formats
(classic `application/csp-report` and Reporting API
`application/reports+json`), rate limited 20/min/IP on the
trusted-proxy-aware peer key (the pipeline's own
`csp_report_rate_limiter`, never shared with the OG bucket), a 64 KiB
body cap (413), 415 for unrecognized content types, 400 for
unparseable JSON, 405 for non-POST, and the structured `tracing`
event over a whitelist of fields — no cookies, no raw body. The typed
`security.sri`/`security.trustedTypes` keys (config, with the
`report-only` CSP spelling) are schema'd with descriptions (the E11
DOC-001 gate) and load-time-validated (`CFG-SEC` names a bad spelling
with the valid modes as the fix — the DSP-113 knob-validator pattern).

**Additive in the same diff:** the `CFG-SEC` registry row
(`docs/errors.toml`), the regenerated reference pages, the spec/arch/
deviations doc updates listed above, and the 27-file conventional
commit split in `ship.toml`.

## Where to pick up, in one sentence

The client Trusted Types half is the remaining E04 work (`packages/
oxide`: the `tt.createHTML` policy, the `innerHTML` walk, the `rg`
lint test — see `docs/deviations.md`); F09's engine is still the
remaining half (`crates/search`: tantivy index, `/_oxide/search`, the
static matcher); E11's remaining half is the docs-site app
(`examples/docs` over `generate_all()`) plus `endpoints.md` (the V2.J
table's open rows gate it) and the E05-dependent `plugin-compat.md`;
an `oxide verify` walk of `integrity` would close the mismatch-case
loop server-side; and the adapter-level head producer that fills
`Injector.preload`/`css_links` turns the link-integrity seam from
wired-and-ready to end-to-end.

# Handoff — V2-E11's reference-docs generator + the error-code registry landed (`oxide docs generate` writes docs/reference/ from the linked crates with the `--check` CI gate; docs/errors.toml is the 197-entry V2.K registry the coverage test holds against every production code literal); both milestone crates are pedantic-clean

## Where things stand

**Branch:** `main` (the milestone ships as uncommitted changes plus
`ship.toml` / `.github/pr-body.md` at the repo root, both gitignored;
`ship.toml` groups the diff into conventional commits — one per file).
**Verified this session (rustc 1.99.1 stable, 2-core/4 GB/10 GB-disk
sandbox, `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0`, nasm built
from source into `~/.local` for rav1e):**
- `cargo fmt --all --check` clean; `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` (the CI bar) ZERO
  findings; the pedantic sweep over the milestone crates
  (`cargo clippy -p oxide-fw-cli -p oxide-fw-config --all-targets
  --all-features -- -D warnings -W clippy::pedantic`) ZERO findings —
  `docs_gen.rs` and the two TDD files included; the pre-existing
  server/cache/odf pedantic debt still prints under the sweep,
  unchanged.
- Runtime suites green (all-features): `oxide-fw-cli` lib 278 (0
  failed, 12 ignored) + `docs_tdd` 10 + `errors_registry_tdd` 6 (the
  16 new pins) + `attest_tdd` 25 + `perf_bisect_tdd` + `integration`;
  `oxide-fw-config` 115 (the 46 schema-description additions regenerate
  `schemas/oxide-fw-config.schema.json`, `schema_stability` green);
  adapters, dev, dispatch, render, router, schema, diagnostics, odf,
  graph, cache, server, assets, compiler green; bundler/jshost/
  core-wasm compile-verified only (their test binaries did not fit the
  sandbox disk — the A07-session precedent).
- The `docs_generate` criterion group (dev-profile numbers, the
  sandbox's release profile did not fit the disk): capabilities 54 µs,
  cli 667 µs, config 1.19 ms, errors 757 µs — a whole-page render is
  milliseconds, the docs path's worst case.
- The zero-`#[allow]` gate: `grep -rnE '#!\[(allow|expect)\(|cfg_attr\([^)]*_allow\('`
  over `crates/ tests/ benches/ examples/` — empty.

## What landed

**M1 — the E11 generator seam** (`feat/docs-generate-seam`):
`crates/cli/src/docs_gen.rs` — the `ReferenceGenerator` trait (stable
`path()` + deterministic `render()`), the static registry
(`generators()`), `generate_all()`, the read-only `check()` (`DOC-003`
findings, the `check_config` shape applied to docs). Three pages:
`capabilities.md` (a fold of `capability_markdown_table` + the
`degradation` reasons — the matrix stays the single source of truth),
`config.md` (a walk of `config_schema()`, refusing `DOC-001` when any
leaf key lacks a description — the E11 acceptance, which retired the 46
bare schema keys in `crates/config/src/schema.rs` + the regenerated
`schemas/oxide-fw-config.schema.json`), `cli.md` (the live clap tree).
`oxide docs generate` (`Command::Docs`/`DocsOp`) writes
`<root>/docs/reference`; `--check` is the gate the new CI `docs-check`
job runs. Pins: `tests/docs_tdd.rs` (10). Bench: `benches/docs.rs`
behind `bench-util`. Example: `examples/docs_generate.rs`.

**M2 — the registry seam** (`feat/errors-registry-page`):
`docs/errors.toml` — 197 entries (one per stable code; messages
authored from the call sites, `fix` where one exists, `spec_only` for
the seven not-yet-landed V2.K additions), the strict
`parse_registry` (`DOC-002` names the line), the `errors.md` page
(landed first, spec-only marked), and
`tests/errors_registry_tdd.rs::registry_covers_every_production_code_literal`
— every code-shaped literal in production sources must have a registry
row (the V2.K `rg` gate as a test; it caught `I18N-LOAD` and
`ATT-KEYFMT` during bring-up). Pins: 6.

**Additive / doc-drift retired in the same diff:** the 46 schema
descriptions (above); `crates/cli/README.md` gains the `docs generate`
bullet; `docs/architecture.md` the cli row + the what-changed bullet;
the spec's V2.O E11 row; `README.md`'s badge re-measured 1,831→1,847
via `scripts/count-tests.sh`; `HANDOFF.md` (this header).

## Where to pick up, in one sentence

F09's engine is still the remaining half (`crates/search`: tantivy
index, `/_oxide/search`, the static matcher); E11's remaining half is
the docs-site app (`examples/docs` over `generate_all()`) plus
`endpoints.md` (the V2.J table's open rows gate it) and the
E05-dependent `plugin-compat.md`; F08's loader flips `rust_fns_present`
from the walk to the real compile step.

---

# Handoff — V2-F09's `search` key + the §18 refusal's `AST-SRC-001` arm landed (the fourth and last open code of the config-key refusal rule is live; edge refuses, Node/serverless/static warn the documented string), and E11's docs-table seam realized (`capability_markdown_table` + the byte-identical README embed); both milestone crates are pedantic-clean

## Where things stand

**Branch:** `main` (the milestone ships as uncommitted changes plus
`ship.toml` / `.github/pr-body.md` at the repo root, both gitignored;
`ship.toml` groups the diff into conventional commits — one per file).
**Verified this session (rustc 1.99.1 stable, 2-core/4 GB/10 GB-disk
sandbox, `CARGO_PROFILE_DEV_DEBUG=0`):**
- `cargo fmt --all --check` clean; the CI bar
  (`cargo clippy --workspace --all-targets --all-features -- -D warnings`)
  ZERO findings; the pedantic sweep over the milestone crates
  (`cargo clippy -p oxide-fw-adapters -p oxide-fw-config --all-targets
  --all-features -- -D warnings -W clippy::pedantic`) ZERO findings.
- Runtime suites green (all-features): `oxide-fw-adapters` 78 (26 lib +
  16 config_refusal — the `refuses_search_on_edge` /
  `warns_degraded_search_on_node` / `allows_search_on_native_targets`
  pins, the search-aware order and matrix-agreement pins + 5 doc_table +
  5 deploy_matrix + 2 integration + 4 live_matrix + 9 matrix_v2 +
  6 snapshots + 2 doctests), `oxide-fw-config` 115 (24 lib + 10
  refusal_surface — the `search_configured` mapping pins — + 7
  schema_stability incl. `schema_declares_the_search_keys` + the six
  suites + doctests), `oxide-fw-cli` 278 lib (0 failed, 12 ignored —
  `the_search_key_refusal_rides_the_build_wire` included) and 25/25 in
  `attest_tdd` (the repaired pre-existing pins).
- The zero-`#[allow]` gate: `grep -rnE '#!\[(allow|expect)\(|cfg_attr\([^)]*_allow\('`
  over `crates/ tests/ benches/ examples/ build.rs` — empty.
- Benches compile and run under `cargo test --benches --all-features`
  (`config_refusal` gains the `search_configured` split; new `doc_table`
  group behind `bench-util`).
- Doc-drift fixes IN this diff: `crates/config/README.md` (the
  `feature_report` bullet names the config-KEY surfaces + the new
  consumer example), `crates/adapters/README.md` (the generated
  capability table + the AST-SRC-001 code), the spec's V2.O table (the
  F09/E11 partial rows), `docs/architecture.md` (the adapters row + the
  what-changed bullet), `docs/deviations.md` (the session record),
  `README.md` (the badge re-measured 1,818→1,831 via
  `scripts/count-tests.sh`), and `HANDOFF.md` (this header).

## What landed

**M1 — F09's `search` key + the `AST-SRC-001` arm**
(`feat/search-key-refusal-arm`): the typed key over the full V2.L shape
(`SearchConfig`/`SearchFieldsConfig` in `crates/config/src/lib.rs`,
schema'd in `search_schema()` + the regenerated
`schemas/oxide-fw-config.schema.json`, `KNOWN_KEYS` in
`crates/config/src/layer.rs`), `ConfigFeatureReport::search_configured()`
as a REQUIRED trait method (the compiler walks every impl — the config
crate's mapping reads `search.collections` only; the §16.5 static-JSON
collection indexes never refuse, the R02 reading unchanged) and the
`check_config` search arm in capability declaration order (live → rust
fns → search → the target-independent fonts rule): `Unsupported` (edge)
→ the registry error + fix text ("Use static JSON search or a Binary or
Docker target"), `Degraded` (node/serverless/static) → the warning with
the documented string. The key is inert until F09's engine lands — the
RFN-002 pattern (enforcement live from day one); the engine consumes the
unchanged query knobs. Pins: 3 adapters + 3 config + 1 CLI + 1 schema.

**M2 — the E11 docs-table seam** (`feat/matrix-doc-table`):
`capability_markdown_table()` — the §18 matrix as a deterministic
markdown table (rows = `Target::ALL`, columns = `Capability::ALL`, cells
`F`/`D`/`U`), a fold of `support` like `capability_matrix`; the crate
README embeds the exact output and
`tests/doc_table_tdd.rs::doc_table_matches_the_function` fails when they
drift (the E11 pin realized at the crate-docs level). The `doc_table`
criterion bench behind `bench-util` and the extended `capability_matrix`
walkthrough ride along.

**Micro-debt / additive items retired (not milestone substitutes):**
`crates/config` had ZERO examples — `examples/feature_report.rs` is the
crate's first consumer example (typed config → `feature_report` →
`check_config`, including the static-JSON-never-refuses subtlety); the
config README's `feature_report` bullet now names both config-KEY
surfaces; the schema test lists carry `search` (the BUG-11 drift guard);
and two PRE-EXISTING breakages from the previous delivery snapshot were
repaired (verified failing on the pristine base via stash):
`crates/cli/src/attest.rs`'s `dist_subjects` walk emitted a duplicate
`dist/build-id` subject, and `crates/cli/tests/attest_tdd.rs`'s
`pem_public_key` helper wrapped the bare key instead of a real RFC 8410
SPKI DER — `attest_tdd` is 25/25 green again.

## Where to pick up, in one sentence

F09's engine is the remaining half (`crates/search`: tantivy index,
`/_oxide/search`, the static matcher — the typed key and the schema are
already its contract); F08's loader flips `rust_fns_present` from the
walk to the real compile step; the E01 remainder (Vue/Solid refresh
wiring) is still the V8-host job it was; E11's docs-site generator
(`capabilities.md` from the same function) supersedes the README embed.

---

# Handoff — V2-R02's config-key refusal half landed (the §18 refusal rule now consults the config layer: the `ConfigFeatureReport` seam + `check_config` refuse `AST-LIV-001`/`RFN-002`/`AST-FNT-001` before any deploy-artifact work, the config crate maps `fonts.subset: 'used'` and the driver's `ProjectFacts`, and the deferred presentation half — `oxide doctor`'s per-capability column + `build --explain`'s matrix row — rides the same diff); both milestone crates are pedantic-clean

## Where things stand

**Branch:** `main` (the milestone ships as uncommitted changes plus
`ship.toml` / `.github/pr-body.md` at the repo root, both gitignored;
`ship.toml` groups the diff into conventional commits — one per file).
**Verified this session (rustc 1.99.1 stable, 2-core/4 GB/10 GB-disk
sandbox, `CARGO_PROFILE_DEV_DEBUG=0` — the debug rlibs do not fit the
disk otherwise):**
- `cargo fmt --all --check` clean; `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` (the CI bar) ZERO findings;
  `cargo clippy -p oxide-fw-adapters -p oxide-fw-config --all-targets
  --all-features -- -D warnings -W clippy::pedantic` ZERO findings. The
  other four `[lints] workspace = true` crates (dispatch, render, router,
  schema) show zero findings of their own under the same sweep (the
  pre-existing server/cache/odf debt still prints, unchanged).
- Runtime suites green (all-features): `oxide-fw-adapters` 65 (26 lib +
  13 config_refusal + 5 deploy_matrix + 2 integration + 4 live_matrix +
  9 matrix_v2 + 6 snapshots — the matrix_v2 file carries the 3
  capability-id pins), `oxide-fw-config` 56 (24 lib + 6 refusal_surface +
  the six suites), `oxide-fw-cli` 277 lib (0 failed, 12 ignored — the
  facts/refusal/matrix-row pins included), `oxide-fw-dev` 84, server,
  diagnostics, dispatch, render, router, schema, odf, graph, cache,
  plugin, assets, compiler all green. bundler/jshost/core-wasm are
  compile-verified only (the workspace check + clippy pass) — their test
  binaries did not fit the sandbox disk, the A07-session precedent.
- The `config_refusal` criterion group runs behind `bench-util` (measured:
  2.47 µs for the all-surfaces sweep over the twelve targets, 95 ns for
  the empty report) and the `config_refusal` walkthrough example prints
  the per-target verdicts.
- The zero-`#[allow]` gate: `grep -rnE '#!\[(allow|expect)\(|cfg_attr\([^)]*_allow\('`
  over `crates/ tests/ benches/ examples/ build.rs` — empty.
- Doc-drift fixes IN this diff: `crates/config/README.md` described a
  `load(root, env, env_vars)` signature the crate never had (the real one
  is `load(dir, mode, cli_overrides)`) and omitted the refusal mapping;
  `crates/adapters/README.md` gains `check_config`/`as_str`; the spec's
  V2.O table (R02's config half merged), `docs/architecture.md` (the
  adapters row + the what-changed bullet), `docs/deviations.md` (the
  config-half record), `README.md` (the badge re-measured 1,789→1,818 via
  `scripts/count-tests.sh`), and `HANDOFF.md` (this header).

## What landed

**M1 — R02's config-key refusal half, the §18 seam** (`feat/
config-key-refusal`): `crates/adapters/src/refusal.rs` — the
`ConfigFeatureReport` trait (live routes, `*.server.rs` presence,
`fonts.subset: 'used'`, non-static routes) + `check_config(report,
target)`, pure and infallible, every verdict re-stating
`support(target, capability)` as a `Diagnostic` so the matrix stays the
single source of truth: `Unsupported` → registry-code errors with the
V2.K fix text, `Degraded` → warnings carrying the documented degradation
string, findings in a deterministic order. `crates/config/src/refusal.rs`
— `ProjectFacts` (the driver's facts) + `OxideConfig::feature_report`
(the key mapping). The CLI build computes the facts from the scanned
route tree (`RouteEntry::live`, the effective §10.1 modes — the same rule
the enriched manifest applies) plus a `**/*.server.rs` source walk and
refuses BEFORE any deploy-artifact work; warnings print as `warn: [code]`
with a `tracing::warn`. Pins: 13 adapters (`tests/config_refusal_tdd.rs`)
+ 6 config (`tests/refusal_surface.rs`) + 4 CLI. The dependency direction
is the recorded V2-R02 ruling (config → adapters; the adapters crate
still never sees `oxide.config.ts`); `AST-SRC-001` has no arm until F09
introduces the `search` key (the deviation record explains why the
collection indexes must NOT refuse).

**M2 — the R02 presentation half** (`feat/doctor-capability-row`):
`Capability::as_str()` kebab-case ids (pinned v0.1- and v0.2-stable),
`capability_row_lines(target)` in the CLI, `oxide doctor`'s informational
per-capability section and `oxide build --explain`'s end-of-build matrix
row. 6 pins (3 adapters ids + 3 CLI rows/doctor).

**Micro-debt retired (in-diff, additive — not milestone substitutes):**
the adapters crate's remaining pedantic findings — the
`capability_matrix` example's `print_literal`/uninlined-args, the
102-line `v02_rows_match_the_spec` lifted over `too_many_lines` (the
spec table extracted to `v02_spec_checks`), the bench-file
`semicolon_if_nothing_returned`/`must_use` nits; plus the config
README's signature drift.

## Where to pick up, in one sentence

F09's `search` key gains `search_configured()` + the `AST-SRC-001` arm +
the spec's `warns_degraded_search_on_node` pin (the refusal seam is one
method and one arm away); F08's loader flips `rust_fns_present` from the
walk to the real compile step; the E01 remainder (Vue/Solid refresh
wiring) is still the V8-host job it was.

---

# Handoff — V2-E01 phase-4 state preservation + V2-R02 capability-matrix v2 landed (the third HMR acceptance is a wire contract: the v1 `update` frame carries a `preserve` verdict, and the client snapshots/restores around the swap); both milestone crates are pedantic-clean under a command-line `-W clippy::pedantic` sweep

## Where things stand

**Branch:** `main` (the milestone ships as uncommitted changes plus
`ship.toml` / `.github/pr-body.md` at the repo root, both gitignored;
`ship.toml` groups the diff into conventional commits — one per
reviewer-visible intent — for the ship tooling to materialize).
**Verified this session (rustc 1.99.1 stable, 2-core/4 GB/10 GB-disk sandbox):**
- `cargo fmt --all --check` clean; `cargo clippy -p oxide-fw-dev -p
  oxide-fw-adapters --all-targets --all-features -- -D warnings -W
  clippy::pedantic` ZERO findings in both milestone crates (a first for
  `adapters`: the sweep's pre-existing findings in that crate — missing
  `# Errors` docs, `format!`-push, `must_use` candidates, `too_many_lines`
  on the 169-line edge template, `match_same_arms`/`unnested_or_patterns`
  against the old table — were retired in this diff, not suppressed; the
  untouched `graph` crate holds the remaining pedantic debt). The CI bar
  holds workspace-wide by policy; `cargo clippy -p oxide-fw-cli
  --all-targets -- -D warnings` is the compile-verified gate for the
  V8-linked crate (see the not-run list in `.github/pr-body.md`).
- Runtime suites green: `oxide-fw-dev` 84 (54 lib + 21 boundary + 7
  state + 2 doctests), `oxide-fw-adapters` 52 (26 lib + 5 deploy_matrix
  + 2 integration + 4 live_matrix + 6 matrix_v2 + 9 snapshots),
  `packages/core` node --test 325/325 incl. the 9 new `hmr-state` pins and the 2 boot-restore pins (the ESM-dev probe reads `window.document` while the boot flow reads `globalThis.document` — the fixture fills both);
  the `state_preservation` criterion group runs behind `bench-util` and
  the `hmr_state` walkthrough example prints the keep/clear verdicts.
- The zero-`#[allow]` gate: `grep -rnE '#!\[(allow|expect)\(|cfg_attr\([^)]*_allow\('`
  over `src/ crates/ tests/ examples/ build.rs` — empty.
- Doc-drift fixes IN this diff: `crates/adapters/README.md` described a
  `FrameworkRenderer`/`AdapterPageRenderer` API this crate has never
  exported (the render adapters live in `oxide-fw-jshost`) — rewritten
  around the real surface (Target/Capability/support/capability_matrix/
  degradation/emit/check_requirements); the spec's V2.O table (R02
  merged; E01's phase-4 record), `docs/architecture.md` (the dev +
  adapters rows and the what-changed bullets), `docs/deviations.md` (the
  two new records), `README.md` (the badge re-measured 1,775→1,789 via
  `scripts/count-tests.sh`), and `HANDOFF.md` (this header).

## What landed

**M1 — V2-E01 phase 4, the state-preservation substrate** (`feat/
hmr-state-preservation`): `crates/dev/src/state.rs` — `Preservation`
(`Keep` / `Clear{reason}`, internally tagged on `policy`), the
injectable `StatePolicy` trait + `DefaultStatePolicy` (boundary Update →
keep, FullReload → clear with the planner's reason verbatim),
`update_frame(&ClassifiedPlan, &dyn StatePolicy, updates)` as the single
frame-construction seam under the `hmr_state_preservation` span. The
wire: `HmrMessage::Update` gains `preserve` behind `#[serde(default)]`
(a pre-substrate frame decodes as `Keep` — additive, no schema bump).
The CLI fan-out stamps every update frame through `update_frame`
(tailwind rebuild, both /@css/ swaps, the non-ESM swap, the data-entry
fallback, the §3.2 batch) and a tick-scoped `tick_reload_reason`
downgrades the batch verdict to `clear` when any event in the same
watcher tick planned a full reload — the two frames then carry the SAME
reason. The TS client (`packages/core/src/client/hmr.ts` +
`boot.ts`): `snapshotFormState`/`restoreFormState` (fields keyed
name → id → ordinal path + focus, restore idempotent, gated on the
async applies via `Promise.allSettled`), the `ox-hmr-state`
sessionStorage stash across a fallback reload (10 s TTL, cleared on
restore), `scheduleStashRestore` on the ESM-dev boot path. Pins: 7 Rust
(`tests/state_tdd.rs`) + 9 TS (`packages/core/test/hmr-state.test.ts`);
the `state_preservation` criterion group behind `bench-util`; the
`hmr_state` walkthrough example. 1,775→1,789 Rust test functions.

**M2 — V2-R02, the capability-matrix v2** (`feat/capability-matrix-v2`):
`crates/adapters/src/lib.rs` — the fixed-size `row([...; 8])` table is
now `support(target, capability)`, a total function over the private
`Class` mapping (one arm per capability, exhaustive inner match on the
class — both dimensions compiler-proved); `Capability::ALL` gains the
ten v0.2 rows (`MixedIslands`, `Resume`, `Skew`, `DeltaDeploy`,
`RustFns`, `Search`, `Offline`, `Brownout`, `Permissions`,
`Attestation`); every new `Degraded` cell documents its reason in
`degradation` (matrix-wide pin); the §18 refusal rule extends to the
v0.2 `requires` keys. `capability_matrix` is the fold over `ALL`, so map
and function cannot disagree. The golden pin holds the v0.1 rows
byte-identical (the R02 sketch's Edge-Live `U` corrected to the landed
F05 `D` — byte-identical beats the sketch; recorded in `support`'s
comment and the deviation record). 6 new pins in
`tests/matrix_v2_tdd.rs`; the `capability_matrix` walkthrough example.

**Micro-debt retired (in-diff, additive — not milestone substitutes):**
the adapters crate's pedantic findings (27 lib + 6 src-tests + 4
test-target findings): `# Errors` on `emit`, `#[must_use]` on the eleven
pure emitters/getters, `write!`-over-`format!-push` in the
headers/redirects/wrangler emitters, the 169-line edge template lifted
to `EDGE_FALLBACK_JS`, doc backticks (`NodeLike`, `WinterCG`,
`NodeHost`, `serde_json`), `assert_ne!` over `assert!(!is_empty)`,
uninlined format args, items-after-statements, `map_or`/`is_some_and`
over `map+unwrap_or`, `expect_err` over `.err().expect`,
`needless_raw_string_hashes`, and two `type_complexity` aliases in the
new pin file.

## Where to pick up, in one sentence

The E01 remainder is the Vue/Solid refresh wiring (tooling-pool compile
step + the adapters' client runtimes — a V8-host job), R02's remainder
is the config-key refusal half (`AST-LIV-001`/`AST-SRC-001`/
`AST-FNT-001`/`RFN-002` in the config/CLI layer) plus the doctor column
and `--explain` matrix row; the browser-host Playwright proof
(`hmr-state` spec) rides the same host as every prior E01 phase.

---

# Handoff — V2-A07 prerender error-status sidecar landed (a prerendered error page is no longer served as a 200); all Rust gates RUN for the first time in five sessions — the compile breaks the "pending a toolchain" sessions left behind are repaired, the workspace is fmt/clippy/test green on rustc 1.99.1

## Where things stand

**Branch:** `main` (the milestone ships as uncommitted changes plus
`ship.toml` / `.github/pr-body.md` at the repo root, both gitignored;
`ship.toml` groups the diff into conventional commits — one per
reviewer-visible intent — for the ship tooling to materialize).
**Verified this session (rustc 1.99.1 stable, 2-core/10 GB sandbox):**
- `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets
  --all-features -- -D warnings` clean (the CI bar). Pedantic holds for the
  six `[lints] workspace = true` crates and for every file this milestone
  touches; the rest of the workspace is the declared future hygiene pass —
  the command-line `-W clippy::pedantic` sweep measures ~hundreds of
  findings in the non-opted-in crates (odf alone ~75), which is why the
  policy is per-manifest opt-in.
- Runtime test suites RAN green for 16 crates (server 9/9 suites incl. the
  9 new A07 pins, cli's write_prerender section 8/8 incl. the 4 new
  sidecar pins, dev 76/76, cache, render, odf, router, config, dispatch,
  graph, schema, diagnostics, plugin partially, …). The remaining suites
  (assets, compiler, bundler, jshost, core-wasm, adapters, the FULL cli
  lib run beyond the write_prerender filter) are compile-verified by the
  clippy gate but their runtime runs did not fit the sandbox's disk
  ceiling — see the not-run list in `.github/pr-body.md`.
- The zero-`#[allow]` gate: `grep -rnE '#!\[(allow|expect)\(|cfg_attr\([^)]*_allow\('`
  over `src/ crates/ tests/ examples/ build.rs` — empty.
- Doc-drift fixes are IN this diff: the spec's V2.O table (A03 merged —
  WORKLOG.md retired; the B01/B02/B05/B06 rows corrected from a stale
  "todo" to partial with the landed substrates named; A07 merged),
  `docs/architecture.md` (the server row + the what-changed bullet),
  `docs/deviations.md` (the A07 record, 5 deltas), `README.md` (the
  badge re-measured 1,756→1,775 via `scripts/count-tests.sh`),
  `crates/server/README.md` (the provides list gains the sidecar seam),
  and `HANDOFF.md` (this header).

## What landed

V2-A07's prerender error-status sidecar substrate from
`docs/spec-v0.2-draft.md`, in the F16/F18/F20/E01 substrate pattern:

1. **`static_status.rs` (`crates/server/src/static_status.rs`):**
   `StatusSidecars` (`scan` — the spec's stack walk, `*.status` only,
   malformed content ignored, a missing root an empty map; `status_for`;
   `len`/`is_empty`), `is_status_sidecar`, `parse_status`, and
   `write_status_sidecar` — the write half of the contract lives next to
   the read half: non-200 writes the decimal sidecar, a 200 write clears a
   stale one. `status_sidecars_scan` tracing spans; a sidecar application
   logs on the serve path.
2. **The serve seam (`crates/server/src/lib.rs`):** `StaticAssets` owns a
   `status_sidecars` map filled by the same `scan_filtered` call the CLI
   already makes at serve start (no second walk, no per-request I/O);
   `serve` applies the sidecar status to every 200 response (identity,
   precompressed variant, HEAD) and hard-denies `*.status` paths (the
   SEC-STATIC shape, alongside the walk skip that keeps them out of the
   index).
3. **CLI wiring (`crates/cli/src/lib.rs`):** all four `write_prerender`
   emit paths (two loader short-circuits, two error bodies, the final
   injected page) plus BOTH incremental-reuse branches call
   `write_status_sidecar`, so a status that flips to 200 across builds
   removes the previous build's sidecar. The L0-seed write's silent
   `let _ =` failure is a `tracing::warn!` now (the micro-debt retirement)
   and the five `precompile.rs` worker-class `eprintln!`s are
   `tracing::warn!`s (the e81d0464 convention).
4. **Bench + example + pins:** the `status_sidecars` criterion group behind
   `bench-util` (`status_sidecars_scan` at 0/64-of-256 sidecars +
   `status_for` hit/miss), `crates/server/examples/status_sidecars.rs`
   (the walkthrough consumer the seam never had — runs in milliseconds),
   9 TDD pins in `crates/server/tests/status_sidecars_tdd.rs` and 4 CLI
   pins in `crates/cli/src/tests.rs` (1,756→1,775 Rust test functions).

**State repair this session (the "Rust gates pending a toolchain" debt):**
the tree at `16bce246` did not compile on rustc 1.99.1. Fixed and
documented in-diff: the wire-v1 seam's shadowed-`Result` signatures and a
`Duration::as_millis` fn-path misuse (`crates/dev/src/lib.rs`), a
`double_must_use` (`crates/dev/src/boundary.rs`), a never-run pin that
round-tripped a bare (envelope-less) frame (`crates/dev/src/tests.rs`),
the ring-0.17 ed25519 API surface (`sign` returns the signature,
`public_key` is trait-only), a `BisectError::git_spawn_error` receiver
typo, two by-value `git(...)` array args, a moved-capture `revs` clone,
the `Probe` trait scope at the bisect call site, four collapsible-ifs, an
HRTB closure spelling in `perf_bisect_tdd`, the never-updated
`Command::Build` test fixtures, and four files `cargo fmt` had never
seen. **Deferred:** the full-workspace pedantic hygiene pass, the
Playwright sidecar proof on a browser host, and the E01 remainder (state
preservation, Vue/Solid refresh wiring).

## Where to pick up, in one sentence

Run the remaining runtime suites on a roomier host (`cargo test
--workspace --all-features` — assets, compiler, bundler, jshost,
core-wasm, adapters and the full cli lib run are compile-verified only
here), then the E01 remainder (state preservation — consumes
`Plan`/`ClassifiedPlan` unchanged — or the Vue/Solid refresh wiring), or
the F18/F20 deferrals (the release-CI double-build + `verify --rebuild`
acceptance job is still the cheapest F20 item to close).

---

# Handoff — V2-E01 phase 3 message-schema v1 wire flip landed (the §13.3 v1 envelope: `update` entries + `ox:data` + the typed `DEV-HMR-003` gate); client gates green, Rust gates pending a toolchain

## Where things stand

**Branch:** `main` (the milestone ships as uncommitted changes plus
`ship.toml` / `.github/pr-body.md` at the repo root, both gitignored;
`ship.toml` groups the diff into conventional commits — one per
reviewer-visible intent — for the ship tooling to materialize).
**Verified this session (Rust toolchain absent; node v24 present):**
- `cargo fmt/clippy/test/bench` NOT run — the standing constraint. The
  Rust pins were authored TDD-first (`crates/dev/src/tests.rs`'s v1 wire
  section before the `HmrMessage` restructure), import public API only,
  and every pin is pure/deterministic (fixed stamps, tempfile fixtures).
  The riskiest surface is the CLI publish-site migration: the §3.2 batch
  tail now emits one `Update` frame with N entries (the v0 code published
  `ModuleUpdate`/`BatchModuleUpdate` with the same `(path, url)` payload
  — the entries carry the same URLs, plus stamps), the FullReload/Error
  arms are untouched, and `ServerSwap` stops publishing a frame the
  client no-opped anyway.
- **`packages/core`: `node --test test/*.test.ts` — 314/314 green**
  (baseline 304; +10 phase-3 pins: the v1 `update` fan-out through
  `reloadModule`, the N-entry batch, the one-reload fallback, the css
  link swap vs the `/@css/` module swap, the data-entry refetch, the
  `ox:data` push, the empty-updates no-op, and the two legacy
  read-compat pins). The legacy suite (css-update, route-data-update,
  full-reload, overlay) passes unchanged through the normalizer.
- The zero-`#[allow]` gate: `grep -rnE '#!\[(allow|expect)\(|cfg_attr\([^)]*_allow\('`
  over `crates/ tests/ examples/ benches/` — empty (unchanged policy; the
  new files carry no lint escapes; the u128→u64 stamp uses `try_from`,
  no casts).
- Doc-drift audit + fixes are IN this diff: the README test badge
  re-measured 1,750→1,756 (`scripts/count-tests.sh`'s own grep, post-diff),
  `docs/architecture.md` (the phase-3 what-changed bullet + the E01
  deferral list), `docs/deviations.md` (the phase-3 record, 6 deltas),
  the spec's V2.O table (the E01 row), `crates/dev/README.md` (the
  provides list gains the v1 wire surface), and `HANDOFF.md` (this
  header).

## What landed

V2-E01's phase-3 message-schema v1 wire flip from `docs/spec-v0.2-draft.md`
(the E01 remainder the last three handoffs carried), in the F16/F18/F20/E01
substrate pattern:

1. **The v1 wire (`crates/dev/src/lib.rs`):** `HmrMessage` speaks the
   spec's five shapes behind a `{"v":1}` envelope — `update` (N
   `UpdateEntry`s: `kind: 'js'|'css'|'data'` (the phase-2 `UpdateKind`,
   now the serde type), `path`, `acceptedPath`, publish-time `timestamp`,
   blake3 `hash`), `full-reload`, `error`, `ox:data`, `connected{build}`.
   `to_json`/`try_to_json` stamp the envelope; `try_from_json` is the
   typed decode gate (`HmrWireError::Parse`/`Schema`); `from_json` keeps
   its Option signature but warns loudly (the micro-debt retirement); the
   schema gate's `DEV-HMR-003` diagnostic carries the stale-tab/stale-CLI
   fix text. `UpdateEntry::stamped` + `unix_millis` + `content_hash` are
   the publish-time stamp helpers.
2. **CLI wiring (`crates/cli/src/lib.rs`):** every publish site speaks v1 —
   the fan-out entries are stamped (`Some(Path::new(&pstr))` hashes the
   real file), the §3.2 tail emits ONE `Update` frame (the singular/batch
   wire split dissolves), the tailwind sheet publishes a `css` entry (the
   unused inline `code` payload is retired — the swap refetches), the
   payload-less loader change is a `data` entry, the pushed loader
   payload is `OxData`, and `ServerSwap` stops publishing (a client
   no-op). The second micro-debt retirement: the esm prebundle failure
   path's bare `eprintln!` is a `tracing::warn!` (the rolldown-worker /
   Watcher class).
3. **The client (`packages/core/src/client/hmr.ts` + `hot.ts`):** one
   code path applies the v1 shapes — a `js` entry re-imports
   `acceptedPath` through the HMR runtime, a `css` entry swaps via the
   `/@css/` module (module-owned) or the `<link>` href (bare), a `data`
   entry re-fetches `/__data`, `ox:data` decodes the pushed payload —
   with `normalizeLegacyFrame` mapping the retired v0 spellings for
   stale-tab sessions. `hot.ts`'s worker socket rides `acceptedPath`.
4. **Bench + example + pins:** the `hmr_wire` criterion group behind
   `bench-util` (encode/decode at batch 1/16/64 + the two non-entry
   controls), `crates/dev/examples/hmr_wire.rs` (the walkthrough
   consumer the wire never had), the v1 wire pins in
   `crates/dev/src/tests.rs` (the Rust count moves 1,750→1,756), and 10
   client pins (314/314).

**Spec deltas recorded in `docs/deviations.md` (6):** kind is the
client's update mechanism (a `src/` stylesheet is a `css` entry whose
mechanism is the `/@css/` module re-import); `ox:data.data` rides base64
and a payload-less data change is an `Update` entry; the tailwind `code`
payload is retired; `server-module-swap` is retired; the v1 gate is
strict server-side and compat client-side; `UpdateKind` gains its serde
spelling. **Deferred (the rest of E01):** state preservation, the
Vue/Solid refresh wiring, the compat-window retirement, the Playwright
v1 runs on a toolchain host.

## Docs drift noted this session

- Fixed in-scope: the README badge (1,750→1,756 via
  `scripts/count-tests.sh`'s own grep) and the test-functions line, the
  stale v0 wire spellings in the dev README's provides list, the
  `boundary.rs`/`css_boundary.rs`/`boundary_tdd.rs` rustdoc mentions of
  the retired variants, and `HANDOFF.md` itself (this header).
- Noted for the doc owners (carried, unchanged): the `packages/*/
  package.json` versions remain at 0.6.1/0.6.0 while the workspace is
  0.9.0 (the `scripts/bump-version.sh` drift guard still refuses to run —
  a `chore(packages)` sync should ride the next release commit); the TS
  badge (458/458) could not be re-measured (the adapter suites need
  `node_modules`; `packages/core` runs, its count is inside the badge's
  aggregate); the crates.io/npm publication step is still pending for
  every tagged release.

## Where to pick up, in one sentence

Run the Rust gates on a toolchain host (`cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings
-W clippy::pedantic`, `cargo nextest run --workspace`, `cargo test -p
oxide-fw-dev --features bench-util --benches --no-run`, and the
Playwright suite for the v1 frames), then the E01 remainder is state
preservation (consumes `Plan`/`ClassifiedPlan` unchanged) or the
Vue/Solid refresh wiring, with the release-CI double-build +
`verify --rebuild` acceptance job still the cheapest F20 deferral to
close.

---



## Where things stand

**Branch:** `main` (the milestone ships as uncommitted changes plus
`ship.toml` / `.github/pr-body.md` at the repo root, both gitignored;
`ship.toml` groups the diff into conventional commits — one per
reviewer-visible intent — for the ship tooling to materialize).
**Verified this session (no Rust toolchain in the sandbox, per the
handoff's no-execution constraint):**
- `cargo fmt/clippy/test/bench` NOT run — same constraint as the E01,
  F18 and F20 sessions. The pins were authored TDD-first (the
  `tests/boundary_tdd.rs` phase-2 section before `src/boundary.rs`),
  import public API only, and every pin is pure and deterministic (paths
  in, plans out — no I/O, no clocks, no framework runtime). The riskiest
  surface is the `cmd_dev` StyleSwap tail swap: the planner decision is
  additive (the emitted `ModuleUpdate`/`CssUpdate` frames are
  byte-identical; the only new traffic is the `DEV-HMR-002` warning frame
  for an orphaned in-`src/` stylesheet), the legacy bundler-mode path is
  untouched, and the harvest change is invisible to JS events (a
  stylesheet can only terminate a walk that reaches it — JS chains never
  do).
- Every edited Rust file was re-parsed with a Rust grammar (tree-sitter,
  a parser library — not the Rust toolchain) as a syntax smoke check:
  all eight files parse clean. This is NOT a substitute for
  `cargo fmt --all --check` / clippy; it only rules out syntax-level
  breakage before the toolchain host runs the real gates.
- `grep -rnE '#!?\[(allow|expect)\(|cfg_attr\([^)]*_allow\('` over
  `src/ crates/ tests/ benches/ examples/ build.rs xtask/` — empty (the
  zero-allow-escape policy holds; the new files carry no lint escapes
  either).
- Doc-drift audit + fixes are IN this diff (see below): the README's
  v0.9.0 tag drift (two places — the F20 session fixed the v0.8.0 pair
  and the release commit re-drifted them) plus the 1,750-vs-1,713 badge
  re-measure (`scripts/count-tests.sh`'s own grep, post-diff).

## What landed

V2-E01's phase-2 CSS boundary substrate from `docs/spec-v0.2-draft.md` (the
deferred half of the boundary WP whose full surface is pure and V8-free),
in the F16/F18/F20 substrate pattern:

1. **`boundary.rs` (`crates/dev/src/boundary.rs`):** `UpdateKind` (the
   §13.3 schema's `kind: 'js'|'css'|'data'` discriminant; `as_str` is the
   wire spelling the TS client switches on), `update_kind` (the SPEC-14
   `ChangeKind` → wire-kind bridge), `css_self_accepts` (the
   extension-level style-injection probe), `plan_classified` +
   `ClassifiedPlan` (the fan-out's decision function: `ChangeKind` + the
   `plan` walk in one call, with the §13.3 CSS terminal — a `FullReload`
   walk for a stylesheet collapses to `Update([changed])`, the swap), and
   `css_orphan_diagnostic` (`DEV-HMR-002` with the fix help text).
   `hmr_boundary_classified` tracing spans on the seam. The phase-1
   surface (`plan` / `source_self_accepts` / `full_reload_diagnostic`) is
   unchanged.
2. **CLI wiring (`crates/cli/src/lib.rs`):** `build_hmr_graph` walks
   `css`/`scss`/`sass` too (stylesheets join the graph; they only receive
   JS→CSS edges — the specifiers always carry the extension, so
   exact-path resolution hits) and marks stylesheets self-accepting via
   the probe; the `cmd_dev` StyleSwap branch (ESM, in-`src/`) routes its
   decision through `plan_classified` — the emitted frames stay
   byte-identical, and an orphaned stylesheet publishes the `DEV-HMR-002`
   warning instead of reload noise. The legacy bundler-mode path is
   untouched.
3. **Micro-debt retired:** `HmrMessage::to_json` flattened serialize
   failures into an empty frame silently (`unwrap_or_default`) — the
   failure path is now a `tracing::error!`, the fallible `try_to_json`
   seam exists for callers that must distinguish, and `to_json`'s
   signature is unchanged (no caller churn). The stale `ModuleUpdate`
   rustdoc ("v0.1 does a reload ... post-v0.1 track") now describes the
   landed planner-driven fan-out.
4. **Bench + example + pins:** three criterion groups behind `bench-util`
   (`boundary_css_terminal` — the orphan collapse; `boundary_css_edge` —
   the own-boundary shape; `boundary_classified_js` — the Js control on
   the 50-deep chain), `crates/dev/examples/css_boundary.rs` (the
   walkthrough consumer), and 9 phase-2 pins in `tests/boundary_tdd.rs`
   plus the `try_to_json` unit pin — the Rust test count moves
   1,740 → 1,750 (`scripts/count-tests.sh`).

**Spec deltas recorded in `docs/deviations.md` (3):** the terminal lives
in `plan_classified`, not `plan` (the pure function stays spec-verbatim);
stylesheets receive edges but never emit them (a CSS `@import` chain needs
no graph edges — the child sheet self-accepts); the orphan-CSS case warns
(`DEV-HMR-002`) instead of reloading. **Deferred (the rest of E01):** the
message-schema v1 wire flip (Rust enum + TS client + adapter suites +
Playwright pins move together), state preservation across boundary swaps,
the Vue/Solid refresh wiring, and the browser-run CSS acceptance pins.

## Docs drift noted this session

- Fixed in-scope: root `README.md` (the "`v0.8.0` is tagged" note and the
  "v0.2.0 through v0.8.0" line — v0.9.0 is tagged and CHANGELOG'd; the
  test-count badge re-measured to 1,750 post-diff via
  `scripts/count-tests.sh`'s own grep), `docs/architecture.md` (the `dev`
  row + the phase-2 what-changed bullet), `docs/deviations.md` (the
  phase-2 record), the spec's V2.O table (the E01 row),
  `crates/dev/README.md` (the provides list gains the classified-planner
  surface), and `HANDOFF.md` itself (this header).
- Noted for the doc owners (carried, unchanged): the `packages/*/
  package.json` versions remain at 0.6.1 while the workspace is 0.9.0
  (the `scripts/bump-version.sh` drift guard still refuses to run — a
  `chore(packages)` sync should ride the next release commit, as the F20
  handoff already recorded); README's TypeScript badge (458/458) could
  not be re-measured here (no `node_modules`; the script's TS counter
  reports 0 across 95 files without them); the crates.io/npm publication
  step is still pending for every tagged release.

## Where to pick up, in one sentence

Run the Rust gates on a toolchain host (`cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`cargo nextest run --workspace`, `cargo test -p oxide-fw-dev --features
bench-util --benches --no-run`), then the E01 remainder is the
message-schema v1 wire flip (the Rust enum's kebab tags, the TS client's
`hmr.ts` and the four adapter suites move together) or state preservation
(consumes `Plan`/`ClassifiedPlan` unchanged), with the release-CI
double-build + `verify --rebuild` acceptance job still the cheapest F20
deferral to close.

---

# Handoff — V2-F20 attested reproducible builds landed (SBOM + provenance + signature engine, `build --attest` / `verify`); substrate authored, Rust gates pending a toolchain

## Where things stand

**Branch:** `main` (the milestone ships as uncommitted changes plus
`ship.toml` / `.github/pr-body.md` at the repo root, both gitignored;
`ship.toml` groups the diff into conventional commits — one per
reviewer-visible intent — for the ship tooling to materialize).
**Verified this session (no Rust toolchain in the sandbox, per the
handoff's no-execution constraint):**
- `cargo fmt/clippy/test/bench` NOT run — same constraint as the E01
  and F03 sessions. The pins were authored TDD-first
  (`tests/attest_tdd.rs` before `src/attest.rs`), import public API
  only, and every pin is pure or fixed-seed deterministic (a fixed
  32-byte ed25519 seed mints the same key file every run), so the
  suite's only external facts are the SHA-256 test vectors and the
  tempfile fixtures. The riskiest surface is the `cmd_build` tail swap
  (the `--attest` hook runs after the fingerprint store, before the GC
  sweep) and the `Verify` command plumbing, both additive — a plain
  `oxide build` without `--attest` executes none of the new code.
- Every edited Rust file was re-parsed with a Rust grammar (tree-sitter,
  a parser library — not the Rust toolchain) as a syntax smoke check:
  all six files parse clean. This is NOT a substitute for
  `cargo fmt --all --check` / clippy; it only rules out syntax-level
  breakage before the toolchain host runs the real gates.
- `grep -rnE '#!?\[(allow|expect)\(|cfg_attr\([^)]*allow\('` over
  `src/ crates/ tests/ benches/ examples/ build.rs xtask/` — empty (the
  zero-allow-escape policy holds; the new files carry no lint escapes
  either).
- Doc-drift audit + fixes are IN this diff (see below): the README's
  v0.8.0 tag drift (two places) and its 1,713-vs-1,701 test-count
  re-measure, the cli README's command-list drift (phantom `oxide
  serve`; missing `why` / `perf bisect` / `verify`), and the spec's
  V2.O table (F20 split out of the all-`todo` F-row).

## What landed

V2-F20's attestation substrate from `docs/spec-v0.2-draft.md` (the next
S-sized item whose dependency, A04, is merged and whose full surface is
pure + fixed-seed deterministic), in the F16/F18 substrate pattern:

1. **`attest.rs` (`crates/cli/src/attest.rs`):** the spec-verbatim
   engine — `canonical_json` (explicit recursive key sort; the bytes are
   stable even if `serde_json/preserve_order` ever unifies in),
   `is_canonical` (the pre-signature gate that kills whitespace-only
   tampers), the strict `Cargo.lock` / `pnpm-lock.yaml` line parsers
   (own code, no new dependency, no external generator), `sbom_from_locks`
   (CycloneDX 1.5, `pkg:cargo`/`pkg:npm` purls, sorted),
   `provenance_statement` (in-toto Statement v1 + SLSA v1, the spec's
   `buildDefinition` facts, `internalParameters.sourceDateEpoch` only
   when set), `subtree_digest` / `dist_subjects` (root-relative,
   slash-normalized, sorted, `attest/` excluded — no self-reference),
   `seed_to_pkcs8` / `load_signing_key(_bytes)` /
   `ed25519_pubkey_from_pem` (ring, already in the workspace graph),
   `sign_statement` / `verify_signature` / `verify_subjects`, the
   `write_attestation` composer, and the `ATT-*` diagnostics with fix
   help text. `attest_sbom` / `cli_attest_build` / `cli_attest_verify`
   tracing spans on the seams.
2. **CLI wiring (`crates/cli/src/lib.rs`):** `build --attest [--key]`
   hooks after the fingerprint store — writes `dist/build-id` (the A04
   `build_id_of_dist`; part of the A04 deferral closes here) and
   `dist/attest/{sbom.cdx.json, provenance.intoto.json, provenance.sig}`,
   failing the build on attestation failure; `oxide verify [--dist]
   [--pubkey] [--rebuild]` runs the canonical gate, the signature check
   (skipped loudly without a key — the envelope records the keyid), the
   subject walk, and the scratch-copy rebuild comparison. Facts
   collection (config blake3, `git rev-parse`/`status --porcelain`,
   builder id, `SOURCE_DATE_EPOCH`) stays in the command layer.
3. **Micro-debt retired:** the bundler's incremental worker failure
   paths logged through bare `eprintln!`
   (`rolldown_backend.rs` runtime/init arms — now `tracing::warn!` with
   the fallback spelled out, the same class as the E01 watcher fix), and
   `load_config_strict` flattening the structured `OxideError` into a
   string (the `[CFG-*]` code + `fix:` help line now survive into the
   anyhow chain the command boundary prints).
4. **Bench + example + pins:** `crates/cli/benches/attest.rs`
   (`canonical_json/{obj_64,obj_512}`, `sbom/{100,1000}`,
   `subtree_digest/dist_128_files`, `ed25519/{sign_60k,verify_60k}`;
   behind `bench-util`, the B09 pattern), `crates/cli/examples/
   attest.rs` (the walkthrough consumer: attest → canonical → signature
   → subjects → the one-byte tamper), and `tests/attest_tdd.rs`
   (24 pins incl. the two-builds-different-directories reproducibility
   pin, the one-byte-tamper pin, the SHA-256 known vectors and the
   lockfile-parser contracts).

**Spec deltas recorded in `docs/deviations.md` (6):** the signature
envelope is a JSON `{keyid, sig}` file; verification is canonical-gate
first, then signature; directories hash as subtree manifests and
`attest/` is never a subject; a lock-less project attests with an empty
cargo section; the pnpm side lists every locked package (the
"reached a bundle" filter is deferred); `--rebuild` compares every
subject except `dist/build-id` (the scratch build runs without
`--attest`). **Deferred:** the bundle filter, the unconditional A04
`dist/build-id` write (today `--attest`-only), a DSSE envelope upgrade,
and the CI double-build acceptance job.

## Docs drift noted this session

- Fixed in-scope: root `README.md` (the `v0.7.0`-tagged note and the
  "v0.2.0 through v0.7.0" line — v0.8.0 is tagged and CHANGELOG'd; the
  test-count badge re-measured to 1,713 via `scripts/count-tests.sh`'s
  own grep; the CLI groups gained `oxide verify` and the example block
  shows `build --attest`), `crates/cli/README.md` (the phantom `oxide
  serve` command → `oxide start`/`preview`; the missing `why`, `perf
  bisect` and `verify` rows), `HANDOFF.md` itself (this header), `docs/
  architecture.md` (`cli` row + the F20 what-changed bullet),
  `docs/deviations.md` (the F20 record), and the spec's V2.O table (F20
  split out of the all-`todo` F-row to `merged` with the deferral
  pointer).
- Noted for the doc owners (carried, unchanged): the `packages/*/
  package.json` versions remain at 0.6.1/0.6.0 while the workspace is
  0.8.0 (the `scripts/bump-version.sh` drift guard still refuses to run —
  a `chore(packages)` sync should ride the next release commit); README's
  TypeScript badge (458/458) could not be re-measured here (no
  `node_modules`); the crates.io/npm publication step is still pending
  for every tagged release.

## Where to pick up, in one sentence

Run the Rust gates on a toolchain host (`cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`cargo nextest run --workspace`, `cargo test -p oxide-fw-cli --features
bench-util --benches --no-run`), wire the release-CI double-build +
`verify --rebuild` acceptance job, then the E01 remainder (state
preservation, CSS boundaries, Vue/Solid refresh — consumes `Plan`
unchanged) or the A01 phase-3 dispatch split (which uncloses the
unconditional `dist/build-id` write and the deferred F05/F07/F12
wiring) are the next candidates.

---

# Handoff — V2-E01 HMR boundary substrate landed (planner + planner-driven fan-out + reload diagnostics); substrate authored, Rust gates pending a toolchain

## Where things stand

**Branch:** `main` (the milestone ships as uncommitted changes plus
`ship.toml` / `.github/pr-body.md` at the repo root, both gitignored;
`ship.toml` groups the diff into conventional commits — one per
reviewer-visible intent — for the ship tooling to materialize).
**Verified this session (no Rust toolchain in the sandbox, per the
handoff's no-execution constraint):**
- `cargo fmt/clippy/test/bench` NOT run — same constraint as the F03
  session. The diff is Rust-side (`crates/dev`, `crates/cli`, docs), so
  the standing green baseline is the F18 session's gates plus this diff's
  review. The pins were authored TDD-first (`tests/boundary_tdd.rs`
  before `src/boundary.rs`), are public-API-only, and the planner is
  pure — the riskiest surface is the `cmd_dev` fan-out swap, which keeps
  the react-refresh skip, the node_modules pre-filter, the singular/batch
  coalescing and the `src/` URL shaping byte-identical.
- `grep -rnE '#!?\[(allow|expect)\(|cfg_attr\([^)]*allow\('` over
  `crates/` — empty (unchanged; zero allow-escape policy holds).
- Doc-drift audit + fixes are IN this diff (see below): the README tag
  drift (v0.6.1 vs the released v0.7.0), the stale F03-era handoff header,
  and the `dev` crate README's phantom `FileWatcher` /
  mis-attributed `wrap_dispatch_with_stale_guard` rustdocs.

## What landed

V2-E01's boundary-planner substrate from `docs/spec-v0.2-draft.md` (the
next no-new-dependency item whose full surface is V8-free), in the
F16/F18 substrate pattern:

1. **`boundary.rs` (`crates/dev/src/boundary.rs`):** the spec-verbatim
   BFS — `plan(changed, importers, accepting, entries) -> Plan`
   (`Plan::Update(boundaries)` sorted + deduplicated / `Plan::FullReload`
   with the spec's reason spellings), cycle-safe via the visited set, the
   boundary check before the entry check (a route `page.tsx` that is both
   updates in place). Plus the two seams the spec names:
   `source_self_accepts` (the `import.meta.hot.accept` probe) and
   `full_reload_diagnostic` (the `DEV-HMR-001` warning with the fix help
   text the §13.4 overlay renders). `hmr_boundary` tracing spans on the
   planner; `oxide-fw-dev` gains the workspace-pinned `tracing` dep.
2. **CLI wiring (`crates/cli/src/lib.rs`):** `build_hmr_graph` replaces
   `build_importers_map` (same walk; harvests `accepting` — components,
   `.server.*`, hot-accept sources — and `entries` — route-structural
   files — in the same pass). The adapter-HMR fan-out is planner-driven:
   a non-boundary chain (`util.ts ← util2.ts ← page.tsx`) now updates the
   page boundary instead of leaving its import binding stale — the gap
   the old one-hop block documented against itself. React-refresh skip,
   orphan pre-filter, `src/` URL shaping and the §3.2 batch coalescing
   unchanged.
3. **Micro-debt retired:** the dead `update_boundary` one-hop
   simplification (no production callers; its doc claimed the real
   walk-up "lives in the graph engine" — it never did) and the
   `Watcher::with_notify` fallback's bare `eprintln!` (now
   `tracing::warn!`).
4. **Bench + example + pins:** `crates/dev/benches/boundary.rs` (deep
   chains 10/50/200, wide fan-outs, diamond dedup, the reload fallback;
   behind `bench-util`, the B09 pattern), `crates/dev/examples/
   hmr_boundary.rs` (the walkthrough consumer), and
   `tests/boundary_tdd.rs` (10 spec pins incl. the cycle, the diamond
   dedup and the orphan arm) + 3 unit tests in-module.

**Spec deltas recorded in `docs/deviations.md` (4):** the changed file
always re-imports (head of the batch); orphans pre-filter to a quiet
self-update (a file nothing imports cannot reach a client — reload noise,
the §1.3 boot-storm lesson); component modules are boundaries by
framework-plugin convention (CSS never enters the JS walk);
`update_boundary` is retired, not wrapped. **Deferred (the rest of E01):**
state preservation, the CSS boundary algorithm, Vue/Solid refresh wiring.

## Docs drift noted this session

- Fixed in-scope: root `README.md` (the "`v0.6.1` is tagged" note and the
  "v0.2.0 through v0.6.1" line — v0.7.0 is tagged and CHANGELOG'd),
  `HANDOFF.md` itself (this header — the previous top section described
  F03 as the latest milestone while F16/F18/v0.7.0 landed after it),
  `crates/dev/README.md` (phantom `FileWatcher` → `Watcher`; the
  stale-guard fn lives in `oxide-fw-cli`, not here; the provides list
  now includes the planner surface), `docs/architecture.md` (`dev` row +
  the E01 what-changed bullet), `docs/deviations.md` (the E01 record —
  F18 never got one), and the spec's V2.O table (E01 split out of the
  all-`todo` E-row to `doing`).
- Noted for the doc owners (carried, unchanged): `docs/archive/WORKLOG.md`
  dedup (the A03 hygiene remainder); README's "1,701 test functions" vs
  the actual count after this diff (+13 Rust pins — the next count audit
  should re-run `scripts/count-tests.sh`); the crates.io/npm publication
  step is still pending for every tagged release.

## Where to pick up, in one sentence

Run the Rust gates on a toolchain host (`cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`cargo nextest run --workspace`, `cargo test -p oxide-fw-dev --features
bench-util --benches --no-run`), then F20 (attested reproducible builds,
S-sized, A04-merged dependency) is the next spec-verbatim substrate
candidate — and E01's remainder (state preservation, CSS boundaries,
Vue/Solid refresh) consumes `Plan` unchanged.

---

# Handoff — V2-F03 interaction replay landed (capture engine + islands/boot wiring + opt-out seam); core package gates green

## Where things stand

**Branch:** `main`
**Working tree:** the milestone ships as uncommitted changes plus `ship.sh`
in the resolved downloads directory (bash 3.2-compatible, executable,
gitignored; one conventional commit per file, then idempotent PR + rebase
merge + tag + push). Run `./ship.sh <repo>` to materialize the history.
**Verified this session (no Rust toolchain, no `node_modules`):**
- `packages/core`: `node --test test/*.test.ts` — 304/304 green (baseline
  289; +15 F03 pins: the spec's click-before-hydrate-replays-once,
  click-after-hydrate-not-recorded, input value + event replay, capacity
  drop-oldest at 100, unresolvable-path drop, and focus replay; the
  mid-flight double-replay guard; the checkbox `checked` restore; the
  islands `hydrateIsland` wiring pin; and the boot `interactionReplay:
  false` opt-out pin). The two `bootWithRouter` router-absence pins moved
  from listener-count proxies to behavioral assertions (a click must not
  reach `adapter.update`) because the replay capture legitimately owns a
  document `click` listener of its own.
- `node test/replay.bench.ts` — capture/miss 12.8 ns (click outside any
  island, 5-level `closest` walk), capture/record ≈ 148 ns at the
  100-record bound, replay/dispatch ≈ 13.3 µs per 100-record replay
  (≈ 133 ns/record: resolve + identity check + dispatch).
- `cargo fmt --all --check`, the clippy gates and `cargo nextest` were
  NOT run — no Rust toolchain in this sandbox. The diff touches no Rust
  file (packages/core TS + docs only), so the F12 session's green Rust
  gates are the standing baseline. `tsc --noEmit` NOT run (needs
  `node_modules`; same as the F06/F07 sessions). Adapter-package suites
  (`packages/{react,vue,svelte,solid}`) fail at module resolution without
  `pnpm install` — pre-existing, unchanged by this diff.
- `grep -rnE '#!?\[(allow|expect)\(|cfg_attr\([^)]_allow\('` over
  `crates/ tests/ examples/ conformance/ packages/` — empty.

## What landed

V2-F03 from `docs/spec-v0.2-draft.md` (the spec's next S-sized
no-dependency item, and the only candidate whose full gate surface is
runnable without a Rust toolchain), in the F05/F06/F07 substrate pattern:

1. **`replay.ts` (`packages/core/src/client/replay.ts`):** the
   spec-verbatim capture engine — `startReplay(doc?)` installs
   capture-phase listeners (`focus` rides `focusin`), records
   `{ type, island, path, value?, checked? }` only while the island host
   is pending (`data-ox-hydrated !== 'done'`), caps the log at 100 with
   drop-oldest, and `replayFor(host)` replays the host's records in
   order, exactly once, right after hydration. Structurally typed like
   `IslandHost`/`IslandsDoc` so real DOM and test fakes satisfy the same
   surface; import-safe under the SSR module-safety contract.
2. **Islands wiring (`islands.ts`):** `hydrateIsland` calls
   `replayFor(el)` immediately after `data-ox-hydrated` becomes `done` —
   framework listeners are attached, so the replayed interactions land on
   live handlers.
3. **Boot wiring (`boot.ts`):** both `boot()` (via `__oxideBootRun`) and
   `bootWithRouter()` start the capture BEFORE any island hydrates. The
   payload's `interactionReplay: false` opts out; capture is
   page-lifetime glue (like `loadScripts`), and `startReplay` is
   idempotent per document so a double boot cannot double-record.
4. **Pins + bench:** `test/replay.test.ts` (15 pins incl. the wiring and
   opt-out integration tests) and `test/replay.bench.ts` (the numbers
   above; not part of the CI test glob).

**Spec deltas recorded in `docs/deviations.md` (4):** records carry the
captured `target` and replay verifies node identity (the spec's
contains-guard cannot catch a same-slot replacement after a defer swap);
`pathOf` refuses paths that do not terminate at the island host (no
truncated-garbage records); replay restores `checked` for `change`
records (the sketch recorded but dropped it); `startReplay` is
idempotent per document. **Deferred:** the Rust config surface
(`interactionReplay` in `crates/config` + schema regen + hydration
bootstrap emission — the client already honors the payload field) and
the Playwright block-release acceptance; see `docs/deviations.md`.

## Docs drift noted this session

- Fixed in-scope: `docs/architecture.md` (TS `oxide` package row +
  what-changed list) and `docs/deviations.md` carry the F03 record.
- Noted for the doc owners (carried from F12, unchanged): README's crate
  count (says 18, 19 members — missing `dispatch`) and its contradictory
  test-count badge vs the "1,239 test functions" line; the "v0.1.0 is
  not yet tagged" claims vs `v0.4.1`; the CI job-count claims (22/21 vs
  the 20 defined); the spec Appendix V2.O status table (all `todo` for
  landed substrates — F03 now included); CONTRIBUTING's "Rust 1.98
  pinned by rust-toolchain.toml" vs `channel = "stable"`;
  `benchmarks/README.md` omitting the `dispatch`/`server` benches.
  Also: the `v0.4.2` release commit is on `main` but NO `v0.4.2` tag
  exists — the next tag per the tag+conventional-commits rule lands on
  `v0.5.0` (feat commits exist in `v0.4.1..HEAD`); and
  `packages/*/package.json` remain at 0.1.0 vs workspace 0.4.2, which
  keeps `scripts/bump-version.sh`'s drift guard refusing to run.

## Where to pick up, in one sentence

A01 phase-3 on a V8-capable host remains the bottleneck (it also
uncloses F12's sampler/enforcement wiring and F07's tee/endpoint
wiring); then F10's pool side, the `tests/live.sh` e2e, and the F-series
(F16 "why dynamic?" and F18 perf bisect are the remaining
no-dependency candidates — both Rust-side, so they need a toolchain;
F03's Rust config surface is a small add-on to the landed client
substrate).

---

# Handoff — V2-F12 brownout-mode substrate landed (governor + enforcement seam + config surface + gauge); all gates green

## Where things stand

**Branch:** `main`
**Working tree:** the milestone ships as uncommitted changes plus `ship.sh`
in the resolved downloads directory (bash 3.2-compatible, executable,
gitignored; one conventional commit per file, then idempotent PR + squash
merge + tag + push). Run `./ship.sh <repo>` to materialize the history.
**Verified this session (2 vCPU sandbox, no V8 toolchain, no nasm):**
- `cargo test --all-features` over the 15-crate V8-free graph
  (excludes `jshost` + `cli` (V8), `plugin` + `assets` (nasm)) — all green
  (1070 tests, baseline 1037; +33 F12 pins: server `governor_tdd` 18
  incl. the spec table's raw_level breach matrix, immediate escalation,
  the 6-tick hysteresis drop, the band that holds, the serve_decision ×
  priority matrix, the shed header spelling and the Emergency-only
  readiness flip; config `brownout_config` 14 incl. the DSP-113 rejections
  and the schema-section shape; plus the metrics
  `brownout_level_gauge_round_trips` pin).
- `cargo fmt --all -- --check` — exit 0.
- Workspace-lints pedantic gate `cargo clippy --no-deps -p oxide-fw-config
  -p oxide-fw-dispatch -p oxide-fw-router -p oxide-fw-schema
  --all-targets --all-features -- -D warnings -W clippy::pedantic` — zero
  diagnostics (the CLI `-W` does not defeat the `[lints.clippy]`
  `priority = -1` override; the gate fired and was fixed during the
  session, proving it enforces).
- CI-style gate `cargo clippy --no-deps` over the V8-free graph
  (`--all-targets --all-features`, `-D warnings`) — zero diagnostics.
  Excluded: `jshost` + `cli` (V8), `plugin` + `assets` (nasm absent).
  `server` remains un-opted (pre-existing pedantic debt belongs to the
  planned hygiene pass); all NEW code is pedantic-clean by direct check
  (zero diagnostics point at `governor.rs`, the metrics gauge or the
  brownout config lines).
- `grep -rnE '#!?\[(allow|expect)\(|cfg_attr\([^)]_allow\('` over
  `crates/ tests/ examples/ conformance/ packages/` — empty.
- `cargo test --all-features --benches` over the graph — exit 0; the new
  `governor` criterion bench reports `next_level` steady-state
  1.3–5.0 ns, `raw_level` ≈ 1.0–1.4 ns, `governor/level_read` ≈ 372 ps
  (the acceptance contract: one relaxed atomic load), `serve_decision`
  ≈ 330 ps.

## What landed

V2-F12 from `docs/spec-v0.2-draft.md` (the brownout-mode governor), in
the A02/A05/F04/F05/F06/F07 substrate pattern:

1. **`server::governor` (`crates/server/src/governor.rs`):** the
   spec-verbatim model — `Level` (Normal/Stale/Shed/Emergency with the
   numeric header payload), `Signals`, `Thresholds`, the pure
   `raw_level` breach scan (`>=`, inclusive), the pure `next_level`
   transition (escalate immediately, hold in the hysteresis band,
   de-escalate one level after `calm_ticks` calm ticks), and `Governor`
   (the `AtomicU8` the sampler publishes and the request path reads).
   The enforcement seam beside it: `Priority` (`low|normal|critical`),
   `serve_decision` (level × priority → `Serve`/`ServeStale`/`Shed`),
   `shed_response` (`503` + `retry-after: 5` + `x-oxide-brownout:
   <level>`), `ready_status` (`503` only at `Emergency`).
2. **Gauge (`crates/server/src/metrics.rs`):** `brownout_level: AtomicU64`
   + `Metrics::set_brownout_level` + the `oxide_brownout_level` gauge in
   the Prometheus exposition (the F05 pool-gauge pattern), pinned.
3. **Config surface (`BrownoutConfig`/`BrownoutThresholdsConfig`/
   `HedgeConfig`):** `brownout: { enabled: false, staleCeiling: "10x",
   thresholds: { queueWaitMs: [50,200,500], loopLagMs: [20,100,300],
   rssMb: [0,0,0] }, hedge: { enabled: false, afterFactor: 2.0 } }`
   (camelCase, `deny_unknown_fields`); load validation emits **DSP-113**
   (ascending ladders, a `staleCeiling` multiplier ≥ 1, a hedger
   `afterFactor` > 1, and the `rules[].priority` grammar); `parse_stale_
   ceiling` is public; the JSON schema declares the section and the
   checked-in `schemas/oxide-fw-config.schema.json` was regenerated
   (stability test green; its known-sections list also gained the
   missing `stream` entry — F07 drift fixed in scope).
4. **Route priority (`NativeRule.priority`):** the router struct and the
   config mirror both carry `priority: Option<String>`; the CLI's
   `config_rules_to_router` passes it through; the spelling validates at
   load under DSP-113 and parses via `governor::parse_priority` (pinned
   both sides). `page.server.ts` `config.priority` adoption rides the
   compiler-side wiring.

**Spec deltas recorded in `docs/deviations.md` (6):** the
`exit_ratio: f64` hysteresis ratio is an integer percent
(`exit_ratio_pct: u64`, default 60 — the zero-`#[allow]` policy forbids
the lossy `u64`→`f64` casts; the comparison is exact `u128` math);
config ladders are milliseconds (§14 human units); DSP-113 validates
what the spec leaves unchecked; `ServeStale` is "eligible", not
"forced"; `Governor::set` clamps to 3; the `rssMb: [0,0,0]` config
spelling maps to the spec's never-breach `u64::MAX` sentinel.

## Docs drift noted this session

- Fixed in-scope: `crates/config/tests/schema_stability.rs`'s
  known-sections list was missing `stream` (the F07 session regenerated
  the schema but did not update the list); added, plus `brownout`.
  `docs/architecture.md`'s `server` crate row + what-changed list
  updated; `docs/deviations.md` carries the F12 record.
- Noted for the doc owners (carried from F06/F07, unchanged): README's
  crate count (says 18, 19 members — missing `dispatch`) and its
  contradictory test-count badge vs the "1,239 test functions" line; the
  "v0.1.0 is not yet tagged" claims vs `v0.4.1`; the CI job-count claims
  (22/21 vs the 20 defined); the spec Appendix V2.O status table (all
  `todo` for landed substrates — F12 now included); CONTRIBUTING's
  "Rust 1.98 pinned by rust-toolchain.toml" vs `channel = "stable"`;
  `benchmarks/README.md` omitting the `dispatch`/`server` benches. New
  this session: `scripts/bump-version.sh`'s drift guard refuses to run
  today — the `packages/*/package.json` versions are 0.1.0 while the
  workspace is 0.4.1, so the script's "every surface at CURRENT" check
  fails before any bump (its `oxide-*` workspace-dependency pins and the
  root `package.json` are the only version surfaces that still agree).

## Where to pick up, in one sentence

A01 phase-3 on a V8-capable host remains the bottleneck and now also
uncloses F12's wiring (the 500 ms sampler task over `pool.stats()` /
loop-lag / RSS, the enforcement stage after prelude calling
`serve_decision` + `shed_response`, the `warning: 110` header on stale L0
service, `ready_status` on `/_oxide/ready`, the opt-in hedger and the
`benchmarks/overload.sh` load test), then F10's pool side (whose early
drop consults the brownout threshold), the `tests/live.sh` e2e, and the
F-series (F08/F09 need R02's crates; F03/F16/F18 are the next
no-dependency candidates).

---

# Handoff — V2-F07 resumable streams substrate landed (stream_store + data-plane seam + config + client); all gates green

## Where things stand

**Branch:** `main`
**Working tree:** the milestone ships as uncommitted changes plus `ship.sh`
in the resolved downloads directory (bash 3.2-compatible, executable,
gitignored; one conventional commit per file, then idempotent PR + squash
merge + tag + push). Run `./ship.sh <repo>` to materialize the history.
**Verified this session (2 vCPU / 4 GB sandbox, no V8 toolchain):**
- `cargo test --all-features` over the 15-crate V8-free graph
  (excludes `jshost` + `cli` (V8), `plugin` + `assets` (nasm)) — all green
  (1037 tests, baseline 1004; +33 F07 pins: server `stream_tdd` 18 incl.
  the spec-named `read_from` offsets / overflow-disables-replay /
  ttl-expiry / reattach-after-complete / concurrent-append-and-read pins,
  dispatch `stream_tdd` 6 incl. the data-plane flip and the build-check
  pins, config `stream_config` 10 incl. the DSP-112 and CFG-SIZE pins,
  and the A04 `data_plane_paths` pin updated to the landed world).
- `packages/core`: `node --test test/*.test.ts` — 289/289 green (baseline
  283; +6 F07 pins: pass-through, first-pass/onChunk, the mid-stream drop
  reattaching with `Range: bytes=<n>-` at the exact count, the 410 →
  `stream expired` throw, the 5-try backoff ladder ending in `stream
  lost`, and the non-ok-without-body reattach spending a try).
- `cargo fmt --all -- --check` — exit 0.
- Workspace-lints pedantic gate `cargo clippy --no-deps -p oxide-fw-config
  -p oxide-fw-dispatch -p oxide-fw-router -p oxide-fw-schema --all-targets
  --all-features -- -D warnings -W clippy::pedantic` — zero diagnostics.
- CI-style gate `cargo clippy --no-deps` over the V8-free graph
  (`--all-targets --all-features`, `-D warnings`) — zero diagnostics.
  Excluded: `jshost` + `cli` (V8), `plugin` + `assets` (build needs
  `nasm`, absent here). `server` remains un-opted (pre-existing pedantic
  debt belongs to the planned hygiene pass); all NEW code is
  pedantic-clean by direct check.
- `grep -rnE '#!?\[allow\(' crates/ tests/` — empty.
- `cargo bench -p oxide-fw-server --features bench-util --bench stream` —
  `append/{256,8192}` ≈ 14.7 ns (flat in chunk size: one lock + push +
  atomic + watch send), `read_from/{full,mid,tail}` on a 1 MiB buffer ≈
  42 µs / 12.8 µs / 314 ns, `store_get/{hit,miss}` ≈ 59 / 21 ns,
  `stream_id/mint` ≈ 113 ns.

## What landed

V2-F07 from `docs/spec-v0.2-draft.md` (the spec's last "M, no
dependencies" item), in the A02/A05/F04/F05/F06 substrate pattern:

1. **`server::stream_store` (`crates/server/src/stream_store.rs`):** the
   spec-verbatim `StreamBuf`/`StreamStore` model — `begin`/`append`/
   `finish`/`get`/`sweep`, the pure `read_from` replay (backlog from any
   offset, `Some(empty)` at the exact end, `None` past it), `stream_id`
   (blake3(request_id, counter) → 16 lowercase hex, the A04 convention),
   `parse_range_offset` (`bytes=<n>-` only; absent header defaults to 0;
   every other spelling is refused), the response builders (`206` +
   `content-range: bytes n-*/*` with backlog-then-live-tail, `200` +
   `x-oxide-stream-complete: 1`, `410`), and `tee_stream` (the body
   wrapper: every consumer-visible byte lands in the buffer; end and
   producer error both mark done). Bounds: per-buffer ttl + `max_each`
   (overflow drops replay instead of eating memory) + store-wide
   `max_total` (evicts oldest completed first, then oldest).
2. **Data-plane seam (`dispatch::build`):** `/__oxide/stream/` joins
   `is_data_plane_path` — the flip the A04 comment explicitly reserved —
   and `stream_resume_id` parses the id segment (query/fragment
   tolerated, traversal refused). The A04 `stage_build_check` therefore
   guards the reattach route with the same 409 contract.
3. **Config surface (`StreamConfig`/`StreamResumeConfig`):** `stream:
   { resume: { ttlSecs: 60, maxBytesEach: "4mb", maxBytesTotal: "256mb" } }`
   (camelCase, `deny_unknown_fields`, size literals in the §14 grammar);
   load validation emits **DSP-112** (ttl ≥ 1 s, each ≥ 1 byte, total ≥
   each) and **CFG-SIZE** on a bad literal; the JSON schema declares the
   section and the checked-in `schemas/oxide-fw-config.schema.json` was
   regenerated (stability test green).
4. **Client (`packages/core/src/client/resume.ts`):** `resumableFetch(
   input, init?, onChunk?, opts?)` per spec step "Client" — the first
   pass pumps the body, a mid-stream drop reattaches
   `/__oxide/stream/<id>` with `Range: bytes=<got>-` up to five tries on
   the 200 ms doubling ladder, `410` throws `stream expired`, five
   failures throw `stream lost`. fetch + sleep injectable (Node-test-safe,
   the `live.ts` contract); `onChunk` sees each chunk exactly once, in
   order, across the drop.

**Spec deviation found and recorded in `docs/deviations.md`:** the spec's
`Notify::notify_waiters` wait loses chunks appended between a reader's
backlog check and the waiter's first poll (waiters register on poll, not
on creation) — the reattach stream would hang mid-stream on a multithread
runtime. `StreamBuf` carries a `watch` channel over `Progress { len, done }`
instead (the F05 `Sub` pattern): every state transition sends, `changed()`
never misses, the held primary receiver makes sends infallible.

## Docs drift noted this session

- Fixed in-scope: the A04 `data_plane_paths` pin (and the `build.rs`
  comment) documented the pre-F07 data plane; updated to the landed world
  with the F07 pins beside them. `docs/architecture.md` crate table +
  changelog updated. `docs/deviations.md` carries the F07 record.
- Noted for the doc owners (unchanged from F06's list): README's crate
  count (says 18, 19 members — missing `dispatch`) and its contradictory
  test-count badge vs the "1,239 test functions" line; the "v0.1.0 is not
  yet tagged" claims vs `v0.4.0`; the CI job-count claims (22/21 vs the 20
  defined); the spec Appendix V2.O status table (all `todo` for landed
  substrates — F07 now included); CONTRIBUTING's "Rust 1.98 pinned by
  rust-toolchain.toml" vs `channel = "stable"` (and CONVENTIONS' "1.99");
  `benchmarks/README.md` omitting the `dispatch`/`server` benches.

## Where to pick up, in one sentence

A01 phase-3 on a V8-capable host remains the bottleneck and now also
uncloses F07's wiring (tee resumable response bodies with `tee_stream`,
mount the `/__oxide/stream/*` handler from `stream_resume_id` +
`parse_range_offset` + `StreamStore::get` + `stream_resume_response`,
flow the `stream.resume` knobs into `StreamStore::new`, add the per-IP
reattach rate limit and the 30 s `sweep` task), then F10's pool side, the
`tests/live.sh` e2e (assert `ODP1` after the first full), and the
F-series.

---

# Handoff — V2-F06 ODF patch deltas landed (odf::patch + server policy + TS transport); all gates green

## Where things stand

**Branch:** `main`
**Working tree:** the milestone ships as uncommitted changes plus `ship.sh`
in the resolved downloads directory (bash 3.2-compatible, executable,
gitignored; one conventional commit per file, then idempotent PR + squash
merge + tag + push). Run `./ship.sh <repo>` to materialize the history.
**Verified this session (2 vCPU / 4 GB sandbox, no V8 toolchain):**
- `cargo test --all-features` over the 15-crate V8-free graph
  (excludes `jshost` + `cli` (V8), `plugin` + `assets` (nasm)) — all green
  (1004 tests across 77 suites, baseline 970/76; +34 F06 pins: odf
  `patch_tdd` 25 incl. the 512-case apply∘diff proptest and the
  2000-row-append-under-5% acceptance bound; server `live_tdd` +9 incl.
  the base-chain and runner-patch pins).
- `packages/core`: `node --test test/*.test.ts` — 283/283 green (baseline
  260; +23 F06 pins: the 12 committed cross-language goldens, the
  cross-language base-id parity, the chain and stale-base reconnect pins),
  `tsc --noEmit` NOT run (no `node_modules` in this sandbox; the test
  runner needs none).
- `cargo fmt --all -- --check` — exit 0.
- Workspace-lints pedantic gate `cargo clippy --no-deps -p oxide-fw-config
  -p oxide-fw-dispatch -p oxide-fw-router -p oxide-fw-schema --all-targets
  --all-features -- -D warnings -W clippy::pedantic` — zero diagnostics.
- CI-style gate `cargo clippy --no-deps` over the V8-free graph
  (`--all-targets --all-features`, `-D warnings`) — zero diagnostics.
  Excluded: `jshost` + `cli` (V8), `plugin` + `assets` (build needs
  `nasm`, absent here). `odf`/`server` remain un-opted (pre-existing
  pedantic debt belongs to the planned hygiene pass); all NEW code is
  pedantic-clean by direct check (no pedantic finding touches
  `patch.rs`, the new fixtures code, `benches/patch.rs`, or the new
  `live.rs` additions).
- `grep -rnE '#!?\[allow\(' crates/ tests/` — empty.
- `cargo bench -p oxide-fw-odf --features bench-util --bench patch` —
  diff+encode 24 µs @256 / 305 µs @2000 rows; apply 67 µs / 746 µs; the
  full server policy (decode×2 + diff + encode + decision) 234 µs @256 /
  2.06 ms @2000. `cargo bench -p oxide-fw-server --features bench-util
  --bench live` re-run green (sse_frame ~430 ns on this host).

## What landed

V2-F06 from `docs/spec-v0.2-draft.md` (decision refinement D30), in the
A02/A05/A06/F04/A04/F05 substrate pattern:

1. **`odf::patch` (`crates/odf/src/patch.rs`):** the spec-verbatim
   `Seg`/`Op` model, the structural `diff` (objects recurse by key; arrays
   trim common prefix/suffix so append/prepend/middle-replace/truncate are
   each one `Splice`; everything else is a whole-value `Set`), `apply`
   (create-missing-leaf Set, object-key-only Del, bounds-checked Splice,
   reserved-key rejection), and the D30 envelope — `ODP1` prefix + an
   ordinary ODF1 payload whose root is `{ b: <base id>, o: [ops] }`, so the
   existing encoder/decoder is reused byte for byte. `base_id` is the first
   16 hex of the SHA-256 of the full ODF base bytes (`sha2`, not blake3 —
   the client hashes the same bytes with the same algorithm).
2. **Cross-language goldens (`crates/odf/tests/fixtures/patch/`):** the 12
   spec-named cases (set, nested set, delete, append, prepend, middle
   splice, truncate, type change, Map untouched, deep array of objects,
   no-op, base mismatch) committed as `patch_<name>.{base.odf.bin,odp.bin,
   expected.json}` triples — the Rust suite (`tests/patch_tdd.rs`) and the
   TS suite (`packages/core/test/odf-patch.test.ts`) both load the same
   bytes, so encode/decode/apply AND the base-id hashing are pinned
   cross-implementation. Regenerate with
   `cargo run -p oxide-fw-odf --example odf_fixture`.
3. **Server sending policy (`server::live::delta_payload` +
   `Sub::prepare_rerun`):** the rerun worker's `Option<Bytes>` result now
   flows through the D30 policy — decode old/new, diff, encode, ship
   `ODP1` only while `patch.len() * 2 < full.len()` and
   `ops.len() <= 64`; everything else (no base yet, undecodable bytes,
   oversized diffs, encode failure) ships the full payload. Each `Sub`
   keeps its last FULL snapshot as the patch base (`last_full`); the
   connect path (`push_snapshot`) seeds it, so the first rerun can already
   patch. A fresh connection always gets a full snapshot (the F05
   invariant, preserved).
4. **Client patch transport (`packages/core/src/client/odf-decode.ts`):**
   `decodeOdfOrPatch(b64, last)` sniffs `ODP1` vs `ODF1`, applies wire ops
   (`applyPatch` — same semantics as the Rust `apply`, deep-cloned with
   identity preserved, reserved keys rejected anywhere in a path) against
   the value this stream last decoded (a WeakMap side table keyed by the
   previous value), verifies `b` against the stored truncated SHA-256
   (`sha256Hex16`, a compact synchronous implementation — the decode hook
   is sync; the vectors + goldens pin parity with the server), and
   advances the base by re-encoding the applied value with the canonical
   TS encoder. `connectLive`'s default decode is now the patch transport
   (the F05 `decode?: LiveDecode` seam); any decode failure — including a
   refused base — closes the stream and takes the existing jittered
   reconnect, and the server answers a fresh connection with a full
   snapshot.
5. **Acceptance bound pinned:** a 2,000-row list with one appended row
   encodes to a patch under 5% of the full payload (test-asserted; the
   bench shows the same shapes: 305 µs diff+encode vs 2.06 ms full-policy
   path at 2000 rows).

**Substrate landmine found and fixed on the way (recorded in
`docs/deviations.md`):**
1. **The ODF encoder aliases bit-exact-equal containers into `Ref(i)`
   slots** — an empty Splice `insert` aliases an empty `path` array, so a
   naive `decode_patch` that ignored refs failed to parse its own
   re-encoded output (the truncate/no-op goldens caught it). Fixed:
   `decode_patch` decodes with the shared table and resolves refs before
   parsing the ops shape.

## Docs drift noted this session

- Fixed in-scope: `crates/odf/src/lib.rs` pointed at a nonexistent
  `docs/format.md` and a nonexistent `packages/oxide` package — corrected
  to the real paths (`packages/core/src/client/odf-decode.ts`, the
  fixtures directories).
- Noted for the doc owners (unchanged): README's crate count (says 18,
  19 members — missing `dispatch`) and its contradictory test-count badge
  vs the "1,239 test functions" line; the "v0.1.0 is not yet tagged"
  claims vs `v0.3.1`; the CI job-count claims (22/21 vs the 20 defined);
  the spec Appendix V2.O status table (all `todo` for landed substrates);
  CONTRIBUTING's "Rust 1.98 pinned by rust-toolchain.toml" vs
  `channel = "stable"` (and CONVENTIONS' "1.99"); `benchmarks/README.md`
  omitting the `dispatch`/`server` benches; `odf-decode.ts`'s "~1 KB"
  size claim in `client/index.ts`'s comment.

## Where to pick up, in one sentence

The F06 transport is active end-to-end inside the F05 substrate, so the
next milestone is the A01 phase-3 wiring on a V8-capable host (the rerun
closure's `Option<Bytes>` now flows through `prepare_rerun` automatically —
the endpoint, the revalidate-tag hook, and the client-router lifecycle
activate patching with zero further changes), then F10's pool side, the
`tests/live.sh` e2e (assert `ODP1` after the first full), and the F-series.

---

# Handoff — V2-F05 live loaders substrate landed (SSE framing + hub + runner + client); all gates green

## Where things stand

**Branch:** `main`
**Working tree:** the milestone ships as uncommitted changes plus `ship.sh`
in the resolved downloads directory (bash 3.2-compatible, executable,
gitignored; one conventional commit per file, then idempotent PR + squash
merge + tag + push). Run `./ship.sh <repo>` to materialize the history.
**Verified this session (2 vCPU / 4 GB sandbox, no V8 toolchain):**
- `cargo test --workspace --all-features` over the 15-crate V8-free graph
  (excludes `jshost` + `cli` (V8), `plugin` + `assets` (nasm)) — all green
  (970 tests across 76 suites; server +18 live pins incl. the five
  spec-named hub/framing pins and the runner debounce/rate pins; router +5
  live pins incl. the rkyv round trip; config +10 live pins incl. the
  DSP-111 paths; dispatch's A04 pins now pin the live route's query build
  contract; render +5 bootstrap pins; adapters +4 Live-row pins).
- `packages/core`: `node --test test/*.test.ts` — 260/260 green (7 new
  live pins: URL shape, data decode+apply, stale reload, jittered
  reconnect, disposer, visibility close/reopen), `tsc --noEmit` clean.
- `cargo fmt --all -- --check` — exit 0.
- Workspace-lints pedantic gate `cargo clippy --no-deps
  -p oxide-fw-config -p oxide-fw-dispatch -p oxide-fw-router
  -p oxide-fw-schema --all-targets --all-features -- -D warnings
  -W clippy::pedantic` — zero diagnostics (the `[lints] workspace=true`
  crates; every site the new code added is pedantic-clean).
- CI-style gate `cargo clippy --no-deps --workspace` over the V8-free
  graph (`--all-targets --all-features`, `-D warnings`) — zero
  diagnostics. Excluded: `jshost` + `cli` (V8), `plugin` + `assets`
  (build needs `nasm`, absent here). `server`/`render`/`adapters` have not
  adopted `[lints] workspace=true` yet (the planned dedicated hygiene
  pass owns their pre-existing pedantic debt); all NEW code in them is
  pedantic-clean by direct check.
- `grep -rnE '#!?\[allow\(' crates/ tests/` — empty.
- `cargo bench -p oxide-fw-server --features bench-util --bench live`
  — `affected/256` ≈ 568 ns, `affected/1024` ≈ 559 ns (flat in hub size —
  the index is per-tag), `subscribe_remove/256` ≈ 6.6 µs (one reconnect),
  `sse_frame/ping` ≈ 124 ns, `sse_frame/data_1k` ≈ 512 ns,
  `sse_frame/data_8_lines` ≈ 1.15 µs.

## What landed

V2-F05 step 1 + steps 2–5's V8-free substrate from
`docs/spec-v0.2-draft.md`, in the A02/A05/A06/F04/A04 substrate pattern:

1. **Router opt-in (`RouteEntry.live`, `RouteLive`):** the scanner reads
   `export const config = { live: true }` / `{ live: { debounceMs: 100 } }`
   from `page.server.ts` next to `tags`/`needs_parent`; the additive
   `Option<RouteLive>` field round-trips the rkyv archive (ArchEntry
   mirror) and deserializes `None` from pre-F05 `routes.json` manifests.
2. **Config surface (`LiveConfig`, §V2-F05 step 8):** `maxSubscribers`
   5000 / `debounceMs` 50 / `keepaliveSeconds` 20 / `maxRerunsPerSecond`
   20 / `perIpMax` 20 / `transport: 'sse' | 'poll'` — defaults inert (no
   opt-in route, no hub). Load validation `DSP-111` (keepalive ≥ 1 s,
   maxSubscribers ≥ 1, maxRerunsPerSecond ≥ 1, perIpMax ≤ maxSubscribers,
   transport enum), JSON schema section regenerated into the checked-in
   artifact.
3. **Server substrate (`server::live` / `server::LiveHub`):** the pure SSE
   framing (`sse_frame`, spec-verbatim byte shapes for `data`/`ping`/
   `stale`), `Sub` (route, url, header snapshot, BTreeSet tags, `watch`
   latest-wins snapshot channel, seq, peer ip), `LiveHub` (exact +
   `prefix:*` wildcard tag index, `affected` fan-out deduplicated by
   identity, `Guard`-drop unsubscribes, global + per-IP capacity with
   rollback, `retag` reindexing after a rerun), `live_response` (the 200
   SSE head), `live_capacity_response` (503 + retry-after),
   `same_origin` (Sec-Fetch-Site then Origin-vs-Host authority compare),
   `live_stream` (current snapshot first — a fresh connection never needs
   a replay — one `data` frame per new snapshot with base64 payloads,
   keepalive pings one period after connect, clean end when the sender
   drops), and the `Runner` (batch debounce, per-subscriber rate cap,
   injected rerun closure, `None` = cleared snapshot = the stale signal).
4. **Data plane (dispatch):** `/__oxide/live` joins `is_data_plane_path`
   (the A04 pins reserved this flip for the route's WP) and
   `stage_build_check` reads the `build` query parameter on the live
   route only — `EventSource` cannot set headers (§V2-F05 step 2); a
   stale id answers `409` + `x-oxide-build-stale: true` exactly like the
   header spelling, absent-everywhere is still allowed.
5. **Render advertisement:** `hydration_bootstrap_with_live` emits the
   five-field `window.__oxide={route,build,live:true,data}`; the 3-arg and
   4-arg spellings stay byte-identical (the V8-linked CLI call sites
   compile unchanged); `Injector::with_live` opts a page in.
6. **Adapters:** `Capability::Live` joins the §18 matrix — Full on
   Binary/Docker/Node/Deno/Bun, Degraded on serverless and Workers (the
   degradation strings name the `transport: 'poll'` fallback), Unsupported
   on Static (`AST-LIV-001` at the future refusal-rule WP). `Target::ALL`
   added for matrix-row completeness pins.
7. **Client (`packages/core/src/client/live.ts`):** `connectLive(route,
   apply, opts)` per spec step 7 — the `/__oxide/live?route&url&build`
   EventSource URL, `data` → decode (base64 ODF; F06 swaps the one decode
   fn) → in-place apply, `stale` → close + reload once, jittered 1–3 s
   reconnect, the 30 s hidden-tab close and visibilitychange reconnect,
   full disposer. Transport/document/reload injectable for the Node test
   runner; import-safe in SSR (the `boot.ts` contract).

**Pre-existing landmines found and fixed on the way (hygiene debt, both
recorded in `docs/deviations.md`):**
1. **The workspace did not resolve at `v0.3.0`**: the A04 ship's
   version-bump commit (7447fb4) moved `[workspace.package] version` to
   0.3.0 while the 18 internal `[workspace.dependencies]` entries still
   demanded `0.1.0` — `cargo metadata`/`cargo fetch` fail on the tagged
   tree. Fixed by dropping the version reqs from the internal entries
   (path-only): lockstep versioning stays single-sourced in
   `[workspace.package]`, so ship.sh's version bump can never re-break
   the tree.
2. **The root `package.json` was invalid JSON** (`"version": "0.3.0",,`
   — a doubled comma left by the previous ship's version-bump edit,
   present on the tagged tree). Fixed, and this ship's version-bump step
   replaces the whole `"version": "…"` line value, never splicing commas.
3. **`--all-features` flipped the schema byte-gates red**: `bundler`'s
   feature graph pulls `serde_json/preserve_order`, which changes every
   `serde_json::Value` map from sorted to insertion order — the generated
   config schema emitted unsorted and `schema_stability.rs` +
   `schema_tdd.rs` failed (proven on a tree with only the pin fix
   applied: pre-existing). Fixed by canonicalizing `config_schema_pretty`
   through a BTreeMap projection (sorted keys under any unification; a
   no-op with the default map flavor).

## Where to pick up, in one sentence

The F05 wiring is now a call-site exercise on a V8-capable host: the
`/__oxide/live` endpoint in the dispatch closure (same-origin via
`server::same_origin`, build check via `stage_build_check` — the route
already joined the data plane —, subscribe-or-503 via `LiveHub::subscribe`,
`live_stream` as the body, the rerun closure calling the same data-route
path as `/__data` with `Sub.headers`, and the tag revalidation hook at the
`TieredCache::invalidate_tag` call sites plus `hub.retag`), the client
router lifecycle (`connectLive` after hydration when `__oxide.live`,
dispose on navigation), the A01 phase-3 split that unlocks it, then F06
(ODF PATCH deltas — its only dependency), F10's pool side, and the F-series.

---

# Handoff — V2-A04 build id + generations substrate landed (V8-free); all gates green

## Where things stand

**Branch:** `main`
**Working tree:** the milestone ships as uncommitted changes plus `ship.sh`
in the resolved downloads directory (bash 3.2-compatible, executable,
gitignored; one conventional commit per file, then idempotent PR + squash
merge + tag + push). Run `./ship.sh <repo>` to materialize the history.
**Verified this session (2 vCPU / 4 GB sandbox, no V8 toolchain):**
- `cargo test --all-features -p oxide-fw-cache -p oxide-fw-dispatch
  -p oxide-fw-render` — all green (248 tests across 16 suites; cache 82
  incl. 9 new build-id pins; dispatch 99 incl. 16 new build pins; render
  67 incl. 5 new bootstrap pins).
- `cargo fmt --all -- --check` — exit 0.
- `cargo clippy --no-deps -p oxide-fw-dispatch --all-targets
  --all-features -- -D warnings -W clippy::pedantic` — zero diagnostics.
- CI-style gate `cargo clippy --no-deps` over the 15-crate V8-free graph
  (`--all-targets`, `-D warnings`) — zero diagnostics. Excluded: `jshost`
  + `cli` (V8), `plugin` + `assets` (build needs `nasm`, absent here).
- `cargo test -p oxide-fw-graph -p oxide-fw-odf -p oxide-fw-server
  -p oxide-fw-compiler -p oxide-fw-bundler` — green (the allow-attribute
  sweep touched their test files).
- `cargo tree -p oxide-fw-dispatch | grep -c jshost` — 0 (spec acceptance).
- `packages/core`: `pnpm -C packages/core test` — 253/253 green (11 new
  build-contract pins), `tsgo --noEmit` clean.
- `cargo bench -p oxide-fw-cache --features bench-util --bench build_id`
  — `build_id/10` ≈ 72 µs, `/50` ≈ 297 µs, `/200` ≈ 1.22 ms (linear in
  covered-file count, ~6 µs/file: one open + blake3 pass per file).
- `cargo bench -p oxide-fw-dispatch --features bench-util --bench build`
  — `build_data_plane/*` 7–16 ns, `stage_build_check/{current,absent}`
  ≈ 14/11 ns, `advertise_build/{fresh,already_set}` ≈ 57/55 ns — the
  data-plane stage the wiring installs costs a string compare.
- Zero `#[allow(...)]` anywhere (`grep -rnE '#!?\[allow\(' crates/ tests/`
  is empty — pre-existing sites in graph/odf/server/cli/jshost/compiler
  test + lib files were removed properly, see `docs/deviations.md`).

## What landed

V2-A04 from `docs/spec-v0.2-draft.md` (build id and generations — the
keystone for F10 skew protection, F11 delta deploys, F13 learned prefetch,
F14 offline-first and F20 attested builds), in the A02/A05/A06/F04
substrate pattern:

1. **Canonical id (`oxide-fw-cache::build_id_of_dist`):** blake3 over the
   sorted `relative/path<TAB>blake3-hex` manifest of every file under
   `dist/client/**` and `dist/server/**` plus `.oxide/routes.manifest`
   (paths relative to the project root), truncated to the 16-hex
   `build_id` convention. Order-independent by construction (BTreeMap
   walk), tolerant of missing covered inputs, `CAC-BUILDID` on a missing
   dist. Pinned by 9 tests in `tests/build_id_tdd.rs` (the spec's named
   `build_id_of_dist_is_stable_and_order_independent` included).
2. **Generations + stage (`oxide-fw-dispatch::build`):** `GenerationSet`
   (current + the 1-deep demoted build accepted inside its window),
   `stage_build_check` (data-plane only: unaccepted `x-oxide-build` →
   `409` + `x-oxide-build-stale: true` + empty body; absent header
   allowed; case-insensitive lookup per B-06), `advertise_build`
   (idempotent, first-writer-wins), `is_data_plane_path` (today's data
   plane only — `/__oxide/live`/`stream` join when those routes land).
   Pinned by 16 tests in `tests/build_tdd.rs` (the spec's three
   `GenerationSet` pins verbatim).
3. **Browser surface (additive):** `render::hydration_bootstrap_with_build`
   emits `window.__oxide={route,build,data}`; the 3-arg bootstrap is
   byte-identical to pre-A04 (the V8-linked CLI call site compiles
   unchanged). `Injector::with_build_id` opts a page in.
4. **Client (`packages/core/src/client/build.ts`):** `buildId` /
   `withBuild` / `handleStale`, wired into the router's `/__data` fetch,
   the action POST (`enhance`) and `__oxide_call_fn` — every data-plane
   fetch advertises `x-oxide-build`, a stale 409 triggers one guarded
   reload per build id (sessionStorage, storage-blocked degrades to
   reload-once, Node-test-safe via `globalThis.location?.reload()`).
   11 pins in `test/build.test.ts` + the router/enhance/fn wiring pins.

**Docs drift recorded:** spec A03 item 1 claims the CI push branch filter
is the truncated `ain]` token — the tree already carries the correct
filter (verified byte-wise); the item is closed in the tree and the spec
text predates it. `.gitignore` carried pasted shell output from an earlier
session (`=== HEAD ===` …) — removed, and `.github/pr-body.md` is now
gitignored like `ship.sh`.

## Where to pick up, in one sentence

A01 remains the unblocked-milestone bottleneck: split
`crates/cli/src/lib.rs` (15,555 lines) per the spec's `scripts/split_cli.py`
plan on a machine that can link V8, then land phase 3 stage-by-stage — the
A04 wiring is now a call-site exercise (`build_id_of_dist` at the end of
`phase_emit` + writes to `dist/build-id`/`.oxide/build-id`,
`stage_build_check` before the L0 lookup, `advertise_build` on data-plane
responses, `Injector::with_build_id` from the render stage, and generation
promotion in `cmd_dev`'s reload), alongside the stage_middleware /
stage_prelude / stage_loaders / stage_experiments wirings, then
B03/B04/B08 and the F-series.

---

# Handoff — V2-F04 cache-safe personalization substrate landed (experiments engine + config surface); all gates green

## Where things stand

**Branch:** `main`
**Working tree:** the milestone ships as uncommitted changes plus `ship.sh`
in the resolved downloads directory (bash 3.2-compatible, executable,
gitignored; one conventional commit per file, then idempotent tag + push).
Run `./ship.sh <repo>` to materialize the history — the `v0.2.0` tag
already exists on origin and is skipped by design (no force, no re-push).
**Verified this session (2 vCPU / 4 GB sandbox, no V8 toolchain):**
- `cargo test -p oxide-fw-dispatch -p oxide-fw-config -p oxide-fw-router
  -p oxide-fw-schema` — all green (240 tests; dispatch 89 incl. 14 new
  experiments pins; config 61 incl. 7 new; router 79; schema 12+).
- `cargo fmt --all -- --check` — exit 0.
- `cargo clippy --no-deps -p oxide-fw-dispatch -p oxide-fw-config
  -p oxide-fw-router -p oxide-fw-schema --all-targets --all-features
  -- -D warnings -W clippy::pedantic` — zero diagnostics. **The lint
  policy is no longer dispatch/router/schema-only:** `config` now declares
  `[lints] workspace = true` (27 pedantic sites fixed, zero `#[allow]` —
  the 213-line `config_schema()` literal was split into per-section
  helpers instead of suppressing `too_many_lines`; `size.rs` avoids the
  flagged float→int casts with an exact decimal round-trip that preserves
  the old truncating/saturating semantics).
- CI-style gate `cargo clippy --no-deps` over the 15 crates compilable in
  this sandbox (`-D warnings`, all targets) — zero diagnostics. Excluded:
  `jshost` + `cli` (V8), `plugin` + `assets` (build needs `nasm`, absent
  here — the rav1e/wasmtime build scripts fail before Rust code runs).
- `cargo tree -p oxide-fw-dispatch | grep -c jshost` — 0 (spec acceptance).
- Zero `#[allow(clippy::*)]` / `#![allow(...)]` in the touched crates.
- `cargo bench -p oxide-fw-dispatch --features bench-util --bench
  experiments` — 5 ids: `assign/{2,8,16}` ≈ 10.2-10.4 µs per 100 assigns
  (~103 ns per assignment, flat in variant count — the blake3 bucket
  dominates), `resolve/3` (one experiment per sticky strategy) ≈ 665 ns,
  `resolve/24` (8×3) ≈ 6.6 µs (~220 ns per experiment, linear).

## What landed

V2-F04 step 1 from `docs/spec-v0.2-draft.md`, in the A02/A05/A06 pattern:

1. **Experiments engine (`oxide-fw-dispatch::experiments`):** the §15.4
   deterministic assignment — blake3 over `id || 0x00 || sticky_value`,
   `n % 100` traffic gate, `(n/100) % variants` variant pick; sticky
   cookie (mint on miss, `Path=/; Max-Age=2592000; SameSite=Lax;
   HttpOnly`), header, and ip-hash strategies; route-prefix eligibility
   (empty = all); `Resolved { assigned: BTreeMap<id, variant>,
   set_cookie: Vec<String>, vary_suffix: String }` with the id-sorted
   `;`-joined suffix. Pinned by 14 tests in `tests/experiments_tdd.rs`
   (spec's named pins: `assignment_is_deterministic`,
   `traffic_zero_assigns_nobody`, `traffic_100_assigns_everyone`,
   `distribution_within_5_percent_over_10000_ids` (empirically ≤ 1.2%
   deviation, well inside the 5% bound),
   `cookie_minted_when_absent_and_stable_when_present`,
   `vary_suffix_empty_without_experiments`).
2. **Locals integration:** `RequestLocals::set_experiments` feeds
   `Resolved.assigned` into the pipeline-owned slot; the DSP-101
   reserved-key report still guards it (pinned: a middleware staging
   `experiments` is reported and dropped; the resolved assignment wins).
3. **Config surface:** top-level `experiments: [{ id, traffic, sticky:
   {sticky: 'cookie'|'header'|'ip-hash', cookieName?, headerName?},
   variants, routes? }]` typed as `ExperimentConfig`/`StickyConfig`
   (internally-tagged, camelCase, `deny_unknown_fields`); load-time
   validation emits **DSP-110** for traffic outside 0–100 and fewer than
   two variants; the JSON schema declares the section and the checked-in
   `schemas/oxide-fw-config.schema.json` was regenerated (stability test
   green).

**Docs drift corrected by this milestone:** the spec's A03-hygiene item 2
claimed `benchmarks/report.md` is stale and feeds `gate.sh` — the report
now carries the B09-baseline table (verified); the spec's F04 pseudocode
predates the `Vec` set-cookie requirement (recorded in
`docs/deviations.md` as deliberate); the workspace-lints comment in the
root `Cargo.toml` said "the remaining V8-free crates adopt pedantic in a
dedicated hygiene pass" — `config`'s adoption this session is that pass
for the crate the milestone touched.

## Where to pick up, in one sentence

A01 remains the unblocked-milestone bottleneck: split
`crates/cli/src/lib.rs` (15,555 lines) per the spec's `scripts/split_cli.py`
plan on a machine that can link V8, then land phase 3 stage-by-stage
(`stage_experiments` between prelude and the L0 lookup is now a
substrate-only wiring: `resolve` + `set_experiments` + vary suffix +
set-cookie strip — see `docs/deviations.md`), then B03/B04/B08 and the
F-series.

---

# Handoff — V2-A05 middleware chain + unified locals substrate landed (V8-free); all gates green

## Where things stand

**Branch:** `main`
**Working tree:** the milestone ships as uncommitted changes plus `ship.sh`
at the repo root (bash 3.2-compatible, executable, gitignored; one
conventional commit per file, then idempotent tag + push). Run `./ship.sh`
to materialize the history — the `v0.2.0` tag already exists and is skipped
by design.
**Verified this session (2 vCPU / 8 GB sandbox, no V8 toolchain budget):**
- `cargo test -p oxide-fw-dispatch` — 67/67 green (26 unit; flow 3; url 5;
  loads 11; chain 13; locals 9).
- `cargo fmt --all -- --check` — exit 0.
- `cargo clippy --no-deps -p oxide-fw-dispatch --all-targets --all-features
  -- -D warnings -W clippy::pedantic` — zero diagnostics. Zero
  `#[allow(clippy::*)]` / `#![allow(...)]` anywhere.
- `cargo tree -p oxide-fw-dispatch | grep -c jshost` — 0 (spec acceptance).
- `cargo bench -p oxide-fw-dispatch --features bench-util --bench
  middleware` — 5 new ids: `chain/noop/{1,2,4}` ≈ 139/176/250 ns (the
  substrate costs ~40 ns per extra layer), `chain/locals_merge_4` ≈
  0.76 µs, `locals/to_value` ≈ 1.13 µs, `locals/apply_middleware` ≈ 126 ns.

## What landed

V2-A05 step 1 from `docs/spec-v0.2-draft.md`, in the A02/A06 pattern:

1. **Chain runner (`oxide-fw-dispatch::chain`):** `run_chain` walks the
   route's ancestor `middleware.ts` chain outer to inner through the real
   `MiddlewareDispatcher::dispatch(MiddlewareDispatchArgs)` seam. It
   mirrors the inline loop's production semantics: locals merge
   later-wins, each layer sees `None` while nothing has accumulated,
   headers accumulate last-writer-wins per case-insensitive name (B-06),
   `short`/`redirect` stop the walk with the raw `MiddlewareRunResult`
   (the caller keeps the location-header and session write-back mapping),
   a rewrite returns the target with the rewriting layer as the B-22
   resume marker, and the resumed pass skips everything before (and
   including) the marker. Pinned by 13 tests in `tests/chain_tdd.rs`.
2. **Unified locals (`oxide-fw-dispatch::locals`):** `RequestLocals`
   composes the one JSON value loaders, actions, endpoints and render
   receive; the reserved keys `session` / `locale` / `experiments` are
   pipeline-owned and win over user-set keys in `to_value()`, and
   `apply_middleware_locals` reports every reserved key a middleware
   staged (the DSP-101 warning sites) without letting it land. Pinned by
   9 tests in `tests/locals_tdd.rs`.

**Docs drift corrected by this milestone:** the spec's A05 "evidence"
(nearest-only middleware) predates the inline loop's full-chain §8.5 fix;
its `run_chain` pseudocode predates both the args-struct dispatcher seam
and the B-06/B-22 fixes — the substrate implements production, and
`docs/deviations.md` records the deltas as deliberate.

The CLI `stage_middleware` / `stage_prelude` wiring stays deferred (see
`docs/deviations.md`): the closure keeps its inline loop until the A01
phase-3 move can compile `oxide-fw-cli` against this crate.

## Where to pick up, in one sentence

A01 next: split `crates/cli/src/lib.rs` (15,555 lines) per the spec's
`scripts/split_cli.py` plan on a machine that can link V8, then land phase
3 stage-by-stage on top of this crate (`stage_middleware` switches to
`run_chain` + `RouteContextTable.middleware_chain`, `stage_prelude` builds
`RequestLocals`, `stage_loaders` switches to `run_load_graph`), then
B03/B04/B08 and the F-series.

---

# Handoff — V2-A06 parallel load graph landed (scanner verdict + dispatch substrate); all gates green

## Where things stand

**Branch:** `main`
**Working tree:** the milestone ships as uncommitted changes plus `ship.sh`
at the repo root (bash 3.2-compatible, executable, gitignored; one
conventional commit per file, then idempotent tag + push). Run `./ship.sh`
to materialize the history — the `v0.2.0` tag already exists and is skipped
by design.
**Verified this session (2 vCPU / 8 GB sandbox, no V8 toolchain budget):**
- `cargo test -p oxide-fw-dispatch -p oxide-fw-router -p oxide-fw-schema`
  — 143/143 green (dispatch 45 incl. 12 load-graph pins; router 82 incl.
  the new token-exact `parent()` scanner pins; schema 10).
- `cargo fmt --all -- --check` — exit 0.
- `cargo clippy --no-deps -p oxide-fw-dispatch -p oxide-fw-router
  -p oxide-fw-schema --all-targets --all-features -- -D warnings
  -W clippy::pedantic` — zero diagnostics. **The lint policy is no longer
  dispatch-only:** `router` and `schema` now declare
  `[lints] workspace = true` (107 pedantic sites fixed this session, zero
  `#[allow]` — the scanner walk and the typegen emitter were split into
  helpers instead of suppressing `too_many_lines`).
- CI-style gate over the full V8-free graph (13 crates, `-D warnings`) —
  zero diagnostics.
- `cargo tree -p oxide-fw-dispatch | grep -c jshost` — 0 (spec acceptance).
- `cargo bench -p oxide-fw-dispatch --features bench-util` — 12 bench ids;
  the new `loads/*` group pins the §9.2 win: `parallel_8` ≈ 2.10 ms vs
  `chain_8` ≈ 16.60 ms (eight 1 ms loads: overlapped vs serialized ≈ 8×),
  graph overhead ~1.1 ms flat from 8 to 16 nodes; `ctx_table/get` still
  flat ~32.5 ns at 100/1000 routes.

## What landed

V2-A06 from `docs/spec-v0.2-draft.md` ("Parallel layout loads", §9.2), in
the spec's two steps:

1. **Scanner (`oxide-fw-router`):** the `needs_parent` verdict is now
   token-exact (`calls_parent`, whole-identifier match — `parentDir`,
   `grandparent()`, `parent_data` no longer serialize a load) and applies
   to **both** pages and layouts (pre-A06 only pages were probed and
   layouts always reported `false`, so a `parent()`-calling layout could
   never be scheduled behind its own parent). The over-approximation note
   from the spec is kept: a mention inside a comment/string still marks
   the load — a false positive only makes loads sequential, never
   incorrect. Pinned by new scanner tests (`parent_token_detected`,
   `parentheses_not_enough_and_substrings_never_count`, layout-verdict
   pins).
2. **Load graph (`oxide-fw-dispatch::loads`):** `run_load_graph` takes the
   node list (`LoadNode { input, needs_parent }`) and a shared
   `Arc<dyn LoaderRunner>`; independent nodes start concurrently,
   a `needs_parent` node awaits its predecessor's slot (`futures::Shared`)
   and receives its data as `LoaderRunInput::parent_data`. Results come
   back in **input order** (never completion order), the first
   short-circuit **in input order** wins, a runner crash maps to
   `LoadResult::Failed` and poisons only its dependents, and independent
   loads still run when an outer load short-circuits (§9.2 requires
   idempotent loaders — the over-run is pinned so cancel-on-short-circuit
   would be a deliberate change). Time-sensitive pins run on the paused
   tokio clock (`start_paused`) — no wall-clock flakes.

The CLI `stage_loaders` wiring stays deferred (see `docs/deviations.md`):
the closure keeps its sequential loop until the A01 phase-3 move can
compile `oxide-fw-cli` against this crate.

## Where to pick up, in one sentence

A01 next: split `crates/cli/src/lib.rs` (15,555 lines) per the spec's
`scripts/split_cli.py` plan on a machine that can link V8, then land phase
3 stage-by-stage on top of this crate (`stage_loaders` switches to
`run_load_graph` + the `RouteContextTable.layout_loads` verdicts that are
already threaded), then B03/B04/B08 and the F-series.

---

# Handoff — V2-A02 `oxide-fw-dispatch` substrate landed; V8-free, all gates green

## Where things stand

**Branch:** `main`
**Working tree:** NOT committed — the milestone ships as uncommitted changes
plus `ship.sh` at the repo root (bash 3.2-compatible, executable,
gitignored; one conventional commit per file, then tag + push). Run
`./ship.sh` to materialize the history and publish the `v0.2.0` tag.
**Verified this session (2 vCPU / 8 GB sandbox, no V8 toolchain budget):**
- `cargo test -p oxide-fw-dispatch` — 33/33 green, 1.3 s wall after deps
  (spec acceptance: < 20 s).
- `cargo fmt --all -- --check` — exit 0.
- `cargo clippy --no-deps -p oxide-fw-dispatch --all-targets --all-features
  -- -D warnings -W clippy::pedantic` — zero diagnostics. The crate opts
  into the new `[workspace.lints]` policy (`[lints] workspace = true`), so
  pedantic also holds on every future plain `cargo clippy` of the crate.
- `cargo clippy -p oxide-fw-dispatch -p oxide-fw-router -p oxide-fw-cache
  --all-targets -- -D warnings` — zero diagnostics (CI-style gate over the
  V8-free graph).
- `cargo tree -p oxide-fw-dispatch | grep -c jshost` — 0 (spec acceptance).
- `cargo bench -p oxide-fw-dispatch --features bench-util` — 8 bench ids;
  `ctx_table/get` is flat ~33 ns from 100 to 1000 routes (O(1) dispatch
  answers), `url/*` helpers 8.8–431 ns (numbers in `docs/archive/WORKLOG.md`).

## What landed

V2-A02 from `docs/spec-v0.2-draft.md`: the V8-free `oxide-dispatch` crate
(package name `oxide-fw-dispatch`, matching the repo's `oxide-fw-*`
convention the spec text predates). `crates/dispatch/{src/url.rs,src/flow.rs}`
+ a criterion bench target gated behind `bench-util` (the router/cache
convention). `url.rs` carries the spec's prelude helpers (`path_of`,
`normalize_path`, `data_route_of`, `MAX_REWRITE_HOPS`) plus the §9.3 pair
`parse_form_urlencoded` / `action_from_query` moved verbatim from
`crates/cli/src/lib.rs:8133`. `flow.rs` carries the stage protocol
(`Flow::Continue | Respond(OxideResponse)`). TDD evidence: the tests were
written first and the red run captured (unresolved-symbol errors for every
symbol under test), then implementation, then bench.

The CLI keeps its private copies until phase 3 (see `docs/deviations.md`,
"V2-A02 dispatch substrate — deferred CLI adoption"). `crates/cli/Cargo.toml`
already declares the dependency so the integration PR is code-only.

## Where to pick up, in one sentence

A01 next: split `crates/cli/src/lib.rs` (15,555 lines) per the spec's
`scripts/split_cli.py` plan on a machine that can link V8, then land phase 3
stage-by-stage on top of this crate (the stages' pure halves are already
here and tested), then B03/B04/B08 and the F-series.

---

# Handoff — V2 Wave B foundations landed (B09/B02/B01/B05/B06); CLI integration deferred

## Where things stand

**Branch:** `main`
**Working tree:** NOT committed — the milestone ships as uncommitted changes
plus `commit.sh` at the repo root (one conventional commit per file; the
script is executable and gitignored by design — see "Commit protocol"
below). Run `./commit.sh` to materialize the history.
**V8-free crates:** `cargo test -p oxide-fw-router -p oxide-fw-cache` green;
clippy-clean at `-D warnings`.
**V8 crates:** untouched, not compiled in the milestone sandbox (no V8
toolchain budget: 2 vCPU / 4 GB). The deferred integration points are
itemized in `docs/deviations.md` ("V2 Wave B foundations — deferred
integration points").

This session implemented the next architecture milestone identified from
`docs/spec-v0.2-draft.md` (Track 2 "algorithms" + the B01 substrate, which
the spec's own execution order puts first):

| WP | What landed | Result |
| --- | --- | --- |
| B09 | criterion harness + `baseline-v01.json` + gate scripts | 22-ids baseline; gate proven (slowed matcher → exit 1) |
| B02 | `CandidateIndex` + zero-copy `match_ref` (D23) | 19.4× at 1k static routes; 0 heap allocs; 10k-case oracle proptest |
| B01 | `RouteContextTable` (D22) | boot-time dispatch answers; oracle-verified vs `layout_chain` |
| B05 | `KeyStr`/`key_of` + reverse tag index (D24/D25) | stack keys; idempotent tags; eager delete; retag fix |
| B06 | redb multimap disk tags + v1→v2 migration | disk 5-tag set 8.7× faster; migration covered by test |

Numbers and the two deliberate cost moves (`build/*` one-time index
construction, `tag_insert` reverse-index price) are in
`benchmarks/report.md` ("Criterion microbenchmarks"); the session log is
appended to `docs/archive/WORKLOG.md`.

## Commit protocol (this milestone only)

`./commit.sh` (repo root, bash 3.2-compatible, executable, gitignored):
walks the changed/new files in dependency order and creates **one
conventional commit per file** with a scoped, descriptive subject
(`feat(router): ...`, `feat(cache): ...`, `bench(benchmarks): ...`,
`docs(...): ...`). Nothing is committed until you run it.

## Where to pick up, in one sentence

Next work per the spec's order: A01 (split `crates/cli/src/lib.rs`, now
15,555 lines) → A02 (`oxide-dispatch` crate) → wire `match_ref` +
`RouteContextTable` + `KeyStr` into the dispatch/L0 paths (the exact
integration points are listed in `docs/deviations.md`), then B03/B04/B07/B08
and the F-series.

---

# Handoff — BUG-MAPKEY + BUG-SOLID-CLIENT fixed; every gate green

## Where things stand

**Branch:** `main`
**Working tree:** committed (see the latest `fix:` commits).
**Binary:** `cargo build -p oxide-fw-cli` (dev profile, `CARGO_PROFILE_DEV_DEBUG=0`).

The React-only HMR plan and the multi-framework HMR rollout (previous
handoffs) are intact. This session closed out the last red gates: the
production-build fixture gate (smoke-app-{vue,svelte,solid}) and the Solid
browser HMR suite, plus the create-oxide environment-dependent test.

## What landed this session

### 1. BUG-MAPKEY — raw SFCs reached Rolldown's parser (prod builds, vue/svelte/solid)

Symptom: `oxide build` died with the file-less
`[BLD-ROLLDOWN] rolldown write: Unexpected JSX expression` on every
non-React fixture (solid additionally: `"jsx" is not exported by
"@solidjs/web"`).

**The controlled experiment** (minifier hypothesis, forced minify off in the
backend via a temporary env override): the failure persisted with minify
off — hypothesis DISPROVEN, experiment override removed after.

Actual root cause, found by graph bisection + in-browser probes:
`resolve_relative` (`crates/cli/src/precompile.rs`) produced precompiled-map
keys with `..` segments (`/proj/src/routes/index/../../components/a.vue`)
while Rolldown resolved the same import to the normalized spelling. The
`PrecompiledPlugin` lookup missed, the RAW `.vue`/`.svelte` file flowed into
the parser (a `<template>` block parses as JSX), and Solid's raw `.tsx`
took the §6.3 automatic-runtime transform against `@solidjs/web` (no
`jsx`/`jsxs` exports there).

Fix (three layers):
- `resolve_relative` normalizes every returned path (the map-key contract).
- `PrecompiledPlugin::new` / `OxidePassesPlugin::new` normalize keys;
  `lookup` also tries the normalized id.
- NEW guard: `OxidePassesPlugin` gained a `load` hook — a raw `.vue` /
  `.svelte` / `.mdx` id with no precompiled content now fails the build
  with a NAMED `BLD-SFC` diagnostic (the old failure was undiagnosable).

### 2. BUG-SOLID-CLIENT — the browser silently painted the IR fallback (Atlas)

Symptom: `hmr-solid` browser tests got `h1 = "Hello Atlas"` (the adapter's
built-in §22 reference tree) while the server sent `<h1>Home</h1>`, with NO
console errors and NO failed requests.

Root cause: `packages/solid/src/{real-client,hydrate-island-real}.ts`
loaded the Solid web runtime via a VARIABLE-specifier `import(name)`.
Neither pipeline can rewrite a dynamic variable import: the dev prebundle
rewrites static literals to `/@prebundle/*.js`, and Rolldown resolves
literals from node_modules. The browser rejected the bare specifier with
`Failed to resolve module specifier` BEFORE any request; the catch turned
that into `undefined` → `mountSolidPageRoot` → silent IR fallback.
Vue/Svelte never hit this because their loaders use string literals.

Fix: `@solidjs/web` is now a static literal (rewritable + resolvable); the
Solid-1 `solid-js/web` fallback is assembled at runtime with
`['solid-js', 'web'].join('/')` — deliberate, because (a) as a literal
Rolldown fails every prod build with `Could not resolve 'solid-js/web'`
(Solid 2 has no `./web` export) and (b) esbuild constant-folds `'a' + 'b'`
back into the banned literal. `./real-ssr.ts` keeps its variable form
(Node resolves bare specifiers from real node_modules).

### 3. Console-noise fix: react-refresh runtime probe

The HMR runtime imported `/@prebundle/react-refresh-runtime.js` on EVERY
ESM-dev page. Non-React projects don't prebundle it → guaranteed 404 →
Chromium console error → the per-framework specs' error-free-console
assertion failed. `packages/core/src/client/hot.ts` now fetch-probes the
URL first (a 404 fetch is silent) and imports only on `ok`; a miss throws
into the existing `.catch` so `$RefreshRuntime$` stays unset and the
full-reload fallback semantics are unchanged.

### 4. BUG-PWD — create-oxide's workspace detection was environment-dependent

`packageJson()` chose `workspace:*` vs `0.1.0` via
`process.env['PWD'].includes('packages/')`. `PWD` doesn't track
`process.chdir()`, so `pnpm test:packages` from the repo root failed the
scaffold e2e while `pnpm -C packages/create-oxide test` passed. Fixed:
`isInsidePnpmWorkspace(dir)` walks up from the OUTPUT directory looking for
`pnpm-workspace.yaml`; `scaffold()` passes it explicitly; `packageJson()`
defaults to published mode. Tests updated + two new deterministic tests
(workspace layout pins `workspace:*`; walk-up detection).

### Test-pyramid additions this session

- `crates/bundler`: 9 unit tests (lookup normalization × 3, BLD-SFC guard × 6).
- `crates/cli`: 4 unit tests pinning `resolve_relative` normalization.
- `packages/create-oxide`: workspace-detection tests (+ fixed expectations).
- The e2e gates themselves are the integration/e2e regression net:
  `tests/fixtures/check.sh` (imported component SFCs through prod builds for
  all four frameworks), `tests/fixtures/plugins/check.sh` (7 cases incl.
  virtual modules + generateBundle), and `hmr-solid.spec.ts` (pins the
  client-loader contract end to end).

## Verified test state (this sandbox, chromium, `--workers=1 --retries=0`)

- **Gates**: `tests/fixtures/check.sh` 4/4 (react+vue+svelte+solid),
  `tests/fixtures/plugins/check.sh` 7/7.
- **Rust**: cli lib 257 (+MDX ignored test green via `--ignored`), bundler
  lib 27, assets 125, schema 4+6.
- **Node**: `pnpm test:packages` 598/598 (repo root — the PWD flake is
  dead); typecheck clean for core/create-oxide/vue/svelte/solid.
- **Playwright**: smoke+island 7, react HMR group 26+6, Group B 5,
  Group C 17, `hmr-vue` 7, `hmr-solid` 7, `hmr-svelte` 7 — all green.
- **Not runnable in this environment** (pre-existing, unchanged):
  webkit (missing GTK4/GStreamer), `sw.spec` ×2 (service-worker precache
  empty in headless chromium here).

## Sandbox notes for the next session

All previous HANDOFF gotchas still apply (60s beforeAll budgets, serial
gate, pristine-fixture syncing, `pkill` by PID, core dist rebuild after
`hot.ts`/`boot.ts` edits, staleness-scan scope, disk pressure dedupe,
`nasm` on PATH for wasmtime). Two additions:

1. **Adapter dist rebuild after touching `packages/*/src`**: the dev
   prebundle and the prod build both consume `packages/*/dist`. `pnpm -C
   packages/<fw> build` (tsdown) — then clear
   `examples/smoke-app-*/.oxide/esm-prebundle` so stale prebundles don't
   mask the change.
2. **Specifiers in adapter CLIENT code must be string literals** (rewritten
   by the dev prebundle, resolved by the prod bundler); a variable-specifier
   import silently kills the browser path. If a specifier must stay
   un-resolvable by Rolldown (optional peers that don't exist under the
   target runtime), assemble it at runtime with `['a','b'].join('/')` —
   NOT `'a' + 'b'` (esbuild constant-folds concatenation back into the
   literal that breaks the prod build).

## Where to pick up, in one sentence

Every gate is green; next work is publishing `solid-jsx-oxc` linux binaries
(the babel fallback stays the portable default), the deferred §3.8b browser
ESM-cache proxy, and keeping the suite green in CI (webkit needs a normal
machine).
