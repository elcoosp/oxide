// @oxide/vue real Vue SSR tests (spec §7). Skipped when vue +
// @vue/server-renderer are not resolvable.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  isVueAvailable, getVue, getVueServerRenderer, renderVueToString,
} from '../src/real-ssr.ts'

const available = await isVueAvailable()
const skip = !available

test('real SSR: vue + @vue/server-renderer are resolvable', { skip }, async () => {
  assert.equal(await isVueAvailable(), true)
  const vue = await getVue()
  const server = await getVueServerRenderer()
  assert.ok(vue)
  assert.equal(typeof vue.createSSRApp, 'function')
  assert.ok(server)
  assert.equal(typeof server.renderToString, 'function')
})

test('real SSR: renders a component tree via h()', { skip }, async () => {
  const vue = (await getVue())!
  const App = { render: () => vue.h('h1', { class: 'greeting' }, 'Hello Vue') }
  const html = await renderVueToString(App, {})
  assert.ok(html.includes('<h1 class="greeting">Hello Vue</h1>'), html)
})

test('real SSR: escapes text content (XSS safety parity)', { skip }, async () => {
  const vue = (await getVue())!
  const App = { render: () => vue.h('p', null, '<script>x</script>') }
  const html = await renderVueToString(App, {})
  assert.ok(html.includes('&lt;script&gt;'), html)
  assert.ok(!html.includes('<script>'), html)
})

test('real SSR: props flow to component (defineComponent)', { skip }, async () => {
  const vue = (await getVue())!
  const App = vue.defineComponent({
    props: { msg: { type: String, default: '' } },
    render(this: { msg: string }) { return vue.h('span', null, this.msg) },
  })
  const html = await renderVueToString(App, { msg: 'from props' })
  assert.ok(html.includes('>from props</span>'), html)
})

test('real SSR: deterministic across identical renders', { skip }, async () => {
  const vue = (await getVue())!
  const App = { render: () => vue.h('div', null, 'stable') }
  const a = await renderVueToString(App, {})
  const b = await renderVueToString(App, {})
  assert.equal(a, b)
})
