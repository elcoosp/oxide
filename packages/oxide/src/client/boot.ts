// Oxide client boot (§10.3). The generated per-project boot entry
// (`.oxide/island-boot.mjs`, written by `oxide build`) imports this module,
// registers the framework adapter, and then calls `boot()`.
//
// This module MUST be import-safe in the SSR / config-loader environment:
// `@oxide/react` re-exports it, and `oxide.config.ts` imports `@oxide/react`
// for its adapter factory — so this file gets evaluated in the Node worker
// where `window` and `document` do not exist. Everything touching the DOM
// is behind a runtime guard; the module has no top-level side effects.

import { hydrateIslands, type IslandsOptions } from './islands.ts'
import type { FrameworkAdapter } from '../types.ts'

interface BootState {
  adapter?: FrameworkAdapter
  loadChunk: (chunk: string) => Promise<unknown>
}

interface BootWindow extends Window {
  __oxideBoot?: BootState
  __oxideRegisterAdapter?: (adapter: FrameworkAdapter, loadChunk: BootState['loadChunk']) => void
  __oxideBootRun?: () => void
}

/** A `Window` shaped object, or `undefined` in a non-browser environment. */
function maybeWindow(): BootWindow | undefined {
  const g = globalThis as { window?: BootWindow }
  return g.window
}

/**
 * Install the boot runtime on `window` if it isn't already there. Idempotent
 * across duplicate module evaluations (bundler dedup failure, SPA reimport).
 * No-op outside the browser.
 */
function ensureInstalled(w: BootWindow): void {
  if (w.__oxideRegisterAdapter) return
  w.__oxideBoot = {
    loadChunk: async (chunk) => {
      return await import(/* @vite-ignore */ chunk)
    },
  }
  w.__oxideRegisterAdapter = (adapter, loadChunk) => {
    const state = w.__oxideBoot
    if (!state) return
    state.adapter = adapter
    state.loadChunk = loadChunk
  }
  w.__oxideBootRun = () => {
    const state = w.__oxideBoot
    if (!state?.adapter) return
    const doc = (globalThis as { document?: Document }).document
    if (!doc) return
    const opts: IslandsOptions = {
      doc: doc as unknown as IslandsOptions['doc'],
      adapter: state.adapter,
      loadChunk: state.loadChunk,
    }
    hydrateIslands(opts)
  }
}

/**
 * Register an adapter and its chunk loader, then run the island scan. The
 * generated boot entry calls this after its static imports have resolved.
 * Safe to call multiple times: the scan is idempotent because each
 * `<ox-island>` host carries a `data-ox-hydrated` guard after its first
 * hydration attempt.
 *
 * Runs synchronously if the document is already interactive; otherwise
 * waits for `DOMContentLoaded`.
 */
export function boot(): void {
  const w = maybeWindow()
  if (!w) return
  ensureInstalled(w)
  const run = w.__oxideBootRun
  if (typeof run !== 'function') return
  const doc = (globalThis as { document?: Document }).document
  if (!doc) return
  if (doc.readyState === 'loading') {
    doc.addEventListener('DOMContentLoaded', run, { once: true })
  } else {
    run()
  }
}

/**
 * Install the boot runtime on `window` without running the island scan.
 * Called by the generated entry before `boot()` so `__oxideRegisterAdapter`
 * is available when the entry imports the framework adapter. No-op outside
 * the browser — safe to call from any module.
 */
export function installBootRuntime(): void {
  const w = maybeWindow()
  if (!w) return
  ensureInstalled(w)
}

// Install eagerly when the browser context is present, so a hand-written
// entry that imports `boot` and calls `window.__oxideRegisterAdapter`
// directly still works. The SSR/config-loader environment does not reach
// this branch.
{
  const w = maybeWindow()
  if (w) ensureInstalled(w)
}

export {}
