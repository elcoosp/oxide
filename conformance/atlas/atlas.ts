// Atlas — the Oxide reference app (spec §22, D20): ONE assertion set run
// against ALL FOUR adapters. This module is the app AS DATA: route
// definitions (route id, url, params, loader data, head) plus the expected
// strings for SSR output, head, deferred data, error/not-found boundaries,
// islands markup and actions. The suite (suite.ts) renders Atlas routes
// through each adapter via FrameworkAdapter.render and checks these
// expectations; framework-specific markers live in the suite's MARKERS map.

import type { HeadTags, RenderInput } from '../../packages/core/src/types.ts'

export interface AtlasRoute {
  route: string
  url: string
  params: Record<string, string | string[]>
  data: Record<string, unknown>
  head: HeadTags
}

const HEAD = (title: string, meta: Array<Record<string, string>> = []): HeadTags => ({
  title,
  meta,
  link: [],
  style: [],
  script: [],
})

export const COMMENTS = [
  { author: 'Grace Hopper', body: 'First comment' },
  { author: 'Alan Turing', body: 'Second comment' },
]

/** A rejected deferred slot; the no-op catch silences unhandled-rejection
 * warnings — the adapter still sees the rejection when it awaits (§7.1). */
function rejectedComments(): PromiseLike<unknown> {
  const p = Promise.reject(new Error('Atlas comments offline'))
  p.catch(() => {})
  return p
}

/** The Atlas route table (spec §8.1 page conventions, adapter-neutral). */
export const atlasRoutes: Record<string, AtlasRoute> = {
  home: {
    route: 'routes/index',
    url: '/',
    params: {},
    data: { framework: 'every framework' },
    head: HEAD('Home', [{ name: 'description', content: 'Atlas home route' }]),
  },
  blog: {
    route: 'routes/blog/[slug]',
    url: '/blog/hello-oxide',
    params: { slug: 'hello-oxide' },
    data: {
      title: 'Streaming Atlas post',
      author: 'Ada Lovelace',
      body: 'One core, four frameworks.',
      comments: COMMENTS,
    },
    head: HEAD('Hello Oxide'),
  },
  streaming: {
    route: 'routes/streaming',
    url: '/streaming',
    params: {},
    // RenderInput data may carry promise-likes for deferred slots (§7.1);
    // the adapter streams the resolution via the core boundary protocol.
    //
    // §7.1 out-of-order fixture: `comments` is declared FIRST but resolves
    // LAST (30 ms), `meta` is declared SECOND but resolves FIRST (0 ms). A
    // sequential renderer emits comments-then-meta; a concurrent one emits
    // meta-then-comments — which is what the assertion checks.
    data: {
      comments: new Promise((resolve) => setTimeout(() => resolve(COMMENTS), 30)),
      meta: new Promise((resolve) => setTimeout(() => resolve('meta-ready'), 0)),
    },
    head: HEAD('Streaming'),
  },
  streamingError: {
    route: 'routes/streaming',
    url: '/streaming-error',
    params: {},
    data: { comments: rejectedComments() },
    head: HEAD('Streaming'),
  },
  error: {
    route: 'routes/error',
    url: '/error',
    params: {},
    data: {},
    head: HEAD('Error'),
  },
  notFound: {
    route: 'routes/not-found',
    url: '/not-found',
    params: {},
    data: {},
    head: HEAD('Not Found'),
  },
  head: {
    route: 'routes/head',
    url: '/head',
    params: {},
    data: { description: 'Head route description' },
    head: HEAD('Head Test', [{ name: 'description', content: 'Head route description' }]),
  },
  islands: {
    route: 'routes/islands',
    url: '/islands',
    params: {},
    data: { count: 3 },
    head: HEAD('Islands'),
  },
  actions: {
    route: 'routes/actions',
    url: '/actions',
    params: {},
    // With JS off the action POSTs natively; the server re-renders with the
    // form data applied (§9.3 progressive enhancement).
    data: { form: { name: 'Ada' } },
    head: HEAD('Actions'),
  },
  actionsEmpty: {
    route: 'routes/actions',
    url: '/actions',
    params: {},
    data: {},
    head: HEAD('Actions'),
  },
  cached: {
    route: 'routes/cached',
    url: '/cached',
    params: {},
    data: { posts: ['Cache hits are L0 hits'] },
    head: HEAD('Cached', [{ name: 'ox-cache', content: 'tags=atlas:posts;life=60s' }]),
  },
}

/** Build a RenderInput from an Atlas route (loader data already applied). */
export function asRenderInput(
  r: AtlasRoute & { resolve?: (id: string) => unknown },
): RenderInput {
  const out: RenderInput = {
    route: r.route,
    url: r.url,
    params: r.params,
    data: r.data,
    head: r.head,
  }
  // §7 / §6.4: `resolve` is an optional field the runtime attaches; a
  // test that supplies its own must reach the adapter. Prior versions
  // of this helper dropped every field not enumerated above, so the
  // conformance resolver tests never saw their own `resolve`.
  if (r.resolve) out.resolve = r.resolve
  return out
}

/** Expected strings — one set, every adapter (framework markers in suite.ts). */
export const expect = {
  home: {
    title: '<title>Home · Atlas</title>',
    content: '>Hello Atlas</h1>',
    intro: 'Serving every framework from one core.',
    link: '<a href="/blog"',
  },
  blog: {
    title: '<title>Hello Oxide · Atlas</title>',
    byline: 'By Ada Lovelace · hello-oxide',
    post: 'Streaming Atlas post',
    body: 'One core, four frameworks.',
    comments: ['First comment', 'Second comment'],
  },
  streaming: {
    fallback: 'Loading comments…',
    resolved: 'First comment',
    metaFallback: 'Loading meta…',
    metaResolved: 'meta-ready',
  },
  error: {
    boundary: "<div class='ox-error' data-ox-error='OX-500'>",
    message: 'Atlas render failure: the data module exploded',
  },
  streamingError: { message: 'Atlas comments offline' },
  notFound: { marker: 'data-ox-not-found', status: '404' },
  head: {
    title: '<title>Head Test · Atlas</title>',
    description: 'content="Head route description"',
    pageExtracted: 'content="page-head-extracted"',
    layout: '<meta name="viewport" content="width=device-width, initial-scale=1">',
  },
  islands: {
    open: "<ox-island data-ox-island='Counter'",
    chunk: "data-ox-chunk='/islands/counter.ts'",
    strategy: "data-ox-strategy='load'",
    props: { initialCount: 3 },
    ssr: '>3</span>',
  },
  actions: {
    form: '<form method="POST" action="/actions"',
    applied: 'value="Ada"',
    result: 'Hello Ada',
    pending: '<!--form-result-pending-->',
  },
  cached: { meta: '<meta name="ox-cache" content="tags=atlas:posts;life=60s">' },
  layout: {
    doctype: '<!doctype html>',
    charset: '<meta charset="utf-8">',
    viewport: '<meta name="viewport" content="width=device-width, initial-scale=1">',
  },
}
