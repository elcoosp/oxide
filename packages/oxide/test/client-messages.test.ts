// Client message catalog loader (§16-D).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { formatIcu, loadMessages, t, _resetMessagesForTest } from '../src/client/messages.ts'

/** Minimal document + fetch stubs — the module reads `document.documentElement`
 *  and `fetch`, nothing else. */
function withGlobals<T>(lang: string, fetchImpl: (url: string) => Promise<Response>, fn: () => Promise<T>): Promise<T> {
  const g = globalThis as unknown as {
    document?: { documentElement: { lang: string; getAttribute(k: string): string | null } }
    fetch?: typeof fetch
  }
  const savedDoc = g.document
  const savedFetch = g.fetch
  g.document = { documentElement: { lang, getAttribute: () => null } }
  g.fetch = ((url: string) => fetchImpl(url)) as unknown as typeof fetch
  return fn().finally(() => {
    g.document = savedDoc
    g.fetch = savedFetch
  })
}

test('loads the active locale catalog and t() substitutes placeholders', async () => {
  _resetMessagesForTest()
  await withGlobals(
    'fr',
    async (url) => {
      assert.equal(url, '/client/messages/fr.json')
      return new Response(JSON.stringify({ locale: 'fr', messages: { 'nav.home': 'Accueil', 'hello': 'Bonjour {name}' } }))
    },
    async () => {
      const msgs = await loadMessages()
      assert.equal(msgs['nav.home'], 'Accueil')
      assert.equal(t('hello', { name: 'Ada' }), 'Bonjour Ada')
      assert.equal(t('missing.key'), 'missing.key')
    },
  )
})

test('a missing catalog degrades to the key, not a crash', async () => {
  _resetMessagesForTest()
  await withGlobals(
    'de',
    async () => new Response('not found', { status: 404 }),
    async () => {
      const msgs = await loadMessages()
      assert.deepEqual({ ...msgs }, {})
      assert.equal(t('anything'), 'anything')
    },
  )
})

test('the catalog is fetched once and cached', async () => {
  _resetMessagesForTest()
  let calls = 0
  await withGlobals(
    'en',
    async () => {
      calls += 1
      return new Response(JSON.stringify({ locale: 'en', messages: { ok: 'OK' } }))
    },
    async () => {
      await loadMessages()
      await loadMessages()
      assert.equal(calls, 1, 'second load is a cache hit')
    },
  )
})

test('prefers the per-route subset when the page names a route', async () => {
  _resetMessagesForTest()
  const fetched: string[] = []
  const g = globalThis as unknown as { __oxide?: { route: string } }
  const saved = g.__oxide
  g.__oxide = { route: 'routes/blog/page' }
  await withGlobals(
    'fr',
    async (url) => {
      fetched.push(url)
      if (url.includes('/blog.json')) {
        return new Response(
          JSON.stringify({ locale: 'fr', route: 'blog', messages: { 'blog.title': 'Articles' } }),
        )
      }
      return new Response(
        JSON.stringify({ locale: 'fr', messages: { 'blog.title': 'Articles', 'nav.home': 'Accueil' } }),
      )
    },
    async () => {
      const msgs = await loadMessages()
      assert.equal(fetched[0], '/client/messages/fr/blog.json', 'subset fetched first')
      assert.equal(msgs['blog.title'], 'Articles')
    },
  )
  g.__oxide = saved
})

test('falls back to the full catalog when no subset exists', async () => {
  _resetMessagesForTest()
  const fetched: string[] = []
  const g = globalThis as unknown as { __oxide?: { route: string } }
  const saved = g.__oxide
  g.__oxide = { route: 'routes/page' }
  await withGlobals(
    'en',
    async (url) => {
      fetched.push(url)
      if (url.includes('/messages/en/')) {
        return new Response('not found', { status: 404 })
      }
      return new Response(
        JSON.stringify({ locale: 'en', messages: { 'nav.home': 'Home' } }),
      )
    },
    async () => {
      const msgs = await loadMessages()
      assert.equal(fetched.length, 2, 'subset attempted, then full catalog')
      assert.equal(msgs['nav.home'], 'Home')
    },
  )
  g.__oxide = saved
})


test('client ICU exact match uses post-offset value (== n - offset)', () => {
  // ICU spec: the =N branch matches when (n - offset) == N, not when n == N.
  // Before the fix, String(n) was used, so with count=1, offset:1 the code
  // looked for '=1' but the branch present was '=0'.
  assert.equal(
    formatIcu(
      '{count, plural, offset:1 =0 {nobody} =1 {just you} other {# others}}',
      { count: 1 },
      'en',
    ),
    'nobody',
    'count=1, offset:1 → 1-1=0 → =0 branch (nobody)',
  )
  assert.equal(
    formatIcu(
      '{count, plural, offset:1 =0 {nobody} =1 {just you} other {# others}}',
      { count: 2 },
      'en',
    ),
    'just you',
    'count=2, offset:1 → 2-1=1 → =1 branch (just you)',
  )
})

test('client ICU # inside exact branch renders the post-offset value', () => {
  // # in a =N branch should render n - offset, not literal '#' or undefined.
  assert.equal(
    formatIcu(
      '{count, plural, =0 {# items exactly} other {other}}',
      { count: 0 },
      'en',
    ),
    '0 items exactly',
    '=0 branch, no offset → # = 0',
  )
  assert.equal(
    formatIcu(
      '{count, plural, offset:1 =0 {# remaining after you} other {other}}',
      { count: 1 },
      'en',
    ),
    '0 remaining after you',
    '=0 branch, offset:1 → # = 1-1 = 0',
  )
})

test('client ICU formats plural and select groups with offsets', () => {
  assert.equal(
    formatIcu('{count, plural, =0 {none} one {# item} other {# items}}', { count: 0 }, 'en'),
    'none',
  )
  assert.equal(
    formatIcu('{count, plural, =0 {none} one {# item} other {# items}}', { count: 1 }, 'en'),
    '1 item',
  )
  assert.equal(
    formatIcu('{count, plural, =0 {none} one {# item} other {# items}}', { count: 3 }, 'en'),
    '3 items',
  )
  assert.equal(
    formatIcu(
      '{count, plural, offset:1 =0 {nobody} =1 {just you} one {you and # other} other {you and # others}}',
      { count: 3 },
      'en',
    ),
    'you and 2 others',
  )
  assert.equal(
    formatIcu('{gender, select, male {He} female {She} other {They}}', { gender: 'female' }, 'en'),
    'She',
  )
})

test('client ICU uses the locale plural rules and handles nested messages', () => {
  assert.equal(
    formatIcu('{n, plural, one {# файл} few {# файла} many {# файлов} other {# файла}}', { n: 2 }, 'ru'),
    '2 файла',
  )
  assert.equal(
    formatIcu('{n, plural, one {# файл} few {# файла} many {# файлов} other {# файла}}', { n: 5 }, 'ru'),
    '5 файлов',
  )
  assert.equal(
    formatIcu(
      '{gender, select, female {{count, plural, one {She has # item} other {She has # items}}} other {They have items}}',
      { gender: 'female', count: 2 },
      'en',
    ),
    'She has 2 items',
  )
})

test('client ICU formats numbers, ordinals and preserves ordinary apostrophes', () => {
  assert.equal(formatIcu('{n, number}', { n: 1234.5 }, 'en'), '1,234.5')
  assert.equal(
    formatIcu('{n, selectordinal, one {#st} two {#nd} few {#rd} other {#th}}', { n: 2 }, 'en'),
    '2nd',
  )
  assert.equal(formatIcu("l'utilisateur {name}", { name: 'Ada' }, 'fr'), "l'utilisateur Ada")
  assert.equal(formatIcu('missing {value}', {}, 'en'), 'missing {value}')
})
