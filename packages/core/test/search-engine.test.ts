// V2-F09 — the client matcher pins (the built-in engine's client half).
// Written FIRST (TDD red) against the missing
// `packages/core/src/client/searchEngine.ts` module.
//
// The contract (the spec's verbatim shapes, mirrored from the landed
// server halves — `crates/search/src/static_index.rs` `StaticMatcher` and
// `crates/server/src/search_endpoint.rs`):
// - `search(q, opts)` answers from the server's `/_oxide/search` endpoint
//   when the boot flag says the engine is serving
//   (`window.__oxide.search === 'server'`), and otherwise lazily loads the
//   static export (`dist/search/static.json` → `/search/static.json`) and
//   runs the SAME matcher the server's degraded StaticMatcher runs: per
//   token, a postings hit scores `bodyBoost`, a title word that STARTS
//   WITH the token scores `max(titleBoost, 3)`, an excerpt word that
//   starts with it scores `bodyBoost`; results sort by score desc with
//   the doc order as the byte-stable tiebreak. The degraded shape is a
//   parity shape, not a different algorithm.
// - `?limit=` rides the endpoint's wire rules: default 10, cap 100,
//   `?tag=`/`?lang=` empty-string = absent. The tag filter applies AFTER
//   the limit take — the landed server shape, mirrored verbatim (a quirk
//   pinned, not silently "fixed" — the two implementations must stay
//   byte-equal on the same corpus).
// - An empty/whitespace query is refused LOCALLY (the server would 400
//   `SRC-104`): no fetch, an empty result — a UI's empty input box is a
//   normal state, not an error.
// - The static index loads LAZILY and ONCE: the first search fetches,
//   every later search reuses it (a static site's matcher must not re-
//   download the corpus per keystroke).

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { search, tokenize, type SearchOptions, type StaticIndex } from '../src/client/searchEngine.ts'

// ------------------------------------------------------ the fixture index

// The corpus the parity shapes run over: three docs whose title/excerpt
// fields discriminate the scoring arms, with the postings the build's
// export carries (full terms + the 2-to-4 char prefixes).
const index: StaticIndex = {
  docs: [
    {
      id: 'docs/install',
      url: '/docs/install',
      title: 'Installation guide',
      excerpt: 'Install oxide with pnpm, then configure the routes.',
      tags: ['docs'],
    },
    {
      id: 'docs/indexer',
      url: '/docs/indexer',
      title: 'The search indexer',
      excerpt: 'The indexer walks content collections and builds the postings.',
      tags: ['docs', 'search'],
    },
    {
      id: 'blog/launch',
      url: '/blog/launch',
      title: 'Launch week',
      excerpt: 'Shipping oxide 1.0 with search built in.',
      tags: ['blog'],
    },
  ],
  postings: {
    install: [0],
    installation: [0],
    ins: [0],
    guide: [0],
    pnpm: [0],
    routes: [0],
    the: [1, 2],
    search: [1],
    indexer: [1],
    index: [1],
    ind: [1],
    inde: [1],
    walks: [1],
    content: [1],
    collections: [1],
    builds: [1],
    postings: [1],
    launch: [2],
    laun: [2],
    week: [2],
    shipping: [2],
    oxide: [0, 1, 2],
    built: [2],
  },
}

function opts(extra: Partial<SearchOptions> = {}): Partial<SearchOptions> {
  return { mode: 'static', staticIndex: index, ...extra }
}

// ------------------------------------------------------------ the pins

test('tokenize lowercases and splits on any non-alphanumeric run (the server matcher rules)', () => {
  assert.deepEqual(tokenize('Install Oxide, then configure!'), [
    'install',
    'oxide',
    'then',
    'configure',
  ])
  assert.deepEqual(tokenize('  '), [])
})

test('static mode scores a title match above a body-only match', async () => {
  const res = await search('indexer', opts())
  assert.equal(res.hits[0]?.id, 'docs/indexer')
  // Per token "indexer": the postings hit (+1), the title word
  // "indexer" starts with it (+3) and the excerpt word does too (+1).
  assert.equal(res.hits[0]?.score, 5)
})

test('a 2-4 char prefix token matches through the export prefix postings', async () => {
  const res = await search('ind', opts())
  assert.equal(res.hits[0]?.id, 'docs/indexer')
  // The prefix postings hit (+1), the title prefix arm (+3), the
  // excerpt prefix arm (+1).
  assert.equal(res.hits[0]?.score, 5)
})

test('results sort by score desc with the doc order as the tiebreak', async () => {
  // "the": doc 1 = postings(+1) + title(+3) = 4; docs 0 and 2 catch the
  // prefix arms ("then" / no — doc 0's excerpt "then" starts with "the",
  // doc 2's postings) at 1 each — doc order breaks the tie.
  const res = await search('the', opts())
  assert.deepEqual(res.hits.map((h) => h.id), [
    'docs/indexer',
    'docs/install',
    'blog/launch',
  ])
  assert.equal(res.total, 3)
})

test('multi-token queries AND-score across fields (each token contributes)', async () => {
  const res = await search('install guide', opts())
  assert.equal(res.hits[0]?.id, 'docs/install')
  // "install": postings(+1) + title prefix "installation"(+3) + excerpt
  // "Install"(+1) = 5; "guide": postings(+1) + title(+3) = 4 → 9.
  assert.equal(res.hits[0]?.score, 9)
})

test('limit caps the hits while total reflects everything scored', async () => {
  const res = await search('oxide', opts({ limit: 2 }))
  assert.equal(res.hits.length, 2)
  assert.equal(res.total, 3)
})

test('the tag filter applies after the limit take (the landed server shape, pinned)', async () => {
  // "oxide" scores docs 0 (2) and 2 (2) above doc 1 (1): the limit-2
  // take is [docs/install, blog/launch], and the post-take blog filter
  // keeps exactly the blog doc — the parity shape (the server's
  // StaticMatcher filters after the take, so a narrower tag can return
  // fewer hits than the limit allowed).
  const res = await search('oxide', opts({ limit: 2, tag: 'blog' }))
  assert.deepEqual(res.hits.map((h) => h.id), ['blog/launch'])
  assert.equal(res.total, 3)
  // The same query against the docs tag keeps the take's non-blog doc
  // OUT (it was filtered post-take) — 1 hit at total 3.
  const res2 = await search('oxide', opts({ limit: 2, tag: 'docs' }))
  assert.deepEqual(res2.hits.map((h) => h.id), ['docs/install'])
  assert.equal(res2.total, 3)
})

test('an empty or whitespace query is refused locally: an empty result, no fetch', async () => {
  let fetches = 0
  const fetchFn = async () => {
    fetches += 1
    throw new Error('must not fetch')
  }
  for (const q of ['', '   ']) {
    const res = await search(q, opts({ fetchFn: fetchFn as typeof fetch }))
    assert.deepEqual(res, { total: 0, hits: [] })
  }
  assert.equal(fetches, 0)
})

test('server mode calls the endpoint with the query params and returns the payload', async () => {
  const calls: string[] = []
  const fetchFn = (async (input: string | URL | Request) => {
    calls.push(String(input))
    return new Response(JSON.stringify({ total: 1, hits: [{ id: 'x', url: '/x', title: 'X', snippet: 's', score: 3 }] }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch
  const res = await search('indexer', {
    mode: 'server',
    limit: 5,
    tag: 'docs',
    lang: 'en',
    fetchFn,
  })
  assert.equal(res.total, 1)
  assert.equal(res.hits[0]?.id, 'x')
  assert.equal(calls.length, 1)
  const url = new URL(calls[0], 'https://example.test')
  assert.equal(url.pathname, '/_oxide/search')
  assert.equal(url.searchParams.get('q'), 'indexer')
  assert.equal(url.searchParams.get('limit'), '5')
  assert.equal(url.searchParams.get('tag'), 'docs')
  assert.equal(url.searchParams.get('lang'), 'en')
})

test('the boot flag picks the server mode: window.__oxide.search === "server"', async () => {
  const calls: string[] = []
  const fetchFn = (async (input: string | URL | Request) => {
    calls.push(String(input))
    return new Response(JSON.stringify({ total: 0, hits: [] }), { status: 200 })
  }) as typeof fetch
  ;(globalThis as Record<string, unknown>).__oxide = { search: 'server' }
  try {
    await search('anything', { fetchFn, staticIndex: index })
    assert.equal(calls.length, 1)
    assert.ok(calls[0].includes('/_oxide/search'), calls[0])
  } finally {
    delete (globalThis as Record<string, unknown>).__oxide
  }
})

test('the static index loads lazily and exactly once across searches', async () => {
  let loads = 0
  const loader = async () => {
    loads += 1
    return index
  }
  await search('indexer', { mode: 'static', staticIndex: loader, fetchFn: (async () => { throw new Error('no endpoint in static mode') }) as typeof fetch })
  await search('install', { mode: 'static', staticIndex: loader, fetchFn: (async () => { throw new Error('no endpoint in static mode') }) as typeof fetch })
  await search('launch', { mode: 'static', staticIndex: loader, fetchFn: (async () => { throw new Error('no endpoint in static mode') }) as typeof fetch })
  assert.equal(loads, 1)
})

test('a non-200 endpoint answer throws with the status (the caller shows its own UI)', async () => {
  const fetchFn = (async () => new Response('{"error":"rate limited"}', { status: 429 })) as typeof fetch
  await assert.rejects(
    () => search('indexer', { mode: 'server', fetchFn }),
    /429/,
  )
})

test('a malformed static export throws instead of silently matching nothing', async () => {
  const fetchFn = (async () => new Response('<html>not json</html>', { status: 200 })) as typeof fetch
  await assert.rejects(
    () => search('indexer', { mode: 'static', staticUrl: '/search/static.json', fetchFn }),
    /static.json/,
  )
})
