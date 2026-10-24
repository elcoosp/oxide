// @oxide/solid real Solid SSR tests (spec §7). Skipped when the upstream
// Solid renderer (either major) is not resolvable.
//
// Solid 2.0 (RC) exposes the web renderer under `@solidjs/web` with a single
// `renderToString` entry — async is native to the reactive graph, so the
// 1.x `renderToStringAsync` variant is gone. The adapter detects either
// major and normalizes, so these tests exercise the real upstream renderer
// without knowing which package resolved.
//
// Escaping note: Solid's SSR escaper escapes `<` and `&` but not `>` for
// text content. Assertions match that documented behavior — the guarantee is
// no raw `<` element opener can emerge from interpolated text.
//
// Attribute note: Solid's `ssrElement` emits a trailing space inside a class
// attribute value (`class="greeting "`); assertions tolerate that
// whitespace rather than assume a normalized form.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  isSolidAvailable, getSolidWeb, getSolidSource, renderSolidToString,
} from '../src/real-ssr.ts'

const available = await isSolidAvailable()
const skip = !available
const source = available ? await getSolidSource() : undefined

async function getWeb(): Promise<Record<string, unknown>> {
  const web = await getSolidWeb()
  if (web === undefined) throw new Error('getSolidWeb: undefined')
  // Spread the object into a fresh Record -- TypeScript treats the spread of
  // an interface-typed object as Record<string, unknown> for a plain target,
  // without needing an index-signature cast.
  return { ...web } as Record<string, unknown>
}

test('real SSR: upstream Solid renderer is resolvable', { skip }, async () => {
  assert.equal(await isSolidAvailable(), true)
  const web = await getSolidWeb()
  assert.ok(web)
  // Either major exposes at least one of the two renderer entries.
  const hasRenderer =
    typeof web.renderToString === 'function' ||
    typeof web.renderToStringAsync === 'function'
  assert.ok(hasRenderer, 'renderer entry present')
  assert.ok(source === '@solidjs/web' || source === 'solid-js/web', `source=${source}`)
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
    // Documented capability gap: this Solid build has no top-level
    // `ssrElement`; the string-factory and determinism tests still exercise
    // the real renderer.
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
  if (typeof escapeFn !== 'function') return
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
