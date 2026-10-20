// Client message catalog loader (§16-D).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { loadMessages, t, _resetMessagesForTest } from '../src/client/messages.ts'

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
