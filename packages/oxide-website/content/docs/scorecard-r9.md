---
title: Compliance scorecard — r9
description: The current spec-compliance scorecard for Oxide v0.1. Every section, every finding, every verdict — kept in the repository so the audit posts can cite it directly.
order: 100
---

# Oxide v0.1 spec-compliance scorecard — r9

**Range audited:** `fbfc794..663201a` (43 commits).
**Overall:** ~93 / 100 (r8 ~92 → Δ +1).

Each round runs the same method: 4 parallel read-only verification
agents; every commit message and doc treated as a claim and traced to a
call chain with `file:line` evidence; TypeScript suites and conformance
executed for real; Rust tests counted statically and anchored (1,153
test functions at r9; `cargo nextest --workspace` reports 1,109
executed).

## The master P0 table

| # | Item | r9 verdict |
|---|---|---|
| P0-1 | core-wasm never emits its wasm | **PARTIAL** — `edge_bundle.js` still unemitted; `mergeHead` mismatch stands |
| P0-2 | Worker defer path buffers | **PARTIAL** — sequential streaming live; `/__data` unbounded `collect` stands |
| P0-3 | Parallel layout loads (§9.2 MUST) | **CLOSED** |
| P0-4 | PPR orphaned | **PARTIAL** — runtime shell cache wired and cookie-guarded; no build-time shell, no rebuild invalidation |
| P0-5 | Session write path | **CLOSED** (r7) + **D1 extension PARTIAL** — six serve paths persist; short-circuit-on-throw and `/__data` precedence residues remain |
| P0-6 | h3 body + streaming | **CLOSED** (r9 — `h3Advertise` schema'd) |
| P0-7 | Middleware single-layer | **CLOSED** (r5) |
| P0-8 | HttpCacheHandler unsealed L2 | **CLOSED** (r9 — sealed, tampered-entry tested) |
| P0-9 | Single-flight poisoned key | **CLOSED** (r7) |
| P0-10 | Routes authoring model violates §14/§8.1 | **PARTIAL** — resolver model live end-to-end; `oxide render` attaches no ComponentSource, `adapter_entry` unread, the `ssr:'real'` default undecided |

## Closed this round

**The r8 fix list, all 11 items:** the red TS suite; the scaffolder's
config; PPR's cookie-leak guard; the website config migration (partial —
see r9's new findings); the schema drift; the three CI-theater stubs;
the bench report's p99 framing (half); P0-8; G2's trace threading;
the docs/nits sweep.

**Three new features, tested:**
- **K1** content queries runtime (`getCollection` / `getEntry`; the
  build emits `<collection>.entries.json`; loaders read from
  `OXIDE_ROOT/dist/content/`).
- **K2** route search (build-time extractor over emitted HTML + a
  byte-stable 122-line client scorer).
- **K3** PWA (build-generated SW with build-id-scoped cache,
  stale-while-revalidate, network-first navigations, configurable
  `sw.offlineFallback`).

## Open findings, honestly

1. **The 2026-10-19 HANDOFF is superseded.** This scorecard and the
   README carry the current state; the HANDOFF file is 300+ commits
   stale and should be treated as a historical record.
2. **P0-1 and P0-2 remain open** despite the r8 audit listing them as
   "fixed in the same push" — that claim was wrong and is retracted in
   this round. Neither is touched by the r8 push.
3. **PPR is a runtime shell cache**, not the §10.4 build-time feature.
   Don't claim it in marketing.
4. **WP-D1's short-circuit case is structurally open:** the worker omits
   `session` on thrown redirects/errors, so the four SSR short-circuit
   arms cannot persist even when the loader mutated the session.
5. **`oxide render` attaches no ComponentSource** — a real-SSR config
   through `oxide render` 500s. `oxide build`/`start`/`dev` are fine.
6. **K2/K3 have real hazards:** the search index is served `immutable`
   despite being unhashed; `sw.offlineFallback` is not precached, so
   the knob is dead in the common case.

## Watchlist for r10

- Is CI green at HEAD? (r9's WP-1 closed the red test.)
- K1's first real consumer — until a shipped loader calls
  `getCollection`, "content queries" stays on the do-not-claim list.
- The `ssr:'real'` factory default — decide or document.
- Defer streaming still has zero browser e2e; the Playwright CI job
  makes adding the spec cheap now.
- HANDOFF refreshed or formally retired.

*The full r9 audit's findings section is summarised here; the source
of truth for any specific claim is the commit or file cited in the
audit's finding list, not this table.*
