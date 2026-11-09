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
