# Handoff — plan complete: 33/33 items ticked

## Where things stand

**Branch:** `website`
**HEAD:** `d40a981e feat(refresh): re-enable §1.2 $RefreshSig$ emit — remount on hook-shape change`
**Working tree:** clean.
**Plan status:** all 33 items ticked (`docs/plan-esm-hmr.md`). §1.2 and §1.3 — the two items the previous handoff left open — are closed and verified.

## What landed this session (three commits after `be2b6fd3`)

1. **`8359a20b` — §1.3 transitive fan-out + boot-storm stability.** The §1.3 fan-out code (reverse-dep publish for non-component edits, scoped to `.tsx`/`.jsx` importers) was verified end-to-end: a leaf util edit publishes ONE `BatchModuleUpdate` carrying `[util.ts, hmr-target.tsx]`, the DOM updates in place, state is preserved, no reload. Landing it on a fresh clone exposed a boot-storm chain that made every spec flaky; five fixes went in with it (watcher boot baseline, node_modules event filtering, stale-generation gating, workspace-rebuild PATH fix, client refresh-runtime race + foreign-route data guard).
2. **`d40a981e` — §1.2 $RefreshSig$ emit re-enabled.** The 2026-11-04 ECONNRESET disable was a misattribution — the real culprit was the boot-storm chain, not the emit. The emit inserts `_s(Comp, hook_key);` after each component's DECLARING statement (statement end, not body end, so arrow components stay valid). Acceptance: `hmr-refresh-sig.spec.ts` un-skipped and green — adding a hook REMOUNTS (counter resets), no reload.
3. **Watch-budget rework** (in `d40a981e`): the recursive inotify watch spent one watch per node_modules directory (~10k in a pnpm store) and nondeterministically exhausted `fs.inotify.max_user_watches` — the watcher silently stopped firing for whichever subtrees lost the registration race. Now: pruned per-directory watches, self-healing watches for post-boot directories (closes the mkdir→write race §3.3's test exercises), and a boot baseline seeding node_modules `package.json` entries.

## Verified test state

Gate: `--workers=1 --retries=0` (unchanged).

- **chromium** — full HMR group (`hmr`, `hmr-transitive`, `hmr-refresh-sig`, `hmr-cold-start`, `hmr-reconnect`, `hmr-prebundle`, `hmr-bundler`): **31/31**. Group B (`smoke`, `island`, `hmr-server-fn`, `hmr-cssmodules`, `hmr-env`, `hmr-worker`, `hmr-perf`): 12/13. Group C (`deferred`, `loading`, `middleware`, `no-js`, `og`, `socket`, `sse`, `dev-rescan`): 17/17.
- **firefox** — core HMR specs (`hmr`, `hmr-transitive`, `hmr-refresh-sig`): **26/26**.
- **Unit tests** — compiler 48/48, dev 56/56 + 46/46 + 1/1.
- **Not runnable in this environment** (pre-existing, not regressions):
  - **webkit**: the host is missing GTK4/GStreamer libraries and there is no root to install them. Runs fine on a normal dev machine.
  - **hmr-isolation (§3.6)**: `solid-jsx-oxc` ships only a darwin-arm64 native binary (the prior session published it from macOS), so `smoke-app-solid` cannot SSR on linux-x64. Fixing it needs the crate source published for linux (outside this repo).
  - **sw.spec (×2)**: fails identically on the pre-session build in this sandbox (service-worker precache empty in headless chromium here); not touched by this session's changes.

## Diagnostics added

- `OXIDE_DEV_LOG_FILE=/path/dev-%p.log` — `oxide dev` tees all tracing output AND Rust panics (with backtrace) into a per-process log file. This is how a dev-server death can be attributed even when nothing drains the child's stdio pipes. Zero panics were captured across the final full verification runs.

## Known gotchas that still apply

1. `test.setTimeout(60_000)` inside `beforeAll` for any spec that boots `oxide dev` (hook budget ≠ test budget).
2. Don't delete `tests/playwright/test-results` mid-run.
3. `HM_PRISTINE` / `HMR_PRISTINE` constants are hard-coded fixture copies — sync them in the same commit as any `hmr-target.tsx` change (`hmr.spec.ts`, `hmr-cold-start.spec.ts`, `hmr-reconnect.spec.ts`).
4. `pkill -f` patterns match the calling shell — kill by PID.
5. `packages/core/dist/` is gitignored but force-tracked; run `pnpm -C packages/core build` after any `hot.ts`/`hmr.ts` change and commit the rebuilt dist alongside.
6. The staleness scan in `ensure_dev_client_bundle` scans `src/`, `packages/*/dist/`, and `node_modules/@oxide-fw/*/dist/` — do not narrow it.
7. The Playwright run gate is serial (`--workers=1 --retries=0`); everything downstream assumes it.
8. This sandbox reaps background processes between tool calls and periodically deletes `node_modules` under the project root — if working here again, re-run `pnpm install` (sub-second from the store) before any node invocation, and expect `target/` to persist.

## Where to pick up, in one sentence

The plan is complete — next work is post-plan hardening (e.g. §3.8b's browser ESM-cache proxy, deferred with rationale in the plan), publishing `solid-jsx-oxc` linux binaries so §3.6's isolation test can run off-macOS, and keeping the suite green in CI.
