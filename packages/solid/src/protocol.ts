// Wire-shape helpers for the core deferred-boundary protocol (spec §7.1) and
// the hydration bootstrap (§9.5).
//
// REPLICATED from `crates/render/src/lib.rs` — render::boundary_fallback,
// render::boundary_content, render::SWAPPER_SRC, render::hydration_bootstrap,
// render::island_element and render::island_element's error boundary shape.
// The Rust render crate is the single source of truth for these byte shapes;
// the JS side must emit identical strings (each adapter package carries this
// small copy because packages/oxide has no cross-package import wiring).
//
// Protocol strings use SINGLE quotes exactly like the Rust crate. Adapter page
// markup elsewhere uses its own quoting — do not "normalize" these.

import { escapeAttr, escapeScript, escapeText } from './ir.ts'

/** Placeholder attribute: `<template data-ox-b='ID'>` + fallback (§7.1). */
export const BOUNDARY_FALLBACK_ATTR = 'data-ox-b'

/** Resolution attribute: `<template data-ox-c='ID'>` + swapper (§7.1). */
export const BOUNDARY_CONTENT_ATTR = 'data-ox-c'

/**
 * The swapper logic — byte-identical to `render::SWAPPER_SRC`. Finds
 * `template[data-ox-c='<id>']`, moves its content over the matching
 * `template[data-ox-b='<id>']` placeholder via replaceWith, then dispatches
 * the `ox:swap` CustomEvent (detail: {id}) that triggers adapter hydration.
 */
export const SWAPPER_SRC =
  `function(id){var t=document.querySelector("[data-ox-c='"+id+"']"),` +
  `p=document.querySelector("[data-ox-b='"+id+"']");` +
  `if(t&&p){var d=document.createElement("div");d.innerHTML=t.innerHTML;` +
  `p.replaceWith.apply(p,d.childNodes);` +
  `document.dispatchEvent(new CustomEvent("ox:swap",{detail:{id:id,value:t.innerHTML}}))}}`

/** Streamed boundary placeholder — render::boundary_fallback. */
export function boundaryFallback(id: number, fallbackHtml: string): string {
  return `<template ${BOUNDARY_FALLBACK_ATTR}='${id}'>${fallbackHtml}</template>`
}

/**
 * Streamed boundary resolution — render::boundary_content. The content
 * template plus a self-contained swapper IIFE with `id` baked in. `nonce`
 * follows the render crate: omitted when empty (Rust injects CSP nonces in
 * production, §10.2 step 4).
 */
export function boundaryContent(id: number, html: string, nonce = ''): string {
  const n = nonce ? ` nonce='${escapeAttr(nonce)}'` : ''
  return (
    `<template ${BOUNDARY_CONTENT_ATTR}='${id}'>${html}</template>` +
    `<script${n}>(${escapeScript(SWAPPER_SRC)})('${id}')</script>`
  )
}

/**
 * Islands host markup (§10.3) — render::island_element byte shape:
 * `<ox-island data-ox-island='C' data-ox-chunk='K' data-ox-strategy='S'
 * data-ox-props='P'>SSR</ox-island>`.
 */
export function islandElement(directive: {
  component: string
  chunk: string
  strategy: string
  propsB64: string
}, ssrHtml: string): string {
  return (
    `<ox-island data-ox-island='${escapeAttr(directive.component)}'` +
    ` data-ox-chunk='${escapeAttr(directive.chunk)}'` +
    ` data-ox-strategy='${escapeAttr(directive.strategy)}'` +
    ` data-ox-props='${escapeAttr(directive.propsB64)}'>${ssrHtml}</ox-island>`
  )
}

/** Error boundary markup — render crate streamed-error shape (lib.rs ~L692). */
export function errorBoundary(code: string, message: string): string {
  return `<div class='ox-error' data-ox-error='${escapeAttr(code)}'>${escapeAttr(code)}: ${escapeText(message)}</div>`
}

/**
 * Hydration bootstrap (§9.5) — render::hydration_bootstrap shape: the base64
 * ODF payload script plus the `window.__oxide` router bootstrap. Emitted when
 * RenderInput.odf carries serialized data; Rust injects CSP nonces later.
 */
export function hydrationBootstrap(odfB64: string, routeId: string): string {
  const route = sanitizeRouteId(routeId)
  return (
    `<script type='application/oxide-data'>${odfB64}</script>` +
    `<script>window.__oxide={route:"${route}",` +
    `data:document.currentScript.previousElementSibling.textContent}</script>`
  )
}

/** Percent-encode every byte outside the route-id alphabet `[a-z0-9-/]`. */
function sanitizeRouteId(routeId: string): string {
  let out = ''
  for (const byte of new TextEncoder().encode(routeId)) {
    const c = String.fromCharCode(byte)
    out += /^[a-z0-9\-/]$/.test(c) ? c : `%${byte.toString(16).toUpperCase().padStart(2, '0')}`
  }
  return out
}
