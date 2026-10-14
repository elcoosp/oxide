// Oxide client boot (§10.3). Loaded as an ESM module on any page whose
// shell carried `<script type="module" src="/client/boot.js">`. Scans the
// live DOM for `<ox-island>` hosts, decodes their props, and dispatches
// each into a registered framework adapter's `hydrateIsland`.
//
// Framework-agnostic by design: the app registers its adapter once via
// `registerAdapter(adapter)`, and this module does the scanning + chunk
// loading + strategy scheduling. That keeps `packages/oxide` free of any
// React/Vue/Svelte/Solid import.

import { hydrateIslands, type IslandsOptions } from './islands.ts'
import type { FrameworkAdapter } from '../types.ts'

interface BootState {
  adapter?: FrameworkAdapter
  // Adapter's own module loader — resolves a chunk id from the client
  // manifest to the module namespace. This is framework-specific
  // (React's `lazy`, Vue's `defineAsyncComponent`, etc.) and is supplied
  // by the registered adapter.
  loadChunk: (chunk: string) => Promise<unknown>
}

declare global {
  interface Window {
    __oxideBoot?: BootState
    /** App code calls this once before boot scans the DOM. */
    __oxideRegisterAdapter?: (adapter: FrameworkAdapter, loadChunk: BootState['loadChunk']) => void
  }
}

window.__oxideBoot = {
  loadChunk: async (chunk) => {
    // Fallback: dynamic import the chunk as a module URL. Real apps
    // override via __oxideRegisterAdapter to route through their client
    // manifest.
    return await import(/* @vite-ignore */ chunk)
  },
}

window.__oxideRegisterAdapter = (adapter, loadChunk) => {
  if (!window.__oxideBoot) return
  window.__oxideBoot.adapter = adapter
  window.__oxideBoot.loadChunk = loadChunk
}

function run() {
  const state = window.__oxideBoot
  if (!state?.adapter) return
  const opts: IslandsOptions = {
    doc: document,
    adapter: state.adapter,
    loadChunk: state.loadChunk,
  }
  hydrateIslands(opts)
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', run, { once: true })
} else {
  run()
}

export {}
