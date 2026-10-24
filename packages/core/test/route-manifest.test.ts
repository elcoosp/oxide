// route-manifest tests (§6.4 + §8.4 + §10.3): the manifest → component
// resolution contract every adapter's client update()/hydrate()/mount()
// shares. The chunk importer is injected (RouteComponentSource.loadChunk), so
// no bundler or browser is involved — a stub manifest + a stub loader drive
// the whole path.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  routeIdFor, componentFor, pageComponentExport, layoutChunksFor,
  setRouteComponentSource, getRouteComponentSource, isParamSegment,
  type ClientManifestLike,
} from '../src/client/route-manifest.ts'

const MANIFEST: ClientManifestLike = {
  buildId: 'b1',
  routes: {
    'routes/index/page': { entry_chunk: 'root-page.js' },
    'routes/about/page': { entry_chunk: 'about-page.js' },
    'routes/blog/page': { entry_chunk: 'blog-page.js' },
    'routes/blog/[slug]/page': { entry_chunk: 'slug-page.js' },
    'routes/blog/[slug]/comments/page': { entry_chunk: 'comments-page.js' },
    'routes/docs/_id_/page': { entry_chunk: 'docs-id-page.js', preload: ['docs-shared.js'] },
    // Endpoint + boot rows are never client pages.
    'routes/api/ping': { entry_chunk: 'nope.js' },
    '__boot__': { entry_chunk: 'boot.js' },
  },
}

test('isParamSegment: every spelling the build emits', () => {
  assert.equal(isParamSegment('[slug]'), true)
  assert.equal(isParamSegment('[...rest]'), true)
  assert.equal(isParamSegment('_slug_'), true)
  assert.equal(isParamSegment(':id'), true)
  assert.equal(isParamSegment('blog'), false)
  assert.equal(isParamSegment('[]'), false, 'empty brackets are not a param token')
  assert.equal(isParamSegment('_'), false)
})

test('routeIdFor: exact manifest key wins without matching', () => {
  assert.equal(routeIdFor('routes/blog/[slug]/page', MANIFEST), 'routes/blog/[slug]/page')
})

test('routeIdFor: route label (the /__data + payload form) maps to the key', () => {
  assert.equal(routeIdFor('routes/blog/[slug]', MANIFEST), 'routes/blog/[slug]/page')
  assert.equal(routeIdFor('routes/about', MANIFEST), 'routes/about/page')
})

test('routeIdFor: root URL and root label resolve to routes/index/page', () => {
  assert.equal(routeIdFor('/', MANIFEST), 'routes/index/page')
  assert.equal(routeIdFor('routes/index', MANIFEST), 'routes/index/page')
  assert.equal(routeIdFor('index', MANIFEST), 'routes/index/page')
})

test('routeIdFor: URL path segment-matches, literals over params, longest first', () => {
  assert.equal(routeIdFor('/blog/hello', MANIFEST), 'routes/blog/[slug]/page')
  assert.equal(routeIdFor('/blog/hello/comments', MANIFEST), 'routes/blog/[slug]/comments/page')
  assert.equal(routeIdFor('/blog', MANIFEST), 'routes/blog/page')
  assert.equal(routeIdFor('/docs/42', MANIFEST), 'routes/docs/_id_/page')
  assert.equal(routeIdFor('/about', MANIFEST), 'routes/about/page')
})

test('routeIdFor: a param segment matches exactly one non-empty URL segment', () => {
  // `/blog` alone does NOT match the `[slug]` key (the param needs its segment).
  assert.equal(routeIdFor('/blog/hello/extra', MANIFEST), undefined)
  assert.equal(routeIdFor('/nope', MANIFEST), undefined)
  // Trailing slash + query are tolerated.
  assert.equal(routeIdFor('/blog/hello/', MANIFEST), 'routes/blog/[slug]/page')
  assert.equal(routeIdFor('/blog/hello?x=1', MANIFEST), 'routes/blog/[slug]/page')
})

test('routeIdFor: non-page rows and __boot__ are never matched', () => {
  assert.equal(routeIdFor('/api/ping', MANIFEST), undefined)
  assert.equal(routeIdFor('routes/api/ping', MANIFEST), undefined)
  assert.equal(routeIdFor('__boot__', MANIFEST), undefined)
})

test('routeIdFor: garbage input resolves to undefined', () => {
  assert.equal(routeIdFor('', MANIFEST), undefined)
  // Runtime hardening: signature accepts `unknown` and returns undefined
  // for non-string input. No @ts-expect-error needed.
  assert.equal(routeIdFor(undefined, MANIFEST), undefined)
})

test('pageComponentExport: default → named → bare function', () => {
  assert.equal(pageComponentExport({ default: 'D' }), 'D')
  assert.equal(pageComponentExport({ Page: 'P' }), 'P')
  assert.equal(pageComponentExport({ component: 'c' }), 'c')
  assert.equal(pageComponentExport({ Component: 'C' }), 'C')
  const fn = () => {}
  assert.equal(pageComponentExport(fn), fn, 'a bare component function is itself')
  assert.equal(pageComponentExport({ nothing: 1 }), undefined)
  assert.equal(pageComponentExport(undefined), undefined)
  assert.equal(pageComponentExport(null), undefined)
})

test('componentFor: no registered source → undefined (IR fallback territory)', async () => {
  setRouteComponentSource(undefined)
  assert.equal(getRouteComponentSource(), undefined)
  assert.equal(await componentFor('/blog/hello'), undefined)
})

test('componentFor: resolves the entry_chunk through the injected loader', async () => {
  const loaded: string[] = []
  setRouteComponentSource({
    manifest: MANIFEST,
    loadChunk: async (chunk: string) => {
      loaded.push(chunk)
      return { default: () => 'SLUG-PAGE' }
    },
  })
  const c = (await componentFor('/blog/hello')) as () => string
  assert.equal(c(), 'SLUG-PAGE')
  assert.deepEqual(loaded, ['/client/slug-page.js'], 'chunk URL is root-relative under /client/')
  // Label form resolves to the same route.
  const c2 = (await componentFor('routes/blog/[slug]')) as () => string
  assert.equal(c2(), 'SLUG-PAGE')
})

test('componentFor: falls back to preload[0] when entry_chunk is absent', async () => {
  const loaded: string[] = []
  const docs = () => 'DOCS'
  setRouteComponentSource({
    manifest: {
      routes: { 'routes/docs/_id_/page': { preload: ['docs-shared.js'] } },
    },
    loadChunk: async (chunk: string) => {
      loaded.push(chunk)
      return { Component: docs }
    },
  })
  assert.equal(await componentFor('/docs/7'), docs, 'named Component export resolved')
  assert.deepEqual(loaded, ['/client/docs-shared.js'])
})

test('componentFor: broken chunk or component-less module → undefined, never throws', async () => {
  setRouteComponentSource({
    manifest: MANIFEST,
    loadChunk: async () => {
      throw new Error('chunk 404')
    },
  })
  assert.equal(await componentFor('/blog/hello'), undefined)
  setRouteComponentSource({
    manifest: MANIFEST,
    loadChunk: async () => ({ notAComponent: true }),
  })
  assert.equal(await componentFor('/blog/hello'), undefined)
})

test('componentFor: route missing from the manifest → undefined', async () => {
  setRouteComponentSource({
    manifest: { routes: { 'routes/about/page': { entry_chunk: 'a.js' } } },
    loadChunk: async () => ({ default: () => 'A' }),
  })
  assert.equal(await componentFor('/blog/hello'), undefined)
})

test('componentFor: absolute URLs pass through the importer untouched', async () => {
  const loaded: string[] = []
  setRouteComponentSource({
    manifest: {
      routes: { 'routes/about/page': { entry_chunk: 'https://cdn.test/about.js' } },
    },
    loadChunk: async (chunk: string) => {
      loaded.push(chunk)
      return { default: () => 'A' }
    },
  })
  await componentFor('/about')
  assert.deepEqual(loaded, ['https://cdn.test/about.js'])
})

// ------------------------------ O1: layout chain preload

test('layoutChunksFor walks the chain and returns layout chunk names', () => {
  const manifest: ClientManifestLike = {
    buildId: 'b1',
    routes: {
      'routes/blog/[slug]/page': {
        entry_chunk: 'slug-page.js',
        preload: ['slug-shared.js'],
        layout_chain: ['routes/layout'],
      },
      'routes/layout': {
        entry_chunk: 'root-layout.js',
        preload: ['root-layout-shared.js'],
      },
      'routes/nested/[id]/page': {
        entry_chunk: 'nested-page.js',
        layout_chain: ['routes/layout', 'routes/nested/layout'],
      },
      'routes/nested/layout': {
        entry_chunk: 'nested-layout.js',
      },
    },
  }
  assert.deepEqual(
    layoutChunksFor('routes/blog/[slug]/page', manifest),
    ['root-layout.js', 'root-layout-shared.js'],
  )
  assert.deepEqual(
    layoutChunksFor('routes/nested/[id]/page', manifest),
    ['root-layout.js', 'root-layout-shared.js', 'nested-layout.js'],
  )
})

test('layoutChunksFor returns empty on missing chain / missing layouts / no entry', () => {
  const manifest: ClientManifestLike = {
    routes: {
      'routes/plain/page': { entry_chunk: 'plain.js' },
      'routes/ghost/page': {
        entry_chunk: 'ghost.js',
        layout_chain: ['routes/missing-layout'],
      },
    },
  }
  assert.deepEqual(layoutChunksFor('routes/plain/page', manifest), [])
  assert.deepEqual(layoutChunksFor('routes/ghost/page', manifest), [])
  assert.deepEqual(layoutChunksFor('routes/absent/page', manifest), [])
})

