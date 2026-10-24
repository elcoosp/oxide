// @oxide/solid adapter tests (spec §7, §7.1, §10.3, §22): manifest shape,
// SSR markup + markers, streaming protocol, determinism, error/not-found
// boundaries, head merge, islands, and client hydration against the fake DOM.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { adapter, render, renderToString, renderShellToString, head, mergeHead, applyTitleTemplate, islandModules } from '../src/index.ts'
import { hydrate, hydrateIsland, mount, update } from '../src/hydrate.ts'
import { createFakeDom, simulateSwap } from './helpers/fake-dom.ts'
import type { RenderInput, HeadTags } from '../../core/src/types.ts'
import { decodeIslandProps } from '../../core/src/client/islands.ts'

const input = (overrides: Partial<RenderInput> = {}): RenderInput => ({
  route: 'routes/index',
  url: '/',
  params: {},
  data: { framework: 'react' },
  head: { title: 'Home', meta: [{ name: 'description', content: 'Atlas home route' }], link: [], style: [], script: [] },
  ...overrides,
})

test('contract: manifest shape satisfies §7 (contract 1, capabilities, plugins)', () => {
  assert.equal(adapter.contract, 1)
  assert.equal(adapter.name, 'solid')
  assert.deepEqual(adapter.extensions, ['.tsx'])
  assert.equal(adapter.jsxImportSource, '@solidjs/web')
  assert.deepEqual(adapter.capabilities, { suspense: 'native', islands: true, hmr: 'component' })
  const plugins = adapter.plugins({})
  assert.ok(Array.isArray(plugins) && plugins.length >= 1)
  assert.ok(plugins.every((p) => typeof p.name === 'string'))
})

test('ssr: home renders framework-shaped markup (data-solid + data-hk keys + title)', async () => {
  const html = await renderToString(input())
  assert.ok(html.startsWith('<!doctype html>'), 'doctype first')
  assert.ok(html.includes("id=\"root\" data-solid"), 'solid root marker on the container')
  assert.match(html, /data-hk="[0-9a-f]{6}-\d+"/, 'deterministic hydration keys on elements')
  assert.ok(!html.includes('data-reactroot'), 'no react markers')
  assert.ok(html.includes('>Hello Atlas</h1>'), 'route content (flavor-agnostic substring)')
  assert.ok(html.includes('<title>Home · Atlas</title>'), 'title template %s applied')
  assert.ok(html.includes('<meta charset="utf-8">'))
  assert.ok(html.includes('<meta name="description" content="Atlas home route">'))
  assert.ok(html.includes('content="atlas-home"'), 'page <head> wrapper extracted into <head>')
})

test('ssr: streaming stream is pull-based ReadableStream<Uint8Array>', async () => {
  const stream = render(input())
  assert.ok(stream instanceof ReadableStream)
  const reader = stream.getReader()
  const first = await reader.read()
  assert.ok(first.value instanceof Uint8Array)
  await reader.cancel()
})

test('streaming: shell placeholder + deferred resolution chunks (§7.1)', async () => {
  const chunks: string[] = []
  for await (const c of render(input({
    route: 'routes/streaming',
    url: '/streaming',
    data: { comments: Promise.resolve([{ author: 'Grace Hopper', body: 'First comment' }]) },
    head: { title: 'Streaming', meta: [], link: [], style: [], script: [] },
  }))) chunks.push(new TextDecoder().decode(c))
  const shell = chunks.slice(0, 2).join('')
  assert.ok(shell.includes("<template data-ox-b='0'>"), 'boundary placeholder in shell')
  assert.ok(shell.includes('Loading comments…'), 'fallback content')
  assert.ok(!shell.includes('First comment'), 'resolution not in shell')
  const later = chunks.slice(2).join('')
  assert.ok(later.includes("<template data-ox-c='0'>"), 'resolution template')
  assert.ok(later.includes('First comment'), 'resolved content')
  assert.ok(later.includes("('0')"), 'swapper IIFE with id baked in')
  assert.ok(later.includes('ox:swap'), 'swapper dispatches the swap event')
})

test('streaming: simulated swap lands resolved content in the fake DOM', async () => {
  const chunks: string[] = []
  for await (const c of render(input({
    route: 'routes/streaming',
    url: '/streaming',
    data: { comments: Promise.resolve([{ author: 'Grace Hopper', body: 'First comment' }]) },
    head: { title: 'Streaming', meta: [], link: [], style: [], script: [] },
  }))) chunks.push(new TextDecoder().decode(c))
  const dom = createFakeDom(chunks.slice(0, 2).join(''))
  assert.ok(!dom.body!.innerHTML.includes('First comment'))
  let swapped = 0
  dom.doc.addEventListener('ox:swap', () => { swapped++ })
  assert.ok(simulateSwap(dom.doc, chunks.slice(2).join('')), 'swapper found placeholder + resolution')
  assert.equal(swapped, 1, 'ox:swap dispatched once')
  assert.ok(dom.body!.innerHTML.includes('First comment'), 'content moved over the placeholder')
  assert.ok(!dom.doc.querySelector("template[data-ox-b='0']"), 'placeholder replaced')
})

test('deterministic: home + blog render byte-identical twice (no hydration mismatch)', async () => {
  const home = await renderToString(input())
  assert.equal(await renderToString(input()), home)
  const blog = input({ route: 'routes/blog/[slug]', url: '/blog/hello', params: { slug: 'hello' }, data: { title: 'Post', author: 'Ada', body: 'Body' } })
  assert.equal(await renderToString(blog), await renderToString(blog))
})

test('error: thrown render maps to the core error boundary (§10.2 step 5)', async () => {
  const html = await renderToString(input({ route: 'routes/error', url: '/error', head: { title: 'Error', meta: [], link: [], style: [], script: [] } }))
  assert.ok(html.includes("<div class='ox-error' data-ox-error='OX-500'>"), 'render-crate error boundary shape')
  assert.ok(html.includes('Atlas render failure'))
  assert.ok(!html.includes('Hello Atlas'))
})

test('not-found: unknown route renders the not-found boundary', async () => {
  const html = await renderToString(input({ route: 'routes/nope', url: '/nope', head: { title: 'Nope', meta: [], link: [], style: [], script: [] } }))
  assert.ok(html.includes('data-ox-not-found'))
  assert.ok(html.includes('404'))
})

test('head: mergeHead + title template + head(ctx) neutral model', () => {
  const outer: HeadTags = { title: '%s · Atlas', meta: [{ charset: 'utf-8' }], link: [], style: [], script: [] }
  const inner: HeadTags = { title: 'Head Test', meta: [{ charset: 'utf-8' }, { name: 'description', content: 'd' }], link: [], style: [], script: [] }
  const merged = mergeHead(outer, inner)
  assert.equal(merged.title, 'Head Test', 'inner title wins')
  assert.equal(applyTitleTemplate(merged), 'Head Test · Atlas', '%s template applied')
  assert.equal(merged.meta.length, 2, 'meta deduped by key, inner appended')
  assert.equal(merged.meta[0].charset, 'utf-8')
  assert.equal(merged.meta[1].name, 'description')
  const ctx = head({ route: 'routes/head', url: '/head', params: {}, data: { title: 'Ctx', description: 'ctx-desc' } })
  assert.equal(ctx.title, 'Ctx')
  assert.ok(ctx.meta.some((m) => m.content === 'ctx-desc'))
})

test('islands: ox-island markup matches the render-crate shape (§10.3)', async () => {
  const html = await renderToString(input({
    route: 'routes/islands', url: '/islands',
    data: { count: 3 },
    head: { title: 'Islands', meta: [], link: [], style: [], script: [] },
  }))
  assert.ok(html.includes("<ox-island data-ox-island='Counter'"), 'component name')
  assert.ok(html.includes("data-ox-chunk='/islands/counter.ts'"), 'chunk reference')
  assert.ok(html.includes("data-ox-strategy='load'"), 'strategy')
  const m = /data-ox-props='([^']+)'/.exec(html)
  assert.ok(m, 'props attribute present')
  // PERF-01: props now ride ODF (falling back to JSON for the non-subset);
  // decodeIslandProps handles both.
  const props = decodeIslandProps(m![1])
  assert.deepEqual(props, { initialCount: 3 })
  assert.ok(html.includes('>3</span>'), 'SSR island content rendered')
  assert.ok(islandModules['/islands/counter.ts'], 'island chunk map exposes the module')
})

test('actions: form markup + JS-off POST re-render applies form data', async () => {
  const plain = await renderToString(input({ route: 'routes/actions', url: '/actions', data: {}, head: { title: 'Actions', meta: [], link: [], style: [], script: [] } }))
  assert.ok(plain.includes('<form method="POST" action="/actions"'), 'plain POST target (works with JS off)')
  assert.ok(plain.includes('<!--form-result-pending-->'))
  const after = await renderToString(input({ route: 'routes/actions', url: '/actions', data: { form: { name: 'Ada' } }, head: { title: 'Actions', meta: [], link: [], style: [], script: [] } }))
  assert.ok(after.includes('value="Ada"'), 'form data round-trips through the action re-render')
  assert.ok(after.includes('Hello Ada'), 'form result rendered')
})

test('hydrate: preserves server markup, binds handlers, glue resolves ox:swap', async () => {
  const serverHtml = renderShellToString(input())
  const dom = createFakeDom(serverHtml)
  const root = dom.root!
  const before = root.innerHTML
  const navigated: string[] = []
  const swaps: number[] = []
  const dispose = hydrate(root, { route: 'routes/index', url: '/', params: {}, data: { framework: 'react' } }, {
    doc: dom.doc as unknown as Parameters<typeof hydrate>[2]['doc'],
    navigate: (href) => navigated.push(href),
    onSwap: (d) => swaps.push(d.id),
  })
  assert.equal(root.innerHTML, before, 'hydration must not rewrite server markup (no mismatch)')
  const link = root.querySelector('a.link')!
  link.dispatchEvent({ type: 'click' })
  assert.deepEqual(navigated, ['/blog'], 'click handler bound via data-ox-id marker')
  // ox:swap → core defer glue dispatches onSwap through the adapter hook
  dom.doc.dispatchEvent({ type: 'ox:swap', detail: { id: 7 } })
  assert.deepEqual(swaps, [7])
  dispose()
  link.dispatchEvent({ type: 'click' })
  assert.equal(navigated.length, 1, 'disposer removed listeners')
})

test('hydrateIsland: hydration + interactive update against the fake DOM (§10.3)', async () => {
  const serverHtml = await renderToString(input({ route: 'routes/islands', url: '/islands', data: { count: 3 }, head: { title: 'Islands', meta: [], link: [], style: [], script: [] } }))
  const dom = createFakeDom(serverHtml)
  const host = dom.doc.querySelector('ox-island')!
  assert.ok(host, 'island host present in SSR markup')
  const before = host.innerHTML
  const dispose = hydrateIsland(host, islandModules['/islands/counter.ts'], { initialCount: 3 })
  assert.equal(host.innerHTML, before, 'island hydration renders identical markup')
  const button = host.querySelector('button')!
  button.dispatchEvent({ type: 'click' })
  assert.ok(host.innerHTML.includes('>4</span>'), 'handler updated the island (client.update)')
  dispose()
})

test('mount/update: SPA mount then adapter.update swaps the route tree (§7)', () => {
  const dom = createFakeDom('<div id="root"></div>')
  const root = dom.root!
  const dispose = mount(root, { route: 'routes/index', url: '/', params: {}, data: { framework: 'react' } }, { doc: dom.doc as unknown as Parameters<typeof mount>[2]['doc'] })
  assert.ok(root.innerHTML.includes('Hello Atlas'), 'SPA mount rendered the shell')
  update({ route: 'routes/blog/[slug]', url: '/blog/x', params: { slug: 'x' }, data: { title: 'Nav Post', author: 'Ada', body: 'B' } })
  assert.ok(root.innerHTML.includes('Nav Post'), 'adapter.update swapped the tree')
  assert.ok(!root.innerHTML.includes('Hello Atlas'), 'previous route gone')
  dispose()
  update({ route: 'routes/index', url: '/', params: {}, data: { framework: 'react' } })
  assert.ok(root.innerHTML.includes('Nav Post'), 'update is a no-op after dispose')
})
