// §7.1 / §9.2: a loader's `event.defer()` value reaches the component.
//
// Svelte 5's SSR path requires *compiled* components, so this mirrors
// `real-adapter.test.ts`: compile an inline SFC to a temp module and import
// it. The component prints `props.data` as JSON so the assertion can see
// exactly what the adapter handed it.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, writeFile, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { randomUUID } from 'node:crypto'
import { createRealAdapter } from '../src/real-adapter.ts'
import { isSvelteAvailable } from '../src/real-ssr.ts'
import type { RenderInput } from '../../core/src/types.ts'

const available = await isSvelteAvailable()

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

/** A component that renders `props.data` verbatim so the test can read what
 *  the adapter passed. Svelte escapes text, so the assertions decode. */
const PROBE_SVELTE = `<script>
  let { data = {} } = $props()
</script>
<pre data-probe>{JSON.stringify(data)}</pre>
`

const input = (overrides: Partial<RenderInput> = {}): RenderInput => ({
  route: 'routes/probe',
  url: '/probe',
  params: {},
  data: {},
  head: { title: 'Probe', meta: [], link: [], style: [], script: [] },
  ...overrides,
})

async function render(overrides: Partial<RenderInput>): Promise<string> {
  const Component = await compileInline(PROBE_SVELTE)
  const adapter = createRealAdapter({
    ssr: 'real',
    componentFor: (route: string) => (route === 'routes/probe' ? Component : undefined),
  })
  const stream = adapter.render(input(overrides))
  const chunks: string[] = []
  for await (const c of stream) chunks.push(new TextDecoder().decode(c))
  return chunks
    .join('')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&amp;/g, '&')
}

test('defer: a placeholder in data is replaced by the resolved value', { skip: !available }, async () => {
  const html = await render({
    data: { comments: { __deferred: 1 }, title: 'Post' },
    deferred: { '1': ['first', 'second'] },
  })
  assert.ok(html.includes('"comments":["first","second"]'), `resolved:\n${html.slice(0, 400)}`)
  assert.ok(!html.includes('__deferred'), `no placeholder survives:\n${html.slice(0, 400)}`)
})

test('defer: the placeholder is untouched when the settle never arrived', { skip: !available }, async () => {
  const html = await render({ data: { comments: { __deferred: 7 } } })
  assert.ok(html.includes('__deferred'), `placeholder preserved:\n${html.slice(0, 400)}`)
})

test('defer: nesting is walked (arrays, objects, depth)', { skip: !available }, async () => {
  const html = await render({
    data: {
      nested: { deep: [{ __deferred: 2 }] },
      list: [{ __deferred: 3 }, 'plain'],
    },
    deferred: { '2': 'inner', '3': 'listed' },
  })
  assert.ok(html.includes('"deep":["inner"]'), `nested:\n${html.slice(0, 400)}`)
  assert.ok(html.includes('["listed","plain"]'), `list:\n${html.slice(0, 400)}`)
})

test('defer: no deferred map is a no-op, not a crash', { skip: !available }, async () => {
  const html = await render({ data: { plain: 'value' } })
  assert.ok(html.includes('"plain":"value"'), `plain data:\n${html.slice(0, 400)}`)
})
