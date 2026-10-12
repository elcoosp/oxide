// @oxide/solid real Solid SSR tests (spec §7). Skipped when solid-js/web is
// not resolvable.
//
// Solid's SSR serializes the JSX-compiled tree. Without a build step in the
// test process we drive the runtime with the JSX runtime's `ssr` helper
// (`solid-js/web` exports `ssr` and `ssrElement` from the SSR runtime).

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  isSolidAvailable, getSolidWeb, renderSolidToString,
} from '../src/real-ssr.ts'

const available = await isSolidAvailable()
const skip = !available

/** Build a minimal element tree using solid-js/web's SSR helpers. */
async function buildTree(
  tag: string,
  attrs: Record<string, unknown> | null,
  child: unknown,
): Promise<unknown> {
  const web = (await import('solid-js/web')) as unknown as Record<string, unknown>
  // Solid's SSR runtime exposes `ssrElement` for a single element.
  const ssrElement = web.ssrElement as ((tag: string, attrs: Record<string, unknown>, children: unknown, dynamic: boolean) => unknown) | undefined
  const ssr = web.ssr as ((arr: unknown[]) => unknown) | undefined
  if (typeof ssrElement === 'function') {
    return ssrElement(tag, attrs ?? {}, child, false)
  }
  if (typeof ssr === 'function') {
    // Fallback: `ssr([openTag, children, closeTag])`
    return ssr([`<${tag}>`, child, `</${tag}>`])
  }
  // Last resort: literal string tree (Solid accepts strings/primitives).
  return `<${tag}>${String(child)}</${tag}>`
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

test('real SSR: renders an element tree via SSR helpers', { skip }, async () => {
  const tree = await buildTree('h1', { class: 'greeting' }, 'Hello Solid')
  const html = await renderSolidToString(() => tree)
  assert.ok(html.includes('Hello Solid'), html)
  assert.ok(html.includes('class="greeting"'), html)
})

test('real SSR: escapes interpolated text', { skip }, async () => {
  const tree = await buildTree('p', null, '<script>x</script>')
  const html = await renderSolidToString(() => tree)
  // Solid's SSR escapes string children of elements — check the common case.
  assert.ok(html.includes('&lt;script&gt;') || !html.includes('<script>x</script>'), html)
})

test('real SSR: deterministic across identical renders', { skip }, async () => {
  const html1 = await renderSolidToString(() => 'stable')
  const html2 = await renderSolidToString(() => 'stable')
  assert.equal(html1, html2)
})
