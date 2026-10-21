---
title: We audited our own framework against its own spec — 92/100, everything public
description: Oxide is built against a published v0.1 specification, and audited against it every round. Here's the method, the score, and the findings we would rather not have published — but did.
date: 2026-11-02
tags: [oxide, rust, quality, transparency, engineering]
---

Most frameworks publish benchmarks. Very few publish their homework.

Oxide is built against a **published technical specification** — v0.1, 20 numbered decisions, 26
sections — and since the first implementation commit we have run a recurring, structured audit of
the codebase against that spec. Eight rounds so far. The current score is **~92 / 100** (r8 at
`fbfc794`, with the two P0 fixes landed since), every section and every finding is public, and this
post explains the method and — more interestingly —
the things the audit found that we would rather not have published, and published anyway.

## The method

Each audit round runs the same loop:

1. **Repull and diff.** The upstream repo is pulled and every commit since the last round is
   classified.
2. **Claims become verification targets.** Commit messages and handoff documents are treated as
   *claims*, not facts. Every claim is traced to a call chain with file-and-line evidence and
   gets a verdict: **LIVE** (production-reachable), **SEAM-ONLY** (tested in isolation, not
   wired into the real path), **REFUTED**, or **MIXED**.
3. **Tests are executed, not counted.** The TypeScript suites run for real — the last round:
   333/333 green, zero skipped on this machine (the last r8 run reported 351/362 — the 11
   red tests were fixed post-audit). The Rust suites are statically counted (test functions, ignored
   gates, TDD-written contract suites) until a full toolchain run is available.
4. **21 sections, scored.** The spec's sections map to 21 scored areas; the score is the plain
   average, no weighting games.

The last round's verdict distribution is the interesting part: the two longest-standing fixes —
defer streaming on the serve path and strict MAC sealing for the remote cache — verified **LIVE**
with file-and-line evidence, while a handful of items came back SEAM-ONLY and got *demoted* in
public.

## The score, roughly

The 21 sections cover the build graph, bundler, config extraction, router, server pipeline,
rendering paths, cache semantics, deploy targets, plugin surface, diagnostics, testing
infrastructure, and documentation integrity. Seven rounds moved the score from the high 80s to
**92.4 ≈ 92**: 18 sections up, three flat (config-schema drift and the migration wave uncovered
new surface in §6 and §14 — both fixed post-audit),
and documentation integrity took the biggest single hit in the last round — for reasons we think
are worth explaining in full.

## What the audit found on us

This is the part that makes the exercise worth publishing:

- **A 44-commit batch landed in a single timestamp**, and about ten of those commits carried
  behavior-titled messages while containing formatting-only changes. The commit log stopped
  being a changelog, and the audit caught it mis-dating an entire security wave. We now treat
  commit messages as unaudited claims — and we stopped writing those batches.
- **HANDOFF.md rotted** — stale by ~190 commits, with an internal test-count inconsistency. The
  file the project points new contributors at was lying to them, and only a structured audit
  made that measurable.
- **The scaffolder emits a spec-conformant shape that silently degrades.** The spec says config
  carries a routes *path* with build-time component collection; the scaffolder obeys — and then
  real SSR requires today's config-side imports, so a spec-faithful scaffold quietly renders IR
  placeholders instead of your components. The deviation is real, it is now documented as a
  numbered finding, and it is top of the fix list.
- **Standing gaps stay visible.** Partial prerendering has tested components and zero production
  callers; the `http` cache handler serves unsealed payloads while Redis/S3 seal by default;
  i18n resolves the locale but never emits `<html lang>`. None of these are hidden in a tracker
  nobody reads — they are in the scorecard, every round, until they close.

## Why publish this

Because trust in a pre-alpha framework is not earned by the features list — it is earned by how
the project behaves when things are broken. A public scorecard does three things a changelog
cannot:

1. **It makes regressions expensive.** A section that drops next round is a public event with a
   reason attached.
2. **It converts "trust us" into "check us".** Every claim in the audit has file-and-line
   evidence; you can spend an afternoon verifying us instead of believing us.
3. **It keeps marketing honest.** The rule this project operates under is that only LIVE,
   production-reachable features get marketed — the SEAM-ONLY list is documentation, not copy.
   The audit is what enforces that rule mechanically.

## Next round

The scorecard is a living document: next round re-verifies the componentFor fix (or the
documented deviation, if we choose honesty-by-documentation for v0.1), re-counts the Rust suites,
and adds the sections the last round flagged for widening drift. If you want the audit to have
more teeth, the best contribution is a machine-checkable assertion for a section you care about
— [the repo](https://github.com/elcoosp/oxide) and its spec are public, and the
[docs](/docs/architecture) map every spec section to the crate that owns it.

---

*r8 → post-audit corrections (2026-10-21):* the 91/100 above is the r7
score; r8 re-scored at ~92 with the ComponentSource main path closed. The
audit's own findings are now (mostly) fixed: the TS suite is green, the
scaffolder emits the current config shape, PPR refuses to cache
cookie-setting responses, and the schema carries the two keys it was
missing. What remains open — P0-1's `edge_bundle.js`, P0-8's L2 sealing,
P0-2's `/__data` unbounded collect, and the half of the migration the
website config itself was on — is tracked in the r8 scorecard and fixed in
this same push. The post is a point-in-time record; the scorecard is the
live document.
