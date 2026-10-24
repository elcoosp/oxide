// §7.1: the deferred-boundary machinery the core owns.
//
// - the render-scoped registry behind `<Defer>` (suspense: 'core' adapters)
// - the wire shapes the adapter streams (placeholder / resolution + swapper)
//
// The registry lives on `globalThis` rather than in a module-level binding
// for the same reason `__oxide_fn_registry` does: a Svelte component is
// *compiled*, and its emitted module sits wherever the build puts it — a
// relative import to this file would resolve from the output location, not
// the source location. A global slot has no path.

export interface BoundaryRegistration {
  /** The id the placeholder carried, matching `{ __deferred: id }`. */
  id: number
  /** A component that renders the resolved content standalone. */
  Component: unknown
  /** Props for that component (the resolved value lives here). */
  props: Record<string, unknown>
}

type Slot = BoundaryRegistration[]

// Register the slot on `globalThis` so it survives module duplication: a
// compiled Svelte component's output lives at a build path, so a relative
// import to this module would fail. The global slot has no path dependency.
// `declare global` gives `globalThis.__oxide_boundary_registry` full type
// safety — the module-level type imports are erased at compile time, so
// there is no runtime cost, and no cast is needed.
declare global {
  // eslint-disable-next-line no-var
  var __oxide_boundary_registry: BoundaryRegistration[] | undefined
}
const SLOT = '__oxide_boundary_registry'

function slot(): Slot | undefined {
  return globalThis.__oxide_boundary_registry
}

/**
 * Start capturing boundaries. Returns the previous slot so a nested render
 * (an island, an error boundary) restores it instead of clobbering the
 * outer capture.
 *
 * Safe as a process-global because the worker serialises calls into one
 * module instance — the same assumption `__oxide_fn_registry` makes.
 */
export function beginBoundaryCapture(): Slot | undefined {
  const prev = slot()
  globalThis.__oxide_boundary_registry = []
  return prev
}

/** Restore the slot captured by `beginBoundaryCapture`. */
export function endBoundaryCapture(prev: Slot | undefined): void {
  if (prev === undefined) {
    delete globalThis.__oxide_boundary_registry
  } else {
    globalThis.__oxide_boundary_registry = prev
  }
}

/** Take everything captured so far. Leaves the slot empty for the next batch. */
export function drainBoundaries(): BoundaryRegistration[] {
  const cur = slot()
  if (!cur) return []
  return cur.splice(0, cur.length)
}

/** Called by `<Defer>` while the shell is rendering. A no-op outside a
 *  capture: a component rendered standalone still emits its placeholder. */
export function registerBoundary(entry: BoundaryRegistration): void {
  const cur = slot()
  if (!cur) return
  if (cur.some((e) => e.id === entry.id)) return
  cur.push(entry)
}

/** The `{ __deferred: id }` placeholder a loader's data carries, or null. */
export function deferredIdOf(value: unknown): number | null {
  if (value === null || typeof value !== 'object') return null
  const v = value as { __deferred?: unknown }
  if (typeof v.__deferred !== 'number') return null
  if (Object.keys(value as object).length !== 1) return null
  return v.__deferred
}

// ---------------------------------------------------------------------------
// Wire shapes (§7.1) — byte-identical to `render::boundary_fallback` /
// `render::boundary_content` and to the four adapter packages' `protocol.ts`.
// ---------------------------------------------------------------------------

export const BOUNDARY_FALLBACK_ATTR = 'data-ox-b'
export const BOUNDARY_CONTENT_ATTR = 'data-ox-c'

const TEXT_ESCAPES: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;' }
const ATTR_ESCAPES: Record<string, string> = {
  '&': '&amp;', '"': '&quot;', '<': '&lt;', '>': '&gt;', "'": '&#39;',
}

function escapeText(s: string): string {
  return s.replace(/[&<>]/g, (c) => TEXT_ESCAPES[c] as string)
}
function escapeAttr(s: string): string {
  return s.replace(/[&"'<>]/g, (c) => ATTR_ESCAPES[c] as string)
}
function escapeScript(s: string): string {
  return s.replace(/<\/(script)/gi, '<\\/$1').replace(/<!--/g, '<\\!--')
}

/** Attribute-only selectors so one swapper drives both host shapes: the IR
 *  renderer's `<template>` and a framework's `<ox-defer>`. */
export const SWAPPER_SRC =
  `function(id){var t=document.querySelector("[data-ox-c='"+id+"']"),` +
  `p=document.querySelector("[data-ox-b='"+id+"']");` +
  `if(t&&p){var d=document.createElement("div");d.innerHTML=t.innerHTML;` +
  `p.replaceWith.apply(p,d.childNodes);` +
  `document.dispatchEvent(new CustomEvent("ox:swap",{detail:{id:id,value:t.innerHTML}}))}}`

export function boundaryFallback(id: number, fallbackHtml: string): string {
  return `<template ${BOUNDARY_FALLBACK_ATTR}='${id}'>${fallbackHtml}</template>`
}

export function boundaryContent(id: number, html: string, nonce = ''): string {
  const n = nonce ? ` nonce='${escapeAttr(nonce)}'` : ''
  return (
    `<template ${BOUNDARY_CONTENT_ATTR}='${id}'>${html}</template>` +
    `<script${n}>(${escapeScript(SWAPPER_SRC)})('${id}')</script>`
  )
}

export { escapeText, escapeAttr }
