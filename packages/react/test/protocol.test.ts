// §7.1 / §9.5 wire shapes — `packages/react/src/protocol.ts` against the
// Rust render crate.
//
// protocol.ts is REPLICATED from `crates/render/src/lib.rs` (see its module
// doc): the Rust crate is the single source of truth for the streamed
// boundary/island/bootstrap byte shapes, and the server emits the Rust
// versions. A drift between the two silently breaks the deferred-boundary
// swap, island hydration and the router bootstrap in the browser while every
// server-side test stays green — so these tests pin byte parity directly
// against the Rust source text.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import {
  BOUNDARY_FALLBACK_ATTR,
  BOUNDARY_CONTENT_ATTR,
  SWAPPER_SRC,
  boundaryFallback,
  boundaryContent,
  islandElement,
  errorBoundary,
  hydrationBootstrap,
} from '../src/protocol.ts'

/** Read the swapper literal straight out of the Rust source of truth.
 *  Throws (failing the suite) when the constant cannot be located — that
 *  means the Rust shape changed and this file must be re-reviewed. */
function rustSwapperSrc(): string {
  const src = readFileSync(
    fileURLToPath(new URL('../../../crates/render/src/lib.rs', import.meta.url)),
    'utf8',
  )
  const m = /pub const SWAPPER_SRC: &str = r#"(.*)"#;/.exec(src)
  if (!m?.[1]) throw new Error('SWAPPER_SRC not found in crates/render/src/lib.rs — re-pin protocol.ts')
  return m[1]
}

test('protocol: SWAPPER_SRC is byte-identical to the Rust render crate', () => {
  assert.equal(SWAPPER_SRC, rustSwapperSrc())
})

test('protocol: the swapper is a self-contained function(id) with the ox:swap event', () => {
  assert.ok(SWAPPER_SRC.startsWith('function(id){'), 'IIFE-invocable, single id parameter')
  assert.ok(SWAPPER_SRC.includes(`"[data-ox-c='"+id+"']"`), 'selects the content template')
  assert.ok(SWAPPER_SRC.includes(`"[data-ox-b='"+id+"']"`), 'selects the fallback placeholder')
  assert.ok(SWAPPER_SRC.includes('replaceWith.apply(p,d.childNodes)'), 'moves nodes, not HTML strings')
  assert.ok(SWAPPER_SRC.includes('CustomEvent("ox:swap",{detail:{id:id,value:t.innerHTML}})'),
    'dispatches ox:swap with the id + content detail the adapters listen for')
})

test('protocol: boundary attributes match the Rust names', () => {
  assert.equal(BOUNDARY_FALLBACK_ATTR, 'data-ox-b')
  assert.equal(BOUNDARY_CONTENT_ATTR, 'data-ox-c')
})

test('protocol: boundaryFallback emits the §7.1 placeholder template', () => {
  assert.equal(
    boundaryFallback(7, '<span class="skeleton">Loading…</span>'),
    `<template data-ox-b='7'><span class="skeleton">Loading…</span></template>`,
  )
})

test('protocol: boundaryContent emits content template + self-contained swapper', () => {
  const html = boundaryContent(3, '<p id="done">Done</p>')
  assert.ok(html.startsWith(`<template data-ox-c='3'><p id="done">Done</p></template>`))
  assert.ok(html.includes(`(${SWAPPER_SRC})('3')`), 'the swapper IIFE is parenthesized and invoked with the id baked in')
  assert.ok(html.endsWith(`</script>`))
  assert.ok(!html.includes('nonce='), 'no nonce attribute when the nonce is empty (Rust parity)')
})

test('protocol: boundaryContent escapes the nonce and omits it when empty', () => {
  const withNonce = boundaryContent(3, '<p>ok</p>', 'abc"<b')
  assert.ok(withNonce.includes(` nonce='abc&quot;&lt;b'`), `attr-escaped nonce`)
  assert.equal(boundaryContent(3, '<p>ok</p>', ''), boundaryContent(3, '<p>ok</p>'))
})

test('protocol: islandElement emits the §10.3 host markup with escaped attrs', () => {
  const html = islandElement(
    { component: 'Counter', chunk: '/chunks/counter.js', strategy: 'load', propsB64: 'eyJjb3VudCI6MX0=' },
    '<span>0</span>',
  )
  assert.equal(
    html,
    `<ox-island data-ox-island='Counter' data-ox-chunk='/chunks/counter.js'` +
      ` data-ox-strategy='load' data-ox-props='eyJjb3VudCI6MX0='><span>0</span></ox-island>`,
  )
  // SEC-07 (Rust escape_attr): every injected attribute is single-quoted, so
  // a single quote in a directive value MUST become &#39; — otherwise the
  // value breaks out of the attribute. Same for `<`, `>` and `"`.
  const sneaky = islandElement(
    { component: `A'<b`, chunk: 'c', strategy: 's', propsB64: 'p' },
    '',
  )
  assert.ok(sneaky.includes(`data-ox-island='A&#39;&lt;b'`), `quote + tag breakouts are attr-escaped`)
})

test('protocol: errorBoundary escapes code (attr) and message (text)', () => {
  assert.equal(
    errorBoundary('E-01', 'boom <img src=x onerror=alert(1)>'),
    `<div class='ox-error' data-ox-error='E-01'>E-01: boom &lt;img src=x onerror=alert(1)&gt;</div>`,
  )
})

test('protocol: hydrationBootstrap emits the ODF payload + window.__oxide router', () => {
  const html = hydrationBootstrap('c28tZW9kZg==', 'routes/blog/[slug]')
  assert.ok(
    html.startsWith(`<script type='application/oxide-data'>c28tZW9kZg==</script>`),
    'base64 ODF payload script comes FIRST (currentScript.previousElementSibling reads it)',
  )
  // `[`/`]` are outside [a-z0-9-/] too — %5B / %5D.
  assert.ok(html.includes('window.__oxide={route:"routes/blog/%5Bslug%5D",'))
  assert.ok(html.includes('data:document.currentScript.previousElementSibling.textContent}'))
})

test('protocol: sanitizeRouteId percent-encodes every byte outside [a-z0-9-/]', () => {
  // Uppercase bytes are outside the alphabet: `R` → %52, `I` → %49. The dot
  // in `.tsx` → %2E. Nothing outside [a-z0-9-/] may reach the JS string
  // literal unescaped (it sits inside a double-quoted script string).
  const html = hydrationBootstrap('x', 'Routes/Index.tsx')
  assert.ok(html.includes('route:"%52outes/%49ndex%2Etsx"'))
  const unicode = hydrationBootstrap('x', 'routes/café')
  // 'é' is 0xC3 0xA9 in UTF-8 — each byte is percent-encoded separately.
  assert.ok(unicode.includes('routes/caf%C3%A9'))
  const safe = hydrationBootstrap('x', 'routes/blog/hello-world_2')
  // `_` is NOT in the alphabet [a-z0-9-/] — it is percent-encoded too.
  assert.ok(safe.includes('routes/blog/hello-world%5F2'))
  const plain = hydrationBootstrap('x', 'routes/index')
  assert.ok(plain.includes('route:"routes/index"'), 'lowercase/digits/slash pass through untouched')
})
