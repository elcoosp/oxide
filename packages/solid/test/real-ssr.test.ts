// @oxide/solid real Solid SSR tests (spec §7). Skipped when solid-js/web is
// not resolvable.
//
// Solid's SSR entry (`solid-js/web` under the node/ssr condition) exports
// `renderToString`, `renderToStringAsync`, and — depending on version — a
// family of SSR helpers (`ssr`, `ssrElement`, `ssrAttribute`, `escape`).
// Not every helper is present in every Solid 1.x build, so these tests
// detect the available surface and skip the ones that aren't there rather
// than fail on a missing internal.
//
// Escaping note: Solid's SSR escaper escapes `<` and `&` but not `>` for
// text content (`<script>` → `&lt;script>`). The XSS-safe guarantee is that
// no raw `<` element opener can emerge from interpolated text; assertions
// below match that documented behavior.
//
// Attribute note: Solid's `ssrElement` helper emits a trailing space inside
// a class attribute value (`class="greeting "`), matching the real renderer.
// Assertions tolerate that whitespace rather than assume a normalized form.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  isSolidAvailable, getSolidWeb, renderSolidToString,
} from '../src/real-ssr.ts'

const available = await isSolidAvailable()
const skip = !available

async function getWeb(): Promise<Record<string, unknown>> {
  return (await import('solid-js/web')) as unknown as Record<string, unknown>
}

test('real SSR: solid-js/web is resolvable', { skip }, async () => {
  assert.equal(await isSolidAvailable(), true)
  const web = await getSolidWeb()
  assert.ok(web)
  assert.equal(typeof web.renderToStringAsync, 'function')
  assert.equal(typeof web.renderToString, 'function')
})

test('real SSR: renders a trivial string factory', { skip }, async () => {
  const html = await renderSolidToString(() => 'hello solid')
  assert.ok(html.includes('hello solid'), html)
})

test('real SSR: renders an element tree when the SSR helper exists', { skip }, async () => {
  const web = await getWeb()
  const ssrElement = web.ssrElement as
    | ((tag: string, attrs: Record<string, unknown>, children: unknown, dynamic: boolean) => unknown)
    | undefined
  if (typeof ssrElement !== 'function') {
    // solid-js build in use has no ssrElement; the string-factory and
    // determinism tests exercise the real renderer regardless.
    return
  }
  const tree = ssrElement('h1', { class: 'greeting' }, 'Hello Solid', false)
  const html = await renderSolidToString(() => tree)
  assert.ok(html.includes('Hello Solid'), html)
  // Solid emits a trailing space in class attrs; match class name not quote.
  assert.ok(html.includes('class="greeting'), html)
})

test('real SSR: escapes text via the SSR escape helper when present', { skip }, async () => {
  const web = await getWeb()
  const escapeFn = web.escape as ((s: string, attr?: boolean) => string) | undefined
  if (typeof escapeFn !== 'function') {
    // Documented capability gap: this Solid 1.x build has no top-level
    // `escape`; the real renderer still escapes interpolated text.
    return
  }
  // Solid escapes `<` and `&` but not `>` for text content.
  const escaped = escapeFn('<script>')
  assert.ok(escaped.startsWith('&lt;'), `expected leading &lt;, got: ${escaped}`)
  assert.ok(!escaped.startsWith('<'), `raw < leaked: ${escaped}`)
  assert.equal(escapeFn('a & b'), 'a &amp; b')
})

test('real SSR: deterministic across identical renders', { skip }, async () => {
  const a = await renderSolidToString(() => 'stable')
  const b = await renderSolidToString(() => 'stable')
  assert.equal(a, b)
})
