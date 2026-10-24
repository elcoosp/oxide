// §16.5 — client-side search index reader.
//
// The build emits `dist/client/search-index.json`; this is what a
// client-side island calls to answer a query. No network call, no WASM.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { tokenize, search, type SearchIndex } from '../src/client/search.ts'

function index(entries: SearchIndex['entries']): SearchIndex {
  return { version: 1, entries }
}

test('tokenize lowercases and drops single-char tokens', () => {
  assert.deepEqual(tokenize('Hello, World!'), ['hello', 'world'])
  // "of" is 2 chars, so it survives the >= 2 filter — only single-char
  // tokens are dropped.
  assert.deepEqual(tokenize('a of the'), ['of', 'the'])
  assert.deepEqual(tokenize(''), [])
  assert.deepEqual(tokenize('camelCase'), ['camelcase'])
})

test('empty query returns no results', () => {
  const idx = index([
    { route: 'routes/index', url: '/', title: 'Home' },
    { route: 'routes/about', url: '/about', title: 'About' },
  ])
  assert.deepEqual(search(idx, ''), [])
  assert.deepEqual(search(idx, '   '), [])
  assert.deepEqual(search(idx, 'a'), [])
})

test('scoring prefers title over heading over excerpt', () => {
  const idx = index([
    { route: 'routes/a', url: '/a', title: 'Introduction', headings: [], excerpt: '' },
    { route: 'routes/b', url: '/b', title: 'Other', headings: ['Introduction'], excerpt: '' },
    { route: 'routes/c', url: '/c', title: 'Other', headings: [], excerpt: 'An introduction to things' },
  ])
  const results = search(idx, 'introduction')
  assert.equal(results.length, 3)
  // Title match scores higher than heading, heading higher than excerpt.
  assert.equal(results[0].route, 'routes/a', 'title first')
  assert.equal(results[1].route, 'routes/b', 'heading second')
  assert.equal(results[2].route, 'routes/c', 'excerpt third')
  assert.equal(results[0].score, 3)
  assert.equal(results[1].score, 2)
  assert.equal(results[2].score, 1)
})

test('multi-token queries are AND-scored across fields', () => {
  const idx = index([
    {
      route: 'routes/getting-started',
      url: '/docs/getting-started',
      title: 'Getting Started',
      headings: ['Installation', 'First project'],
      excerpt: 'A quick guide to running your first project.',
    },
  ])
  // "getting started" — both tokens hit the title.
  const a = search(idx, 'getting started')
  assert.equal(a.length, 1)
  assert.equal(a[0].score, 6, 'two tokens × 3 for title')

  // "getting install" — getting (title, 3) + install (heading, 2).
  const b = search(idx, 'getting install')
  assert.equal(b.length, 1)
  assert.equal(b[0].score, 5)
})

test('results are ordered by score, then route id (byte-stable)', () => {
  const idx = index([
    { route: 'routes/zeta', url: '/z', title: 'Common' },
    { route: 'routes/alpha', url: '/a', title: 'Common' },
  ])
  const results = search(idx, 'common')
  assert.deepEqual(
    results.map((r) => r.route),
    ['routes/alpha', 'routes/zeta'],
    'equal score → alphabetical route id',
  )
})

test('limit caps the result count', () => {
  const entries = Array.from({ length: 25 }, (_, i) => ({
    route: `routes/page-${String(i).padStart(2, '0')}`,
    url: `/page-${i}`,
    title: 'Common',
  }))
  const results = search(index(entries), 'common', { limit: 5 })
  assert.equal(results.length, 5)
})

test('prefix matching finds longer index tokens', () => {
  const idx = index([
    {
      route: 'routes/getting-started',
      url: '/docs/getting-started',
      title: 'Getting Started',
      headings: ['Installation', 'First project'],
    },
  ])
  // 'install' prefix-matches 'installation' in a heading → +2.
  const r = search(idx, 'install')
  assert.equal(r.length, 1, 'partial word finds the page')
  assert.equal(r[0].score, 2)
  // 'sta' prefix-matches 'started' in the title → +3.
  const s = search(idx, 'sta')
  assert.equal(s.length, 1)
  assert.equal(s[0].score, 3)
  // Substring (not prefix) does not match: 'stal' is not a prefix of
  // 'installation'.
  const t = search(idx, 'stal')
  assert.deepEqual(t, [], 'substring does not match')
})
