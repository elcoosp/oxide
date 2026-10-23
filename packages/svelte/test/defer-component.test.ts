// <Defer> — the Svelte adapter's deferred-boundary component (§7.1).
//
// The package ships `src/Defer.svelte` as source; a consumer's Svelte
// compiler builds it. This test does that compile itself (the same
// `svelte/compiler` path `image.test.ts` uses) and asserts the SSR
// output — proving the component is valid Svelte 5 and emits the wire
// marker the core's swapper selects on.
//
// A wrapper is compiled alongside so the fallback/children *snippets*
// can be supplied from markup (Svelte 5 snippets cannot be constructed
// from plain TS). Both compiled modules live in one temp dir under the
// package so `svelte/internal/server` resolves — a `/tmp` path cannot
// see the package's node_modules.
//
// Assertions tolerate Svelte 5's SSR hydration comment markers
// (`<!--[...-->`) which appear around and inside the emitted tree; the
// meaningful assertions are on the element + attribute shapes, not
// adjacency of text.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { compile } from 'svelte/compiler'
import { render } from 'svelte/server'
import { readFileSync, writeFileSync, mkdirSync, rmSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const TMP = join(here, 'defer-compiled')

process.on('exit', () => {
  try { rmSync(TMP, { recursive: true, force: true }) } catch { /* best-effort */ }
})

async function build(): Promise<unknown> {
  rmSync(TMP, { recursive: true, force: true })
  mkdirSync(TMP, { recursive: true })

  const deferSrc = readFileSync(join(here, '../src/Defer.svelte'), 'utf8')
  const deferOut = compile(deferSrc, {
    generate: 'server',
    name: 'Defer',
    filename: 'Defer.svelte',
  })
  writeFileSync(join(TMP, 'Defer.mjs'), deferOut.js.code)

  const wrapperSrc = `<script>
  import Defer from './Defer.mjs'
  let { value = undefined, fallbackText = 'Loading…' } = $props()
</script>

<Defer {value}>
  {#snippet children({ value })}<span class="resolved">{JSON.stringify(value)}</span>{/snippet}
  {#snippet fallback()}<p class="fallback">{fallbackText}</p>{/snippet}
</Defer>
`
  const wrapperOut = compile(wrapperSrc, {
    generate: 'server',
    name: 'Wrapper',
    filename: 'Wrapper.svelte',
  })
  writeFileSync(join(TMP, 'Wrapper.mjs'), wrapperOut.js.code)

  const mod = (await import(pathToFileURL(join(TMP, 'Wrapper.mjs')).href)) as { default: unknown }
  return mod.default
}

const Wrapper = await build()

test('a placeholder value renders <ox-defer data-ox-b="id"> with the fallback', () => {
  const { body } = render(Wrapper as never, {
    props: { value: { __deferred: 0 }, fallbackText: 'Loading…' },
  })
  // The wire marker the swapper's attribute-only selector matches.
  assert.match(body, /<ox-defer[^>]*\bdata-ox-b="0"/, body)
  // Fallback content rides inside the host element; tolerate the SSR
  // hydration comment markers Svelte 5 emits around it.
  assert.match(body, /<p[^>]*class="fallback"[^>]*>Loading…<\/p>/, body)
  // The children snippet is not rendered while the value is unresolved.
  assert.doesNotMatch(body, /class="resolved"/, body)
})

test('an already-resolved value renders children inline — no <ox-defer>', () => {
  const { body } = render(Wrapper as never, { props: { value: 'hello' } })
  assert.doesNotMatch(body, /<ox-defer/, body)
  assert.match(body, /<span[^>]*class="resolved"[^>]*>"hello"<\/span>/, body)
  assert.doesNotMatch(body, /class="fallback"/, body)
})

test('a placeholder with extra keys is NOT a boundary (shape must be exact)', () => {
  const { body } = render(Wrapper as never, {
    props: { value: { __deferred: 0, extra: true } },
  })
  assert.doesNotMatch(body, /<ox-defer/, body)
  assert.match(body, /class="resolved"/, body)
})
