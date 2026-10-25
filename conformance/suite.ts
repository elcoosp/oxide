// Oxide conformance suite (spec §22, D20): ONE assertion set run against ALL
// FOUR framework adapters. A release is blocked if any adapter regresses —
// test/conformance.test.ts asserts every assertion passes for EVERY adapter,
// and run.ts prints the table and exits non-zero on any failure.
//
// Covered (§22 list): SSR output, deterministic markup (no hydration
// mismatch), streaming + deferred data, error and not-found boundaries, head
// and metadata (title template + merge), islands markup + hydration,
// actions with JS on and off, server functions (stub POST shape), caching and
// revalidation shapes, and the client router contract (§8.4). Source maps are
// build-level and NOT asserted here.
//
// The suite uses the CORE client runtime (packages/oxide): createRouter,
// enhance, hydrateIslands — the same modules apps ship — plus a hand-rolled
// fake DOM (no jsdom, no deps).

import nodeAssert from 'node:assert/strict'
import type { FrameworkAdapter, ClientInput } from '../packages/core/src/types.ts'
import { createRouter } from '../packages/core/src/client/router.ts'
import { decodeIslandProps } from '../packages/core/src/client/islands.ts'
import { enhance } from '../packages/core/src/client/enhance.ts'
import { hydrateIslands } from '../packages/core/src/client/islands.ts'
import { serverFn, getServerFn, revalidate, _revalidateLog } from '../packages/core/src/server/index.ts'
import { encodeLite } from '../packages/core/test/helpers/odf-lite.ts'
import { createFakeDom, simulateSwap } from '../packages/react/test/helpers/fake-dom.ts'
import { atlasRoutes as atlas, expect, asRenderInput, type AtlasRoute } from './atlas/atlas.ts'

export interface Assertion {
  name: string
  pass: boolean
  detail?: string
}

export interface SuiteOptions {
  /** Build-generated island chunk map (§10.3) — each island is its own chunk. */
  islandChunks?: Record<string, unknown>
}

export type Adapter = FrameworkAdapter

// ---------------------------------------------------------------------------
// Framework markers (the per-adapter slice of the single assertion set)
// ---------------------------------------------------------------------------

const MARKERS: Record<string, { root: RegExp; extra?: RegExp }> = {
  react: { root: /data-reactroot/ },
  vue: { root: /data-v-app/, extra: /data-v-[0-9a-f]{6}/ },
  svelte: { root: /class="[^"]*svelte-[0-9a-f]{6}"/ },
  solid: { root: /data-solid/, extra: /data-hk="[0-9a-f]{6}-\d+"/ },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const decoder = new TextDecoder()

/** Narrowing helper for noUncheckedIndexedAccess; throws instead of
 *  returning T | undefined. Same pattern as core tests. */
function mustEl<T>(arr: readonly T[], i: number): T {
  const v = arr[i]
  if (v === undefined) throw new Error(`conformance: index ${i} out of bounds`)
  return v
}

/** Bridge the fake-DOM element to Element at the test-fixture boundary.
 *  FakeElement implements a small subset of the DOM; a real Element has
 *  100+ members no hand-built fake can honestly satisfy. This is the one
 *  documented assertion per fake-DOM interaction (same rationale as asDoc
 *  in scripts.test.ts). */
function asElement(v: { innerHTML: string; ownerDocument?: unknown }): Element {
  return v as unknown as Element
}


async function drain(stream: ReadableStream<Uint8Array>): Promise<{ chunks: string[]; html: string }> {
  const chunks: string[] = []
  for await (const chunk of stream) chunks.push(decoder.decode(chunk))
  return { chunks, html: chunks.join('') }
}

const tick = (ms = 5): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms))

/** Run one assertion, converting a throw into a failing result. */
async function check(name: string, fn: () => Promise<string> | string): Promise<Assertion> {
  try {
    const detail = await fn()
    return { name, pass: true, detail }
  } catch (err) {
    return { name, pass: false, detail: err instanceof Error ? err.message : String(err) }
  }
}

function assert(condition: unknown, message: string): void {
  if (!condition) throw new Error(message)
}

/** deepEqual with a thrown Error so the assertion detail lands in `detail`. */
function deepEqual<T>(actual: T, expected: T, message: string): void {
  try {
    nodeAssert.deepEqual(actual, expected, message)
  } catch (err) {
    throw new Error(`${message} (${err instanceof Error ? err.message : String(err)})`)
  }
}

// ---------------------------------------------------------------------------
// The suite
// ---------------------------------------------------------------------------

/**
 * Run the full assertion set against one adapter (§22). ~22 assertions;
 * the same set runs for every adapter so a regression anywhere blocks.
 */
export async function runSuite(adapter: Adapter, opts: SuiteOptions = {}): Promise<Assertion[]> {
  const name = adapter.name
  const marker = MARKERS[name] ?? { root: /data-ox/ }
  const render = async (r: AtlasRoute): Promise<string> => (await drain(adapter.render(asRenderInput(r)))).html
  const results: Assertion[] = []
  const push = async (n: string, fn: () => Promise<string> | string): Promise<void> => void results.push(await check(n, fn))

  // --- §7 contract ---------------------------------------------------------

  await push('contract: manifest shape (§7)', () => {
    assert(adapter.contract === 1, 'contract must be 1')
    assert(typeof adapter.name === 'string' && adapter.name.length > 0, 'name required')
    assert(Array.isArray(adapter.extensions) && adapter.extensions.length > 0, 'extensions required')
    assert(adapter.capabilities.islands === true, 'islands capability required (§10.3)')
    assert(['native', 'core'].includes(adapter.capabilities.suspense), "suspense must be 'native' | 'core'")
    assert(['refresh', 'component', 'full'].includes(adapter.capabilities.hmr), 'hmr must be refresh|component|full')
    const plugins = adapter.plugins({})
    assert(Array.isArray(plugins) && plugins.every((p) => typeof p.name === 'string'), 'plugins() returns named plugins')
    const expected = { react: 'native', vue: 'core', svelte: 'core', solid: 'native' }[name]
    assert(expected === undefined || adapter.capabilities.suspense === expected, `suspense must be '${expected}' for ${name}`)
    return `suspense=${adapter.capabilities.suspense} hmr=${adapter.capabilities.hmr}`
  })

  // --- §22 SSR output ------------------------------------------------------

  await push('ssr: home route content + framework marker', async () => {
    const html = await render({ ...atlas.home })
    assert(html.startsWith(expect.layout.doctype), 'doctype first')
    assert(html.includes(expect.home.content), 'route content rendered')
    assert(html.includes(expect.home.intro), 'loader data interpolated')
    assert(html.includes(expect.home.title), 'title rendered')
    assert(marker.root.test(html), `framework root marker for ${name}`)
    if (marker.extra) assert(marker.extra.test(html), `framework flavor marker for ${name}`)
    return `marker=${marker.root}`
  })

  await push('ssr: document structure + merged head', async () => {
    const html = await render({ ...atlas.home })
    assert(html.includes(expect.layout.charset), 'charset meta')
    assert(html.includes(expect.layout.viewport), 'layout viewport meta')
    assert(html.includes('content="Atlas home route"'), 'loader head meta merged')
    assert(html.includes('content="atlas-home"'), 'page <head> wrapper extracted (§7 head extraction)')
    const headEnd = html.indexOf('</head>')
    assert(headEnd > 0 && html.slice(0, headEnd).includes('content="atlas-home"'), 'extracted meta lives in <head>')
    return 'layout ⊕ loader ⊕ page head'
  })

  await push('hydration: deterministic markup, home (no mismatch)', async () => {
    const a = await render({ ...atlas.home })
    const b = await render({ ...atlas.home })
    assert(a === b, 'render twice → byte-identical (deterministic hydration markup, §7.2)')
    return `${a.length} bytes`
  })

  await push('hydration: deterministic markup, blog/[slug]', async () => {
    const a = await render({ ...atlas.blog })
    const b = await render({ ...atlas.blog })
    assert(a === b, 'render twice → byte-identical')
    return `${a.length} bytes`
  })

  await push('ssr: params + data reach the page (blog/[slug])', async () => {
    const html = await render({ ...atlas.blog })
    assert(html.includes(expect.blog.title), 'route title + template applied')
    assert(html.includes(expect.blog.post), 'data title')
    assert(html.includes(expect.blog.byline), 'param + author byline')
    assert(html.includes(expect.blog.body), 'body data')
    for (const c of expect.blog.comments) assert(html.includes(c), `comment: ${c}`)
    return 'params+data ok'
  })

  // --- §7.1 streaming ------------------------------------------------------

  await push('streaming: shell carries boundary placeholder + fallback', async () => {
    const { chunks, html } = await drain(adapter.render(asRenderInput(atlas.streaming)))
    const shell = chunks.slice(0, 2).join('')
    assert(shell.includes("<template data-ox-b='0'>"), 'template[data-ox-b] placeholder in shell (§7.1)')
    assert(shell.includes(expect.streaming.fallback), 'fallback content in placeholder')
    assert(!shell.includes(expect.streaming.resolved), 'resolution NOT in the shell')
    assert(html.includes(expect.streaming.resolved), 'resolution arrives later in the stream')
    return `${chunks.length} chunks`
  })

  await push('streaming: deferred resolution chunk + swapper script', async () => {
    const { chunks } = await drain(adapter.render(asRenderInput(atlas.streaming)))
    const later = chunks.slice(2).join('')
    assert(later.includes("<template data-ox-c='0'>"), 'template[data-ox-c] resolution (§7.1)')
    assert(later.includes(expect.streaming.resolved), 'resolved content')
    assert(later.includes("('0')"), 'swapper IIFE with boundary id baked in')
    assert(later.includes('ox:swap'), 'swapper dispatches the ox:swap CustomEvent')
    assert(later.includes('replaceWith'), 'DOM-API-only swapper (CSP-safe, no eval)')
    return 'resolution + swapper'
  })

  await push('streaming: swap lands resolved content in the DOM', async () => {
    const { chunks } = await drain(adapter.render(asRenderInput(atlas.streaming)))
    const dom = createFakeDom(chunks.slice(0, 2).join(''))
    let swaps = 0
    dom.doc.addEventListener('ox:swap', () => swaps++)
    assert(simulateSwap(dom.doc, chunks.slice(2).join('')), 'swapper found placeholder + resolution')
    assert(swaps === 1, 'ox:swap dispatched once')
    assert(dom.body!.innerHTML.includes(expect.streaming.resolved), 'resolved content replaced the placeholder')
    assert(!dom.doc.querySelector("template[data-ox-b='0']"), 'placeholder removed')
    return 'swap ok'
  })

  await push('streaming: a non-template boundary host swaps too', async () => {
    // §7.1 gives `suspense: 'core'` adapters a `<Defer>` component. A
    // framework cannot render the placeholder through `<template>`: Vue
    // emits its hydration markers inside one, and the swapper's
    // `innerHTML` would move those markers into the live DOM. So the host
    // is a framework element — `<ox-defer data-ox-*>` — and the swapper's
    // selectors are attribute-only to match either shape.
    const host = `<ox-defer data-ox-b='0'><p>fallback</p></ox-defer>`
    const dom = createFakeDom(`<body>${host}</body>`)
    let swaps = 0
    dom.doc.addEventListener('ox:swap', () => swaps++)
    const resolution = `<template data-ox-c='0'><p>resolved</p></template>`
    assert(simulateSwap(dom.doc, resolution), 'swapper found the framework host')
    assert(swaps === 1, 'ox:swap dispatched once')
    assert(dom.body!.innerHTML.includes('resolved'), 'resolved content replaced the host')
    assert(!dom.doc.querySelector("[data-ox-b='0']"), 'host removed')
    return 'attribute-only selector'
  })

  await push('streaming: rejected boundary streams the error boundary', async () => {
    const { chunks, html } = await drain(adapter.render(asRenderInput(atlas.streamingError)))
    assert(html.includes(expect.error.boundary), 'error boundary in the stream (§10.2 step 5)')
    assert(html.includes(expect.streamingError.message), 'error message surfaced')
    const later = chunks.slice(2).join('')
    assert(later.includes('data-ox-c'), 'error streamed as a boundary resolution')
    return 'error mapped to core model'
  })

  // --- boundaries ----------------------------------------------------------

  await push('error: thrown render renders the error boundary', async () => {
    const html = await render({ ...atlas.error })
    assert(html.includes(expect.error.boundary), 'render-crate error boundary shape')
    assert(html.includes(expect.error.message), 'error message rendered')
    assert(!html.includes(expect.home.content), 'page content replaced by the boundary')
    return 'OX-500'
  })

  await push('not-found: boundary renders for missing routes', async () => {
    const html = await render({ ...atlas.notFound })
    assert(html.includes(expect.notFound.marker), 'not-found marker')
    assert(html.includes(expect.notFound.status), '404 status text')
    return 'not-found ok'
  })

  // --- §16.3 head / metadata -----------------------------------------------

  await push('head: title template %s applied', async () => {
    const html = await render({ ...atlas.head })
    assert(html.includes(expect.head.title), 'layout %s template + route title merged')
    return expect.head.title
  })

  await push('head: loader + page + layout head merged', async () => {
    const html = await render({ ...atlas.head })
    assert(html.includes(expect.head.description), 'loader/defineMeta description meta')
    assert(html.includes(expect.head.pageExtracted), 'page-extracted head element')
    assert(html.includes(expect.head.layout), 'layout meta still present')
    return '3 sources merged'
  })

  await push('head: head(ctx) returns the neutral HeadTags model', () => {
    const ctx = {
      route: atlas.head.route, url: atlas.head.url, params: atlas.head.params,
      data: { title: 'Head Test', description: 'Head route description' },
      locals: { 'oxide:head': { meta: [{ name: 'x-atlas', content: 'ctx-locals' }], link: [], style: [], script: [] } },
    }
    const tags = adapter.head(ctx)
    assert(tags.title === 'Head Test', 'title from data')
    assert(tags.titleTemplate === '%s · Atlas', 'titleTemplate in the model')
    assert(tags.meta.some((m) => m.content === 'ctx-locals'), 'route head tags via ctx.locals merged')
    assert(Array.isArray(tags.link) && Array.isArray(tags.style) && Array.isArray(tags.script), 'neutral model arrays')
    return 'neutral model ok'
  })

  // --- §10.3 islands -------------------------------------------------------

  await push('islands: ox-island markup with props + chunk (§10.3)', async () => {
    const html = await render({ ...atlas.islands })
    assert(html.includes(expect.islands.open), 'island host + component')
    assert(html.includes(expect.islands.chunk), 'chunk reference')
    assert(html.includes(expect.islands.strategy), 'strategy directive')
    const m = /data-ox-props='([^']+)'/.exec(html)
    assert(m, 'props attribute present')
    // PERF-01: props ride ODF (JSON fallback for the non-subset);
    // decodeIslandProps sniffs and handles both.
    if (m === null) throw new Error("conformance: island props missing")
    const propsB64 = m[1]
    if (propsB64 === undefined) throw new Error("conformance: island props empty")
    const props = decodeIslandProps(propsB64)
    deepEqual(props, expect.islands.props, 'props round-trip')
    assert(html.includes(expect.islands.ssr), 'SSR content inside the island')
    return 'markup + props ok'
  })

  await push('islands: core loader hydrates the host + handlers work', async () => {
    const html = await render({ ...atlas.islands })
    const dom = createFakeDom(html)
    const host = dom.doc.querySelector('ox-island')
    assert(host, 'island host in SSR markup')
    const before = host!.innerHTML
    let dispatched = 0
    const dispose = hydrateIslands({
      adapter: {
        hydrateIsland: (el, mod, props) => {
          dispatched++
          return adapter.hydrateIsland!(el as Element, mod, props)
        },
      },
      doc: dom.doc,
      loadChunk: async (chunk) => {
        const mod = opts.islandChunks?.[chunk]
        assert(mod, `island chunk '${chunk}' resolvable`)
        return mod
      },
    })
    await tick()
    assert(dispatched === 1, 'loader dispatched into adapter.hydrateIsland (§10.3)')
    assert(host!.innerHTML === before, 'island hydration renders identical markup')
    const button = host!.querySelector('button')
    assert(button, 'interactive island content')
    button!.dispatchEvent({ type: 'click' })
    assert(host!.innerHTML.includes('>4</span>'), 'handler updated the island via client update')
    dispose()
    return 'load strategy + hydration ok'
  })

  // --- §9.3 actions --------------------------------------------------------

  await push('actions: JS-off POST re-render applies form data', async () => {
    const after = await render({ ...atlas.actions })
    assert(after.includes(expect.actions.form), 'plain POST form (works with JS off)')
    assert(after.includes(expect.actions.applied), 'form data applied by the action re-render')
    assert(after.includes(expect.actions.result), 'action result rendered')
    const before = await render({ ...atlas.actionsEmpty })
    assert(before.includes(expect.actions.pending), 'no data → pending marker')
    return 'progressive enhancement ok'
  })

  await push('actions: enhance POSTs and applies the result (JS on)', async () => {
    const posts: Array<{ url: string; method: string }> = []
    const applied: unknown[] = []
    const listeners = new Map<string, Array<(e: unknown) => void>>()
    const form = {
      // Absolute URL: Node's Request has no document base to resolve '/actions'.
      getAttribute(n: string) { return n === 'action' ? 'http://atlas.test/actions' : null },
      hasAttribute(n: string) { return n === 'action' },
      addEventListener(type: string, fn: (e: unknown) => void) { listeners.set(type, [...(listeners.get(type) ?? []), fn]) },
      removeEventListener(type: string, fn: (e: unknown) => void) { listeners.set(type, (listeners.get(type) ?? []).filter((f) => f !== fn)) },
      dispatchEvent(ev: { type: string }) {
        for (const fn of listeners.get(ev.type) ?? []) fn({ ...ev, preventDefault: () => {} })
        return true
      },
    }
    const dispose = enhance(form, {
      fetch: (async (input: string | Request | URL, init?: RequestInit) => {
        const req = new Request(input, init)
        posts.push({ url: req.url, method: req.method })
        return new Response(JSON.stringify({ ok: true, message: 'Hello Grace' }), { headers: { 'content-type': 'application/json' } })
      }) as typeof fetch,
      getFormData: () => { const fd = new FormData(); fd.append('name', 'Grace'); return fd },
      applyResult: (result) => applied.push(result),
    })
    form.dispatchEvent({ type: 'submit' })
    await tick(10)
    dispose()
    assert(posts.length === 1, 'one POST per submit')
    assert(mustEl(posts, 0).method === 'POST', 'actions are POST-only (§17.1)')
    assert(mustEl(posts, 0).url.endsWith('/actions'), 'form action target')
    deepEqual(applied[0], { ok: true, message: 'Hello Grace' }, 'action result applied')
    return 'enhance ok'
  })

  // --- §9.4 server functions ------------------------------------------------

  await push('server-fns: registry dispatch via stub POST shape', async () => {
    const hello = serverFn('atlas-hello', (who: string) => ({ ok: true, data: `Hello ${who}` }))
    assert(hello.$$oxideServerFn === true, 'server fn tagged (§9.4)')
    assert(getServerFn('atlas-hello') === hello, 'registered under its id')
    // Stub of the pipeline endpoint (Rust owns the real route, §9.4):
    // POST /_oxide/fn/<id> with a JSON {args} body → dispatch → JSON result.
    const req = new Request('http://atlas.test/_oxide/fn/atlas-hello', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ args: ['Grace'] }),
    })
    assert(req.method === 'POST', 'POST shape')
    const body = (await req.json()) as { args: string[] }
    const result = await (getServerFn('atlas-hello') as (...a: unknown[]) => unknown)(...body.args)
    deepEqual(result, { ok: true, data: 'Hello Grace' }, 'dispatch result')
    return 'POST /_oxide/fn/<id> ok'
  })

  // --- §11 caching / revalidation -------------------------------------------

  await push('cache: revalidate event shapes + cache meta', async () => {
    const before = _revalidateLog.length
    const tagEv = revalidate.tag('atlas:posts')
    const pathEv = revalidate.path('/blog')
    const fresh = _revalidateLog.slice(before)
    deepEqual(tagEv, { type: 'tag', tag: 'atlas:posts' }, 'tag event shape (§11.3)')
    deepEqual(pathEv, { type: 'path', path: '/blog' }, 'path event shape')
    deepEqual(fresh, [tagEv, pathEv], 'recorded in order')
    const html = await render({ ...atlas.cached })
    assert(html.includes(expect.cached.meta), 'cache policy surfaced (tags + life)')
    return 'shapes ok'
  })

  // --- §8.4 client router ---------------------------------------------------

  await push('router: goto dispatches adapter.update via createRouter (§8.4)', async () => {
    // Fake browser (hand-rolled, same pattern as packages/oxide router tests).
    const listeners = new Map<string, Array<(e: unknown) => void>>()
    const doc = {
      addEventListener(type: string, fn: (e: unknown) => void) { listeners.set(type, [...(listeners.get(type) ?? []), fn]) },
      removeEventListener(type: string, fn: (e: unknown) => void) { listeners.set(type, (listeners.get(type) ?? []).filter((f) => f !== fn)) },
      querySelectorAll() { return [] },
    }
    const history = { state: null as unknown, pushState(s: unknown) { history.state = s }, replaceState(s: unknown) { history.state = s } }
    const location = { href: 'http://atlas.test/', pathname: '/', search: '' }
    const fetchStub = (async () =>
      new Response(encodeLite({
        route: 'routes/blog/[slug]', url: '/blog/nav', params: { slug: 'nav' },
        data: { title: 'POST title', author: 'Ada Lovelace', body: 'One core, four frameworks.' },
      }), { headers: { 'content-type': 'application/oxide+odf' } })) as typeof fetch
    // Mount the home route, then navigate through the CORE router.
    const dom = createFakeDom('<div id="root"></div>')
    const root = dom.root!
    const disposeMount = adapter.mount(asElement(root), { route: 'routes/index', url: '/', params: {}, data: { framework: 'every framework' } })
    assert(root.innerHTML.includes(expect.home.content), 'SPA mount rendered the shell')
    const updates: Array<ClientInput> = []
    const router = createRouter({
      adapter: {
        update(input) {
          updates.push(input)
          adapter.update(input) // the real adapter swap (§7 update)
        },
      },
      manifest: { routes: {} },
      doc: doc,
      history: history,
      location: location,
      fetch: fetchStub,
    })
    const disposeRouter = router.attach()
    await router.goto('/blog/nav')
    assert(updates.length === 1, 'goto dispatched adapter.update exactly once')
    {
        const u0 = mustEl(updates, 0)
        const u0data = u0.data
        const u0title = u0data !== null && typeof u0data === 'object' && 'title' in u0data
          ? (u0data as { title: unknown }).title
          : undefined
        assert(u0title === 'POST title', 'ODF route data decoded (§9.5)')
      }
    assert(root.innerHTML.includes('POST title'), 'adapter.update swapped the route tree in the DOM')
    assert(root.innerHTML.includes('By Ada Lovelace · nav'), 'params decoded')
    disposeRouter()
    disposeMount()
    return 'goto → update → DOM swap'
  })

  // --- §7.1 out-of-order deferred streaming ---------------------------------

  await push('streaming: deferred boundaries resolve out of declaration order (§7.1)', async () => {
    // The streaming route declares `comments` then `meta`. To prove the
    // adapter streams each boundary AS IT SETTLES (not in declaration order),
    // resolve `meta` (declared 2nd) BEFORE `comments` (declared 1st) via real
    // timers: a concurrent renderer yields meta-then-comments, a sequential
    // one yields comments-then-meta. Already-resolved promises cannot test
    // this — they settle in creation order.
    //
    // `comments` must resolve to an ARRAY of `{author, body}` — the page maps
    // over it; a bare string would throw (and the boundary would render the
    // OX-500 box instead of a comment body).
    const comments = new Promise((resolve) =>
      setTimeout(
        () =>
          resolve([
            { author: 'Grace Hopper', body: 'First comment' },
            { author: 'Alan Turing', body: 'Second comment' },
          ]),
        30,
      ),
    )
    const meta = new Promise((resolve) =>
      setTimeout(() => resolve(expect.streaming.metaResolved), 0),
    )
    const input = asRenderInput({
      route: 'routes/streaming',
      url: '/streaming',
      params: {},
      data: { comments, meta },
      head: { title: 'Streaming', meta: [], link: [], style: [], script: [] },
    })
    const { html } = await drain(adapter.render(input))
    const commentsAt = html.indexOf('First comment')
    const metaAt = html.indexOf(expect.streaming.metaResolved)
    assert(commentsAt > 0, `the comments boundary resolved: ${html.slice(-600)}`)
    assert(metaAt > 0, `the meta boundary resolved: ${html.slice(-600)}`)
    assert(
      metaAt < commentsAt,
      `meta (settles first) streams before comments (settles second): meta@${metaAt} comments@${commentsAt}`,
    )
    return 'resolution order follows settlement order'
  })

  // --- §8.4 prefetch --------------------------------------------------------

  await push('router: prefetch fetches /__data and a later goto reuses it (§8.4)', async () => {
    let fetches = 0
    const listeners = new Map<string, Array<(e: unknown) => void>>()
    const doc = {
      addEventListener(type: string, fn: (e: unknown) => void) { listeners.set(type, [...(listeners.get(type) ?? []), fn]) },
      removeEventListener(type: string, fn: (e: unknown) => void) { listeners.set(type, (listeners.get(type) ?? []).filter((f) => f !== fn)) },
      querySelectorAll() { return [] },
    }
    const history = { state: null as unknown, pushState(s: unknown) { history.state = s }, replaceState(s: unknown) { history.state = s } }
    const location = { href: 'http://atlas.test/', pathname: '/', search: '' }
    const fetchStub = (async () => {
      fetches++
      return new Response(encodeLite({
        route: 'routes/blog/[slug]', url: '/blog/pf', params: { slug: 'pf' },
        data: { title: 'Prefetched', author: 'Ada', body: 'x' },
      }), { headers: { 'content-type': 'application/oxide+odf' } })
    }) as typeof fetch
    const updates: Array<ClientInput> = []
    const router = createRouter({
      adapter: { update(input) { updates.push(input) } },
      manifest: { routes: {} },
      doc: doc,
      history: history,
      location: location,
      fetch: fetchStub,
    })
    const dispose = router.attach()
    // Prefetch warms the data cache; the subsequent goto must not re-fetch.
    router.prefetch('/blog/pf')
    await new Promise((r) => setTimeout(r, 20))
    assert(fetches === 1, 'prefetch issued exactly one /__data fetch')
    await router.goto('/blog/pf')
    assert(fetches === 1, 'goto reused the prefetched data (no second fetch)')
    assert(updates.length === 1, 'goto still dispatched adapter.update')
    dispose()
    return 'prefetch → cache hit'
  })

  return results
}
