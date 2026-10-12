# Oxide Implementation Worklog

Shared log for all agents. Read before working. Append after finishing a Task ID.

Project: Oxide meta-framework per `upload/Oxide Technical Specification v0.1.md` (spec §refs below use its numbering).
Repo root: `/home/z/my-project/oxide/` (git). Deliverable: zip in `/home/z/my-project/download/`.

---
Task ID: 1
Agent: main (Super Z)
Task: Wave 0 — workspace skeleton, pinned dependencies, conventions, crate stubs with cross-crate API contracts.

Work Log:
- Read full spec (781 lines). Extracted crate list (§23), decisions D1–D20, adapter contract (§7), ODF (§9.5), router (§8), cache (§11), server pipeline (§12), dev/HMR (§13), config (§14), plugins (§15), platform (§16), security (§17), deploy (§18), CLI (§19), errors (§19.4), conformance (§22).
- Installed Rust 1.99.0 (rustup, minimal profile). Verified crates.io + npm registry reachable. Node v24.21.0 available for NodeHost + TS tests.
- Created workspace skeleton: 16 spec crates + diagnostics crate, all compiling with stub APIs.
- Pinned all shared dependency versions in root [workspace.dependencies].

Stage Summary:
- Skeleton green: `cargo build --workspace` passes.
- Agents: work in your assigned crates ONLY. Never edit other crates' sources. Use `cargo test -p <pkg>`; a shared target dir means builds may wait on the lock (use timeout ≥ 300s). Read `/home/z/my-project/oxide/CONVENTIONS.md` first. TDD: write failing tests, then implement, then green.

---
Task ID: 2
Agent: odf (initial attempt died at context deadline; finished by odf2)
Task: crates odf + diagnostics — ODF wire format (§9.5) with golden fixtures.

Work Log (odf2, resumed):
- Fixed Result alias import collision; shared-container dedup slot bug (Ref(1) now emitted for repeated structures); ODF-MAGIC vs ODF-TRUNC ordering; refs depth guard at leaf level; integral f64 → JSON integer mapping.
- 16 golden fixture pairs written to crates/odf/tests/fixtures (+ examples/odf_fixture.rs regenerator, byte-stability test).

Stage Summary:
- 24 tests green; zero todo!; committed 8960a8b. Format doc maintained in decoder header + conformance/fixtures/README.md.

---
Task ID: 3
Agent: router (died at deadline; finished by router2)
Task: crates router — radix matcher, rkyv archive, typegen, rules engine (§8, D8, D13).

Work Log (router2):
- Fixed ferr helpers, ArchivedTree Debug, route-id canonicalization (`routes/` prefix), layout_chain derivation from route ids, mmap liveness test, shadow-DTO rkyv archive.
- 55 tests green (incl. 2 proptests); committed d9e7bc5.

Stage Summary:
- Scanner/matcher/typegen/rules all green; 6 documented minimal test adjustments (contract contradictions in pre-written tests).

---
Task ID: 4
Agent: graphcfg (died at deadline; finished by graphcfg2)
Task: crates graph + config — demand engine w/ early cutoff, CAS; layered typed config (§5, D6, §14, D14).

Work Log (graphcfg2):
- Epoch/early-cutoff semantics settled (seeds always reported; hash-stable dependents survive; dep-free nodes recompute after bump). Tool-version tracking added.
- Config: 6-source layering, OXIDE_ allow-list, Levenshtein typo suggestions, §14.1 example extraction test, JSON fallback flag.

Stage Summary:
- graph 35 tests, config 29 tests, all green; committed acd2327.

---
Task ID: 5
Agent: cache
Task: crate cache — L0/L1/L2, tags, SWR, single-flight (§11).

Work Log:
- In-house byte-weighted LRU (moka swap documented), redb disk tier w/ hash-verified blobs, single-flight publish-under-shard-lock design, soft/hard tag invalidation, conditional-request helpers.

Stage Summary:
- 44 tests green; committed 7e9e5dd; deviations logged in docs/deviations.md.

---
Task ID: 6
Agent: compbundle (died at deadline; compiler finished, bundler finished by bundler2)
Task: crates compiler + bundler — passes, guard, ESM bundler, manifests (§6.2–6.4, §9.3–9.4).

Work Log:
- compiler: lexer + 29 tests green (import scan, use_cache, server-fn stubs, guard, HMR inject).
- bundler (bundler2): resolve reuse + normalize() fix, shared-chunk grouping, export_names fix, real import rewriting, manifests roundtrip; 21 tests green.

Stage Summary:
- compiler 32 + bundler 21 tests green; commits cc67005 (+ compiler files synced at 2ae8772).

---
Task ID: 7
Agent: jshost
Task: crates jshost + plugin — JsHost trait, NodeHost child workers, ops, hooks (§4, §15).

Work Log:
- frame/unframe, choose_worker, ScriptedHost/ScriptedPool, NodeHost with 77-line pure-Node worker (render/compile/boom/die/sleep/chunked), V8Host fail-fast stub, plugin registry/chain/WASM validation.

Stage Summary:
- jshost 18 + plugin 11 tests green (7 real Node-process tests); committed e7a64de.

---
Task ID: 8
Agent: rendserv (died at deadline; server finished by servfix)
Task: crates render + server — streaming pipeline, boundary protocol, hyper pipeline, security (§7.1, §10, §12, §17).

Work Log:
- render: head merge, swapper/boundary protocol, islands, hydration bootstrap, lol_html inject (static + streaming byte-equal), 39 tests green.
- server (servfix): Send-future fix (sync native_prefix + BoxFuture crossing await), 48-test suite green incl. hyper integration, ws codec, security tables.

Stage Summary:
- render 39 + server 49 tests green; committed 6f422b9.

---
Task ID: 9
Agent: assetsdev
Task: crates assets + dev — CSS/image/font/content pipelines, watcher, HMR (§6.6–6.7, §16, §13, §5.5).

Work Log:
- CSS parser/nesting/modules/minify/prefixes; image resize/encode/LQIP; font bundles; metadata/sitemap/robots/markdown/highlight; hash-confirmed watcher (touch-without-change = no events); HMR messages + hub + behavior table.

Stage Summary:
- assets 49 + dev 12 tests green; committed 4640c5c.

---
Task ID: 10
Agent: cliadapt
Task: crates adapters + cli + core-wasm — deploy targets, capability matrix, CLI surface, edge subset (§18, §19.1, D18).

Work Log:
- Capability matrix + refusal rule + 10-target emit; all §19.1 commands incl. run_with_writer; core-wasm edge helpers.

Stage Summary:
- adapters 20 + cli 25 + core-wasm 12 tests green; committed e287710.

---
Task ID: 11
Agent: tsruntime
Task: packages/oxide — server APIs, ODF JS decoder, client router, islands, enhance, test helper.

Work Log:
- ODF decoder reverse-engineered from Rust fixtures (16/16 byte-exact, refs resolve as live references); router/enhance/islands tested against hand-rolled fake DOM.

Stage Summary:
- 66 tests green across 7 files; committed b71a1ff.

---
Task ID: 12
Agent: adapters (died at deadline; create-oxide finished by main)
Task: framework adapters react/vue/svelte/solid, conformance suite, create-oxide (§7, §22, §19.2).

Work Log:
- Four adapters (IR-based SSR, streaming boundary protocol, hydration, capabilities per §7) — 14 tests each.
- Conformance suite: assertion set × 4 adapters green (§22 list). create-oxide finished by main (integrator): programmatic scaffolding, 4 frameworks × 4 shapes, ENOEMPTY refusal, 5 tests green.

Stage Summary:
- TS: 14×4 + 4 conformance + 5 = 65 tests green; commits 264e346, 2de90c4.

---
Task ID: 13
Agent: main (Super Z)
Task: Integration, verification, smoke run, docs.

Work Log:
- Full `cargo test --workspace`: 477 passed / 0 failed (re-verified post-clean with debug=0).
- TS total: 131 tests (66 + 4×14 + 5 + 4), all green.
- Release build of oxide-cli (3m19s); smoke run: doctor, routes scan (root-page `/index`→`/` mapping fixed in cli dispatch + retested), build (2 routes, deploy artifacts), start (GET / 200, /blog/abc 200, /_oxide/health ok, 404s correct).
- Disk-full incident handled (cleaned 8.5GB debug artifacts).
- Docs: README.md, docs/architecture.md (§→crate map), docs/deviations.md cross-crate summary, benchmarks/README.md, examples/smoke-app, CI workflow (3-OS matrix), MIT/Apache dual license.

Stage Summary:
- Everything green; repo at d4de01d (15 commits).

---
Task ID: 14
Agent: main (Super Z)
Task: Zip deliverable.

Work Log:
- `git archive` of HEAD + WORKLOG.md → /home/z/my-project/download/oxide.zip (585 KB, 303 files).

Stage Summary:
- Deliverable ready.

---

## Session — §6.3 / §7 framework adapter read path CLOSED

**Milestone:** the read-path gap that blocked "a team can ship a production
site using only Oxide" is now closed for all four frameworks.

What landed this session:

- `oxide-jshost::FrameworkHost` — Rust-side loader + render dispatcher for
  a user's `oxide.config.{ts,mjs,js}` (spec §6.3, §7). Talks to the Node
  worker via two new ops: `framework:load` (descriptor extraction) and
  `framework:render` (dispatch to `FrameworkAdapter.render` + drain stream).
- Worker `oxide-worker.mjs` extended with the two ops; dynamic `import()`s
  the config, evaluates it, calls into the exported adapter.
- Adapter packages all four migrate to latest upstreams:
  - React `19.3.0` (stable)
  - Vue `3.6.0-rc.11` (Vapor Mode + reactivity rewrite)
  - Svelte `5.57.2`
  - Solid `2.0.0-rc.14` + `@solidjs/web` (async-native reactivity; the
    web renderer moved out of `solid-js/web` into a dedicated package)
- `createRealAdapter({ ssr, componentFor, layoutFor })` exposed from every
  `@oxide/{react,vue,svelte,solid}` package. Callable default export
  (`framework: react({ ... })` in config) returns the IR adapter by
  default and dispatches to the real renderer when `componentFor` is
  supplied.
- CLI subcommands:
  - `oxide config [--json] [--root <path>]` — prints the adapter descriptor
  - `oxide render <route> [--root] [--url] [--param k=v] [--data <json>]`
    — dispatches a page render through the adapter, writes HTML to stdout
- E2E tests prove the full chain:
  - `crates/jshost/tests/framework_host_e2e.rs` — real config → real
    `react-dom/server` → HTML, through the Rust worker
  - `crates/cli/src/tests.rs::cli_render_dispatches_real_react_ssr` —
    same, but through the CLI's `oxide render` subcommand

Both E2E tests are `#[ignore]`d by default (they spawn Node). Run with
`cargo test -- --ignored`.

**What remains for full pipeline integration:**

- `cmd_dev` / `cmd_build` / `cmd_start` still use the neutral-IR renderer.
  Wiring them to `FrameworkHost` is a plumbing change, not architecture:
  the loader + dispatcher + adapter contract are all proven end-to-end.
- Content-hashed build output, streaming through the pool (currently the
  worker collects the whole stream into one HTML string), and per-request
  caching in the dev server are follow-ups.
