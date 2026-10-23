// <Defer> — the Vue adapter's deferred-boundary component (§7.1).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createSSRApp, h } from 'vue'
import { renderToString } from 'vue/server-renderer'
import { Defer } from '../src/defer.ts'

test('a placeholder value renders <ox-defer data-ox-b="id"> with the fallback', async () => {
  const app = createSSRApp({
    render: () =>
      h(
        Defer,
        { value: { __deferred: 0 } },
        {
          fallback: () => h('p', { class: 'fallback' }, 'Loading…'),
          default: ({ value }: { value: unknown }) =>
            h('span', { class: 'resolved' }, String(value)),
        },
      ),
  })
  const html = await renderToString(app)
  assert.match(html, /<ox-defer[^>]*data-ox-b="0"/, html)
  assert.match(html, /<p[^>]*class="fallback"[^>]*>Loading…<\/p>/, html)
  assert.doesNotMatch(html, /class="resolved"/, html)
})

test('an already-resolved value renders children inline — no <ox-defer>', async () => {
  const app = createSSRApp({
    render: () =>
      h(
        Defer,
        { value: 'hello' },
        {
          default: ({ value }: { value: unknown }) =>
            h('span', { class: 'resolved' }, String(value)),
        },
      ),
  })
  const html = await renderToString(app)
  assert.doesNotMatch(html, /<ox-defer/, html)
  assert.match(html, /<span[^>]*class="resolved"[^>]*>hello<\/span>/, html)
})
