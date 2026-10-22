// §16.4 / O2: `<html lang>` follows the resolved request locale.
//
// The three emit sites in `src/render.ts` (the page shell and the two
// error-shell arms) interpolate `input.locale` behind a
// `/^[A-Za-z0-9_-]+$/` guard, falling back to `en`. Before this, every
// shell hardcoded `lang="en"`, so the client catalog loader always fell
// back to the English bundle regardless of the server-side resolution.
//
// The guard matters as much as the interpolation: `locale` arrives from
// request content negotiation and is interpolated into an HTML attribute,
// so a hostile value must not be able to close the quote.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { adapter } from '../src/index.ts'
import type { RenderInput } from '../../oxide/src/types.ts'

const input = (overrides: Partial<RenderInput> = {}): RenderInput => ({
  route: 'routes/index',
  url: '/',
  params: {},
  data: {},
  head: { title: 'Home', meta: [], link: [], style: [], script: [] },
  ...overrides,
})

async function drain(stream: ReadableStream<Uint8Array>): Promise<string> {
  const chunks: string[] = []
  for await (const c of stream) chunks.push(new TextDecoder().decode(c))
  return chunks.join('')
}

const langOf = (html: string): string | undefined =>
  /<html lang="([^"]*)"/.exec(html)?.[1]

test('html lang: the resolved locale is emitted', async () => {
  const html = await drain(adapter.render(input({ locale: 'fr' })))
  assert.equal(langOf(html), 'fr', `lang from locale:\n${html.slice(0, 200)}`)
})

test('html lang: falls back to en with no locale', async () => {
  const html = await drain(adapter.render(input()))
  assert.equal(langOf(html), 'en', `default lang:\n${html.slice(0, 200)}`)
})

test('html lang: a region tag survives intact', async () => {
  // `en-GB`, `zh-Hant-TW` — hyphenated BCP-47 tags are within the guard.
  const html = await drain(adapter.render(input({ locale: 'zh-Hant-TW' })))
  assert.equal(langOf(html), 'zh-Hant-TW')
})

test('html lang: a hostile locale cannot break out of the attribute', async () => {
  // The guard is an allow-list, so a value with a quote, a `<`, or a
  // path separator falls back to `en` rather than reaching the markup.
  for (const hostile of ['"><script>alert(1)</script>', '../../etc/passwd', 'en us']) {
    const html = await drain(adapter.render(input({ locale: hostile })))
    assert.equal(
      langOf(html),
      'en',
      `hostile locale ${JSON.stringify(hostile)} must fall back to en:\n${html.slice(0, 200)}`,
    )
    assert.ok(!html.includes('<script>alert(1)</script>'), 'no injected script')
  }
})

test('html lang: the error shell carries it too', async () => {
  // §10.2 step 5: a render that throws produces the error shell, which has
  // its own `<html lang>` site. A missing component is the simplest throw.
  const html = await drain(
    adapter.render(input({ route: 'routes/definitely-missing', locale: 'de' })),
  )
  assert.equal(langOf(html), 'de', `error shell lang:\n${html.slice(0, 200)}`)
})
