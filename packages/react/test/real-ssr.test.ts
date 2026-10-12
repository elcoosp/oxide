// @oxide/react real React SSR tests (spec §7).
//
// These run ONLY when `react` and `react-dom/server` are resolvable — they
// are optional peer deps of this adapter. When they are absent every test in
// this file is skipped; the IR-based tests in adapter.test.ts remain the
// neutral-path coverage and always run.
//
// The tests import React via the module under test's dynamic loader so they
// exercise the same code path the pipeline uses at runtime.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  isReactAvailable,
  getReact,
  getReactDomServer,
  renderReactToString,
} from '../src/real-ssr.ts'

const reactAvailable = await isReactAvailable()
const skip = !reactAvailable

test('real SSR: react + react-dom/server are resolvable', { skip }, async () => {
  assert.equal(await isReactAvailable(), true)
  const React = await getReact()
  const server = await getReactDomServer()
  assert.ok(React, 'react module loaded')
  assert.equal(typeof React.createElement, 'function')
  assert.ok(server, 'react-dom/server module loaded')
  assert.equal(typeof server.renderToString, 'function')
})

test('real SSR: renders a simple component to HTML', { skip }, async () => {
  const React = (await getReact())!
  const App = (props: Record<string, unknown>) =>
    React.createElement('h1', { className: 'greeting' }, `Hello ${String(props.name ?? 'world')}`)
  const html = await renderReactToString(App, { name: 'React' })
  assert.equal(html, '<h1 class="greeting">Hello React</h1>')
})

test('real SSR: escapes text content (XSS safety parity with the IR path)', { skip }, async () => {
  const React = (await getReact())!
  const App = () => React.createElement('p', null, '<script>alert(1)</script>')
  const html = await renderReactToString(App, {})
  assert.ok(html.includes('&lt;script&gt;'), 'literal script tag escaped')
  assert.ok(!html.includes('<script>'), 'no raw script element emitted')
})

test('real SSR: hooks execute during render (useState initial value)', { skip }, async () => {
  const React = (await getReact())!
  const { useState } = (await import('react')) as unknown as {
    useState: <T>(initial: T) => [T, (next: T) => void]
  }
  const Counter = () => {
    const [n] = useState(7)
    return React.createElement('span', { className: 'count' }, String(n))
  }
  const html = await renderReactToString(
    Counter as unknown as (props: Record<string, unknown>) => unknown,
    {},
  )
  assert.ok(html.includes('>7</span>'), `expected count in ${html}`)
})

test('real SSR: deterministic across identical renders', { skip }, async () => {
  const React = (await getReact())!
  const App = () => React.createElement('div', null, 'stable')
  const a = await renderReactToString(App, {})
  const b = await renderReactToString(App, {})
  assert.equal(a, b)
})

test('real SSR: components receive props verbatim', { skip }, async () => {
  const React = (await getReact())!
  let seen: Record<string, unknown> | undefined
  const App = (props: Record<string, unknown>) => {
    seen = props
    return React.createElement('span', null, String(props.value))
  }
  await renderReactToString(App, { value: 42, extra: 'x' })
  assert.ok(seen)
  assert.equal(seen!.value, 42)
  assert.equal(seen!.extra, 'x')
})
