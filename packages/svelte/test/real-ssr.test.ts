// @oxide/svelte real Svelte SSR tests (spec §7). Skipped when svelte/server
// is not resolvable.
//
// Svelte 5's SSR compile (`svelte/compiler` + `generate: 'server'`) emits an
// ES module that imports from `svelte/internal/server`. A `data:` URL module
// cannot resolve bare specifiers, so the compiled output is written to a
// temp file INSIDE the package tree (packages/svelte/test/.tmp/) and imported
// from there — Node's ESM resolver then walks up to the workspace's
// node_modules and finds `svelte` correctly.
//
// Escaping note: Svelte's SSR escaper escapes `<` and `&` but NOT `>`
// (`<script>` → `&lt;script>`). The XSS-safe guarantee is that no raw `<`
// element opener can emerge from interpolated text; assertions below match
// that documented behavior rather than a stricter HTML5 entity set.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, writeFile, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { randomUUID } from 'node:crypto'
import {
  isSvelteAvailable, getSvelteServer, renderSvelteToString,
} from '../src/real-ssr.ts'

const available = await isSvelteAvailable()
const skip = !available

interface SvelteCompilerLike {
  compile(src: string, opts: { generate: string; filename: string }): { js: { code: string } }
}

const TMP_DIR = join(fileURLToPath(new URL('.', import.meta.url)), '.tmp')

async function compileInline(source: string, filename = 'Test.svelte'): Promise<unknown> {
  const compiler = await import('svelte/compiler')
  const out = compiler.compile(source, { generate: 'server', filename })
  await mkdir(TMP_DIR, { recursive: true })
  const tmpPath = join(TMP_DIR, `${randomUUID()}.mjs`)
  await writeFile(tmpPath, out.js.code, 'utf8')
  try {
    const mod = await import(pathToFileURL(tmpPath).href)
    return (mod as { default: unknown }).default
  } finally {
    await rm(tmpPath, { force: true })
  }
}

test('real SSR: svelte/server is resolvable', { skip }, async () => {
  assert.equal(await isSvelteAvailable(), true)
  const server = await getSvelteServer()
  assert.ok(server)
  assert.equal(typeof server.render, 'function')
})

test('real SSR: renders a compiled Svelte component with props', { skip }, async () => {
  const Component = await compileInline(
    `<script>let { name = 'world' } = $props();</script>\n<h1 class="greeting">Hello {name}</h1>\n`,
  )
  const html = await renderSvelteToString(Component, { name: 'Svelte' })
  assert.ok(html.includes('Hello Svelte'), html)
  assert.ok(html.includes('class="greeting"'), html)
})

test('real SSR: escapes interpolated text content (no raw < opener)', { skip }, async () => {
  const Component = await compileInline(`<p>{'<script>x</script>'}</p>\n`)
  const html = await renderSvelteToString(Component, {})
  // Svelte escapes `<` but not `>`; assert no raw element opener can emerge.
  assert.ok(html.includes('&lt;script>'), `expected escaped <script>, got: ${html}`)
  assert.ok(!html.includes('<script>'), `raw <script> element leaked: ${html}`)
})

test('real SSR: deterministic across identical renders', { skip }, async () => {
  const Component = await compileInline(`<div>stable</div>\n`)
  const a = await renderSvelteToString(Component, {})
  const b = await renderSvelteToString(Component, {})
  assert.equal(a, b)
})
