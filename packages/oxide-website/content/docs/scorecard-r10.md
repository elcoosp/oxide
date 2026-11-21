---
title: Compliance scorecard — r10
description: The current spec-compliance scorecard for Oxide v0.1. Every section, every finding, every verdict — kept in the repository so the audit posts can cite it directly.
order: 101
---

# Oxide v0.1 spec-compliance scorecard — r10

**Range audited:** `663201a..8d44700` (3,282 commits; full history at HEAD is 4,815 commits).
**Overall:** r9 ~93 / 100 — **not re-scored this round** (see method and the
un-audited statement). What r10 does re-verify: all ten P0 rows at HEAD with
`file:line` evidence, the CI job posture at the two trunk runs, and the
local-verifiability gates (the "is every documented gate runnable" audit the
trunk red made urgent).

## Method

r9 ran 4 parallel read-only verification agents over every commit and doc.
r10 is a **single-agent, targeted re-verification**, run because the trunk
has been red since `8bcc462e` (2026-10-29): the P0 master table re-verified
at HEAD against source (every verdict below carries a `file:line`), the CI
job table assembled from the two red trunk run ids (job-level conclusions
for individual runs are **NOT VERIFIED** — the GitHub Actions API was
rate-limited to 0 from this network for the whole session; the failing-step
names come from the trunk red of record), and the §5 local gates executed
for real with transcripts (rustc 1.98.0, vendored nasm 2.16.03 and
libclang 18.1.1 — the sandbox walls the F47–F70 records documented were
worked around, not assumed). Static counts re-measured: **3,276** Rust test
functions (`scripts/count-tests.sh`, +1 this round for the AVIF capability
refusal pin).

## The master P0 table, re-verified at HEAD

| # | Item | r9 verdict | r10 verdict at HEAD | Evidence (file:line) |
|---|---|---|---|---|
| P0-1 | core-wasm never emits its wasm | PARTIAL | **PARTIAL (unchanged)** | `crates/cli/resources/` holds only `edge.wasm` — no `edge.js` glue sibling; the payload stays inert-by-fallback. `docs/deviations.md:74` still records "partially closed (payload inert)", which is what the wasm32-edge B3 job greps. |
| P0-1c | edge bundle emission | PARTIAL | **PARTIAL (unchanged)** | same evidence as P0-1: `crates/cli/resources/` contains exactly one file (`edge.wasm`); no `edge.js` was committed. |
| P0-2 | Worker defer path buffers | PARTIAL | **PARTIAL (unchanged)** | sequential streaming is live; the `/__data` envelope still materializes every resolved defer value into one uncapped payload (`crates/cli/src/data_route.rs:247`–`355`: the `deferred` BTreeMap and the single `encode_to_bytes` envelope at :371). The main request-body path IS capped (`crates/server/src/lib.rs:5995`–`6010`, `SRV-413` / `limits.max_body`) — the unbounded half is the response-envelope side. Fallback-M2 candidate. |
| P0-3 | Parallel layout loads (§9.2 MUST) | CLOSED | **CLOSED (confirmed)** | `crates/cli/src/lib.rs:33`–`49`: `LayoutLoadSpec` — "the §9.2 load graph runs the layout concurrently unless its own verdict says it awaits its predecessor's data". |
| P0-4 | PPR orphaned | PARTIAL | **PARTIAL (unchanged)** | runtime shell cache wired and bounded (`crates/cli/src/lib.rs:221`–`230`, `PPR_SHELLS_MAX = 256` with eviction); still no build-time shell, no rebuild invalidation. |
| P0-5 | Session write path | CLOSED + D1 ext PARTIAL | **PARTIAL residues confirmed** | the six serve paths persist; the short-circuit arms remain the documented residue (`crates/cli/src/data_route.rs:143`–`144`, `:184`: the layout short-circuit propagates through the data response; session handling rides the same arm at `:296`). |
| P0-6 | h3 body + streaming | CLOSED (r9) | **CLOSED (confirmed)** | `crates/config/src/schema.rs:225`–`226`: `http3` + `h3Advertise` schema'd. |
| P0-7 | Middleware single-layer | CLOSED (r5) | **CLOSED (not re-swept)** | no regression signal in the red trunk; the r5 closure stands. |
| P0-8 | HttpCacheHandler unsealed L2 | CLOSED (r9) | **CLOSED (confirmed)** | `crates/cache/src/auth.rs:6`–`64`: the sealed keyed-BLAKE3 envelope is the L2 wire format. |
| P0-9 | Single-flight poisoned key | CLOSED (r7) | **CLOSED (not re-swept)** | no regression signal in the red trunk; the r7 closure stands. |
| P0-10 | Routes authoring model violates §14/§8.1 | PARTIAL | **PARTIAL — one half closed since r9** | r9's open finding 5 ("`oxide render` attaches no ComponentSource") is **stale**: `crates/cli/src/commands/render.rs:125`–`136` resolves the SSR entry and attaches the component source (`input["components"] = {entry}`). The `ssr: 'real'` default remains undecided (the smoke app sets it explicitly); `adapter_entry` handling was not re-verified this round. |

## CI job table (trunk red of record: runs 44826409281 / 44826407648, 2026-11-20)

Per-job run-level conclusions are NOT VERIFIED (API rate limit); the
red/green split is the trunk red of record, and each row names the local
gate that reproduces or repairs it.

| job | state at the red trunk | failing step | local parity (r10 session) |
|---|---|---|---|
| rust (3 OS) | RED | `cargo clippy --workspace --all-targets -- -D warnings` | reproduced exit 101 (4 float_cmp in search + the assets test-compile break); after the r10 fixes: exit 0 **with no excludes** |
| msrv 1.98 | RED | `cargo check --workspace --all-targets` | reproduced exit 0 with excludes at pristine HEAD; the excludes were cli/assets — repaired by the capability gates + test-pin fixes; full-workspace exit 0 (see gates transcript) |
| node | RED | `Build the CLI` | the job had NO `Install nasm` step (every other CLI job does) — step added (`ci.yml`); the M1 avif gate also takes ravif out of the default build |
| cargo-deny | RED | licenses/advisories | reproduced: deprecated `unmaintained`/`unsound` keys (cargo-deny PR #611 removed them) failed config validation; fixed by removing the keys, the wasmtime 48.0.10 patch bump (8 RUSTSEC-2026-05xx vulns fixed upstream), and two exact-id unmaintained ignores with reachability. `cargo deny check licenses advisories` → exit 0 |
| smoke-deploy node/docker/cloudflare | RED | `Build the smoke app for the target` | M2: the `oxide build` regression cluster — bisect evidence in the session handoff |
| cli-split-digest, HMR, Playwright ×4, search-feature pins, V8 fixture, scaffold-install ×4 | RED | `Digest each smoke app at HEAD` / `Build CLI` / `Build smoke-app-react` / the serve wire-up pins | same `oxide build` regression cluster as smoke-deploy — one root cause, M2 |
| fmt, docs-check, conformance ×3, wasm32-edge (P0-1c), plugin-fixtures, spec-anchor, doc-rot, launch-content, ODF fuzz, deploy-s3 pins, Redis L2 | GREEN | — | green of record; `cargo fmt --all --check` re-run exit 0 |

## Closed this round

- **The workspace is un-walled.** The two capability gates (assets `avif`,
  jshost/cli `v8`) make `cargo clippy --workspace --all-targets -- -D
  warnings` and `cargo +1.98.0 check --workspace --all-targets` runnable
  with **no excludes** for the first time since the V8/nasm walls were
  recorded (F47–F70 deviations). Both run green in this session, plus
  `cargo nextest run --workspace` and the `--no-default-features` CLI link.
- **The 5-session `oxide-fw-assets` test-compile break is repaired** (the
  rkyv-skew pins: `crates/assets/tests/integration.rs:69/:99` — the source
  type `RouteSearchEntry.title: Option<String>` is documented-correct, the
  pins predated it; fixed via `as_deref`, zero source behavior change).
- **cargo-deny config repaired for cargo-deny PR #611** + wasmtime 48.0.10
  + two exact-id unmaintained ignores (reachability notes in `deny.toml`).
- **`node` job's missing `Install nasm` step added** (R1-compliant: a
  missing toolchain step, no gate softened).
- **9 clippy findings cleared** (4 float_cmp in search, 4 in dispatch,
  1 useless_format in assets) — all test pins, zero `#[allow]`.
- **README drift fixed**: the unsupported "21-section scorecard" phrasing
  dropped, r10 cited, the test count re-measured to 3,276.

## Open findings, honestly

1. **The trunk is red at HEAD** — the `oxide build` regression cluster
   (smoke-deploy ×3, cli-split-digest, HMR, Playwright ×4, search pins, V8
   fixture, scaffold-install ×4) is this round's M2 scope; see the session
   record for the bisect result.
2. **PPR is a runtime shell cache**, not the §10.4 build-time feature
   (unchanged from r9 — don't claim it in marketing).
3. **WP-D1's short-circuit session residue is structurally open** (r9
   finding 4 — confirmed still true at HEAD, `data_route.rs:143`–`184`).
4. **K2/K3 hazards from r9** (the search index served `immutable` though
   unhashed; `sw.offlineFallback` not precached) were **not re-verified**
   this round.
5. **The `% of history fully swept` did not move**: r9's 4-agent sweep
   covers 1,533 / 4,815 commits (31.8%). The 3,282 commits since `663201a`
   were NOT swept commit-by-commit; r10's verification is the targeted P0 +
   CI + gates audit above. The next full sweep (r11) should ride the
   restored green CI.

## Watchlist for r11

- Is CI green at HEAD? (r10 repairs the gates; the `oxide build` cluster
  must land too.)
- The AVIF capability's first real consumer: `images.formats` still defaults
  to `["avif","webp"]` — a default-build request now falls back with `AST-AVIF`.
  Decide whether the default formats list should drop `avif` when the feature
  is absent, or keep the fallback posture (r10 keeps the fallback; the schema
  text is unchanged).
- P0-2's response-envelope cap (streaming/size-capped `/__data`) — the
  remaining unbounded `collect`-shaped buffer.
- The `unmaintained` direct deps (chrono, rustls-pemfile): schedule the
  jiff / rustls-pki-types migrations so the ignores can retire.
- The `ssr: 'real'` factory default — still undecided (r9 watchlist).

*The r10 audit's findings section is summarised here; the source of truth
for any specific claim is the commit or file cited in the table, not this
page.*
