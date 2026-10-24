// @oxide/svelte createRealAdapter factory tests (spec §7). Svelte 5's SSR
// path requires compiled components; tests compile small inline SFCs and
// import them via temp files under packages/svelte/test/.tmp/ (the same
// trick the real-ssr test uses to let bare specifiers resolve).

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, writeFile, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { randomUUID } from 'node:crypto'
import { createRealAdapter } from '../src/real-adapter.ts'
import { isSvelteAvailable } from '../src/real-ssr.ts'
import { adapter as irAdapter } from '../src/index.ts'
import type { RenderInput } from '../../core/src/types.ts'

const available = await isSvelteAvailable()
const realSkip = !available

const TMP_DIR = join(fileURLToPath(new URL('.', import.meta.url)), '.tmp')

interface SvelteCompilerLike {
  compile(src: string, opts: { generate: string; filename: string }): { js: { code: string } }
}

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

const input = (overrides: Partial<RenderInput> = {}): RenderInput => ({
  route: 'routes/index',
  url: '/',
  params: {},
  data: {},
  head: { title: 'Home', meta: [{ name: 'description', content: 'x' }], link: [], style: [], script: [] },
  ...overrides,
})

async function drain(stream: ReadableStream<Uint8Array>): Promise<string> {
  const chunks: string[] = []
  for await (const c of stream) chunks.push(new TextDecoder().decode(c))
  return chunks.join('')
}

test('factory: createRealAdapter returns a §7-shaped FrameworkAdapter', () => {
  const a = createRealAdapter({ ssr: 'auto', componentFor: () => undefined })
  assert.equal(a.contract, 1)
  assert.equal(a.name, 'svelte')
  assert.deepEqual(a.extensions, ['.svelte'])
  assert.deepEqual(a.capabilities, { suspense: 'core', islands: true, hmr: 'component' })
  const plugins = a.plugins({})
  assert.ok(plugins.length >= 1)
  assert.ok(plugins.every((p) => typeof p.name === 'string'))
})

test('factory: no component → falls back to the neutral IR renderer', async () => {
  const a = createRealAdapter({ ssr: 'auto', componentFor: () => undefined })
  const html = await drain(a.render(input()))
  const irHtml = await drain(irAdapter.render(input()))
  assert.equal(html, irHtml, 'IR path byte-identical to the default adapter')
  assert.ok(html.includes('svelte-'), 'svelte scope class preserved')
})

test('factory: ssr=real + no component → OX-500 page (§10.2 step 5)', async () => {
  const a = createRealAdapter({ ssr: 'real', componentFor: () => undefined })
  const html = await drain(a.render(input({ route: 'routes/missing' })))
  assert.ok(html.includes('<!--oxide-status:500-->'), `500 marker: ${html}`)
  assert.ok(html.includes("data-ox-error='OX-500'"), `OX-500 div: ${html}`)
  assert.ok(html.includes('no component resolved for route routes/missing'), `diagnostic wording: ${html}`)
})

test('factory: real strategy renders through svelte/server', { skip: realSkip }, async () => {
  const Component = await compileInline(
    `<script>let { data = {} } = $props();</script>\n<h1 class="greeting">Hello {data.name ?? 'Svelte'}</h1>\n`,
  )
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Component })
  const html = await drain(a.render(input({ data: { name: 'Ada' } })))
  assert.ok(html.includes('Hello Ada'), html)
  assert.ok(html.startsWith('<!doctype html>'), 'doctype first')
  assert.ok(html.includes('<title>Home · Atlas</title>'), 'layout titleTemplate applied')
  assert.ok(html.includes('class="oxide-root"'), 'svelte root container class')
})

test('factory: real strategy escapes text through the real renderer', { skip: realSkip }, async () => {
  const Component = await compileInline(`<p>{'<script>alert(1)</script>'}</p>\n`)
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Component })
  const html = await drain(a.render(input()))
  assert.ok(html.includes('&lt;script>'), `expected escaped <script>, got: ${html}`)
  assert.ok(!html.includes('<script>alert(1)</script>'), 'no raw script element emitted')
})

test('factory: real strategy maps a render throw to the core error boundary', { skip: realSkip }, async () => {
  const Boom = await compileInline(`<script>throw new Error('svelte component exploded')</script>\n`)
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Boom })
  const html = await drain(a.render(input()))
  assert.ok(html.includes("data-ox-error='OX-500'"), html)
  assert.ok(html.includes('svelte component exploded'), html)
})
