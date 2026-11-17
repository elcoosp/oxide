# v0.2 release prep (R06) — verified triggers, changelog draft, version bumps

Status: **prepared 2026-11-16** at HEAD `379e138b`. This file holds the
three things the tag commit needs: the verified release machinery, the
changelog draft (the mechanical regeneration happens at tag time), and the
exact version-bump commands with the two drift facts they must reconcile.
Nothing here mutates versions — the bump is executed by a human/CI at tag
time.

## 1. Release machinery — verified intact

| Workflow | Trigger | What it does | Verified |
| --- | --- | --- | --- |
| `release.yml` | `push: tags: v[0-9]+.[0-9]+.[0-9]+` | Regenerates `CHANGELOG.md` with git-cliff (`--tag <name>`), extracts the tag's section as the release body, commits the changelog back to main, creates the GitHub Release (prerelease when the tag carries `-`) | header, `on:` block, all four steps read at HEAD; `permissions: contents: write` present |
| `release-binaries.yml` | same tag trigger | Builds the per-target release binaries (linux gnu/musl × x64/arm64, macOS, Windows), attaches them to the release created by `release.yml` with `SHA256SUMS`, an SBOM and build provenance (F20 attest) | trigger verified; carries the search feature (87f93554) |
| `publish-npm.yml` | **`workflow_dispatch` only** (deliberate) | Verifies every publishable package.json equals the input version, builds workspace packages, dry-runs `pnpm pack`, then publishes via OIDC Trusted Publishing (`--provenance`, no NPM_TOKEN) | the header records why it is not tag-wired: a registry publish is immutable and not safely retryable, so the trigger is a deliberate human action after the tag exists and crates are published |

A `v0.18.0` tag fires the first two immediately; the npm publish is then
driven manually (`gh workflow run publish-npm -f version=0.7.0` — dry-run
first, the default). **Gate item:** `publish-npm.yml` loops
`create-oxide core react tailwind vue svelte solid` and fails on any
version mismatch — today `packages/tailwind` sits at **0.6.0** while its
six siblings sit at **0.6.1** (Drift fact D1): no input version satisfies
the check until tailwind is aligned (Section 3).

## 2. Version bump — target `0.18.0` (crates) / `0.7.0` (packages)

The repo runs **two version cadences** (recorded so nobody "fixes" either):
the Cargo workspace + the root `package.json` move together
(`chore(release): v0.17.0` touched exactly `Cargo.toml`, `Cargo.lock`,
`package.json`); the seven publishable npm packages move in their own
lockstep (0.6.x). `scripts/bump-version.sh` enforces "every surface at the
same version" and therefore **refuses to run** while crates sit at 0.17.0
and packages at 0.6.x (Drift fact D2) — use the manual commands below, or
reset the script's precondition in a separate chore first.

### Crates + root package (the `chore(release)` pattern, commit for commit)

```bash
# 1. workspace version — exactly one match at [workspace.package]:
grep -n '^version = "0.17.0"' Cargo.toml          # expect 1 hit
sed -i 's/^version = "0.17.0"/version = "0.18.0"/' Cargo.toml
# 2. root package.json:
python3 - <<'PY'
import json; p='package.json'; d=json.load(open(p)); d['version']='0.18.0'
json.dump(d, open(p,'w'), indent=2); open(p,'a').write('\n')
PY
# 3. Cargo.lock self-refresh (the oxide-fw-* own-version entries):
cargo update -w --dry-run && cargo metadata >/dev/null   # rewrites the lock
# 4. commit: chore(release): v0.18.0
```

### Packages (their own lockstep: 0.6.x → 0.7.0)

```bash
# 0. align the tailwind drift first (D1):
pnpm -C packages/tailwind exec npm version 0.6.1 --no-git-tag-version   # or straight to 0.7.0 below
# 1. bump all seven publishable packages:
for p in create-oxide core react tailwind vue svelte solid; do
  pnpm -C packages/$p exec npm version 0.7.0 --no-git-tag-version
done
# packages/cli-npm and packages/oxide-og are private/non-publishing; bump
# them in the same pass only if their lockstep policy says so.
# 2. commit: chore(release): packages v0.7.0
```

### `contractMinor` (spec V2.2.5, R06 step 2)

The spec expects adapters to carry the new optional `contractMinor?: 1`
field next to `contract: 1`. **No `contractMinor` exists anywhere in
`packages/*/src` at HEAD** — the field is unimplemented. It is additive and
optional (the core refuses only a HIGHER value than it knows), so it does
not block the tag, but the spec's step 2 names it: land
`contractMinor: 1` on the four adapters + the core's `refuses-higher`
check in the release PR, or record its deferral in `docs/deviations.md`
before tagging.

## 3. Changelog draft — `0.18.0` (curated)

`release.yml` regenerates the full file mechanically at tag time
(`git-cliff --tag 0.18.0` sweeps every commit since `v0.17.0` — 979 of
them, 332 feat/fix — into the Keep-a-Changelog sections; `cliff.toml`
maps the prefixes). This draft is the curated headline of that sweep: the
merged v0.2 features a reader needs to see, with their real commit refs.
Entries below marked *(this diff)* are the uncommitted release-wave work —
they enter the regenerated changelog through the release PR's commits.

### Added

- **search**: the F09 engine — the six-field tantivy schema, build-in-RAM
  + read-only mmap open, the boost/fuzzy/filter/`<mark>` query contract,
  the L1 query cache with the injected clock (`4720f4cd`…`a1985470`), the
  `/_oxide/search` endpoint with the per-peer rate limit, and the
  hot-reload seam (`SwappableSearchIndex`, `0af1e0f0`…`af4c2c36`) plus the
  client static matcher (no WASM)
- **adapters**: the F11 delta-deploy engine — the blake3-filemap plan, the
  `dir:`/`s3:`/`r2:` `DeployStore` targets, the conditional-put pointer
  swap (`d7a9b96f`, `dc7bd5fa`) and the `oxide deploy` subcommand with the
  `<5%-of-bytes` acceptance (`2dffeeb1`, `tests/deploy_acceptance.sh`)
- **assets/cli**: the C09 image endpoint boots the ImageJobs engine —
  bounded CPU + AVIF lanes, per-output-key single-flight
  (`800652f3`; acceptance `tests/image_acceptance.sh`)
- **react/vue/svelte/solid**: the F01 island twins + the foreign pending
  placeholder (`3020947a`, `2dc58f13`, `80824ab6`, `efe6ce09`) and the
  per-framework vendor groups over rolldown `advancedChunks`
  (`2a726f5c`…`077eb39d`)
- **core**: typed search params — `parseSearch` over the Standard Schema
  contract with the `OX-SRC-001` refusal, the `hrefFor` runtime and the
  React typed `Link` (`9b1ea30e`, `9cd3978d`, `31fee0d8`)
- **core**: the E04 Trusted Types policy defined once per realm, the
  identity fallback, and every `innerHTML` sink routed through it across
  core + the four adapters + render (`af56da2b`…`0d796422`)
- **core/server/cli**: the F13 learned-prefetch substrate — the nav-graph,
  the nav endpoint with k-anonymity-only output, the client policy loop
  and the advertised bootstrap (`d4dfb29d`…`097af489`)
- **compiler/cli**: the boot-entry planner (legacy bytes byte-identical)
  and `.oxide/islands.json` beside the boot entry
  (`cefef24a`, `ff2bc258`)
- **core**: the E01 solid-refresh shim + the in-place refresh registry
  (`511a4a2d`, `067d6c1b`)
- **conformance**: one assertion per newly wired feature — 29 × 4 adapters
  (`e9114b31`; *this diff* verifies them green from a clean checkout)
- **ci**: the deploy-s3 feature job and the image/deploy acceptance steps
  (`95796309`; *this diff* adds the criterion microbench gate)

### Fixed

- **server**: an empty image signing secret disables signing instead of
  failing the endpoint (`babf649f` — the `OXIDE_IMAGE_UNSIGNED` operator
  disable, pinned `91e538dd`)
- **cli**: vendor groups resolve through the one islands site
  (`d30153d5`); prerender wrap checks use the raw context-table id
  (`f5e8351b`)
- **bundler**: the README doctest carries `vendor_groups` (`6d5ed95c`)
- **core**: *(this diff)* `boot.ts` imports `MixedIslandRegistry` from
  `islands` (the wrong-module import broke every fresh `@oxide-fw/core`
  build) and the `tsdown.config.mjs` `search` entry regains its comma

### Performance

- **server**: the frozen metrics map observes without the fallback lock
  (`91ebc24d`)
- **dispatch**: the render stage reuses the carried per-request nonce
  (`a951e3c4`); `action_params` builds only for eligible POST actions
  (`0908b539`)

### Refactored

- **cli/dispatch/compiler**: the one-definition pass — duplicate URL
  helpers deleted, the action stage parses via `oxide_fw_dispatch::url`,
  the boot entry delegates to the planner with the legacy body deleted
  (`e06d542a`, `bfe9dc24`, `2bbf7763`)

### Documentation

- the test badge re-measured (2,531 Rust / 754-757 TS), the F11 deferral
  closed, the C09 architecture row, the deviations re-point
  (`379e138b`, `b1da9888`, `f071d078`)
- *(this diff)* the R01/R03/R05/R06 status rows, the G2 target table in
  `benchmarks/report.md`, `docs/security-review-v0.2.md`,
  `docs/release-v0.2-prep.md`

## 4. Release checklist (spec V2.2.4) — status at prep time

| # | V2.2.4 item | status |
| --- | --- | --- |
| 1 | Waves A and B complete; numbers published | A/B rows merged in Appendix V2.O; `benchmarks/report.md` carries the table + the G2 status (R03) |
| 2 | Every F merged or spiked with a decision | see the F rows; the deferred halves are recorded in `docs/deviations.md` (A08/A09, F08/F17, E05/E06/E08–E11 are the announced v0.3 queue) |
| 3 | Conformance green on the four adapters | 29 × 4 green from a clean checkout (R01) |
| 4 | Capability matrix rows + refusal rule | R02 merged |
| 5 | `cargo test --workspace`, `pnpm test:packages`, conformance, V8 fixture check, benchmark gate green | CI-executed (the `rust`, `bench-gate`, `jshost-e2e`, `conformance` jobs); tier-1 results in the release PR body |
| 6 | README / architecture / spec reflect reality | re-verified in this wave's closeout (R04) |
| 7 | Security review of the new endpoints | `docs/security-review-v0.2.md` drafted for sign-off (R05) |

Tag sequence once the release PR merges: bump versions (Section 2) →
`git tag v0.18.0 && git push origin v0.18.0` → `release.yml` +
`release-binaries.yml` run → verify the release page + binaries →
`gh workflow run publish-npm -f version=0.7.0` (dry-run first) → announce
the capability table.
