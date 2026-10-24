// §8.2 / L6(b): the layout-wrap tree assembly.
//
// The Playwright navigation spec asserts the *DOM* is right after a
// swap; this pins the *tree* the swap produces. A layout-order
// regression (Inner(Outer(Page)) instead of Outer(Inner(Page))) would
// render valid React but the wrong markup, and no browser-level test
// catches that.
//
// `nestLayouts` is the shared swap logic used by initial mount,
// hydration, and navigation update. Tested here in isolation against
// a stub `react.createElement` so it needs no framework dependency.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { nestLayouts } from '../src/real-client.ts'

interface Call { C: unknown; p: Record<string, unknown> }

function stubReact() {
  const calls: Call[] = []
  return {
    calls,
    createElement: (C: unknown, p: Record<string, unknown>) => {
      calls.push({ C, p })
      return { kind: 'element', C, p }
    },
  }
}

/** Typed indexed access with a runtime bounds check. Throws a clear error
 * (failing the test) instead of returning `T | undefined` — the honest way
 * to satisfy `noUncheckedIndexedAccess` in tests whose fixtures are known
 * non-empty. No casts, no `any`, no `!`, no `?? default`. */
function mustEl<T>(arr: readonly T[], i: number): T {
  const v = arr[i]
  if (v === undefined) throw new Error(`mustEl: index ${i} out of bounds (len=${arr.length})`)
  return v
}

test('no layouts — bare page element', () => {
  const r = stubReact()
  const Page = { name: 'Page' }
  const tree = nestLayouts(r, Page, { route: 'routes/index' }, [])
  assert.deepEqual(r.calls.length, 1, 'only the page was created')
  assert.equal(mustEl(r.calls, 0).C, Page)
  assert.deepEqual(tree, { kind: 'element', C: Page, p: { route: 'routes/index' } })
})

test('single layout — page wrapped in the layout', () => {
  const r = stubReact()
  const Page = { name: 'Page' }
  const Layout = { name: 'Layout' }
  nestLayouts(r, Page, { route: 'routes/index' }, [{ id: 'routes/layout', Component: Layout }])
  assert.equal(r.calls.length, 2)
  // Inner-most call is the page.
  assert.equal(mustEl(r.calls, 0).C, Page)
  // Outer call is the layout, with the page as `children`.
  assert.equal(mustEl(r.calls, 1).C, Layout)
  assert.equal((mustEl(r.calls, 1).p.children as any).C, Page)
})

test('two layouts — outermost FIRST in the chain ends up outermost in the tree', () => {
  const r = stubReact()
  const Page = { name: 'Page' }
  const Outer = { name: 'Outer' }
  const Inner = { name: 'Inner' }
  // layouts = [outermost, innermost] — the SSRs input.layouts convention.
  nestLayouts(r, Page, { route: 'routes/index' }, [
    { id: 'routes/layout', Component: Outer },
    { id: 'routes/nested/layout', Component: Inner },
  ])
  // Expected call order: Page, then Inner (as child), then Outer (as child).
  assert.equal(r.calls.length, 3)
  assert.equal(mustEl(r.calls, 0).C, Page, 'page created first (innermost)')
  assert.equal(mustEl(r.calls, 1).C, Inner, 'inner layout wraps the page next')
  assert.equal(mustEl(r.calls, 2).C, Outer, 'outer layout wraps everything last (outermost)')
  // Verify the tree shape: Outer.children = Inner, Inner.children = Page.
  assert.equal((mustEl(r.calls, 2).p.children as any).C, Inner)
  assert.equal(((mustEl(r.calls, 2).p.children as any).p.children as any).C, Page)
})

test('three layouts — order is preserved on every level', () => {
  const r = stubReact()
  const Page = { name: 'Page' }
  const A = { name: 'A' }
  const B = { name: 'B' }
  const C = { name: 'C' }
  nestLayouts(r, Page, {}, [
    { id: 'a', Component: A },
    { id: 'b', Component: B },
    { id: 'c', Component: C },
  ])
  // Creation order (innermost to outermost): Page, C, B, A.
  assert.deepEqual(r.calls.map((x) => (x.C as any).name), ['Page', 'C', 'B', 'A'])
})

test('page props reach the innermost element unchanged', () => {
  const r = stubReact()
  const Page = { name: 'Page' }
  const Layout = { name: 'Layout' }
  const props = { route: 'routes/blog/[slug]', params: { slug: 'hi' }, data: { title: 'x' } }
  nestLayouts(r, Page, props, [{ id: 'routes/layout', Component: Layout }])
  assert.deepEqual(mustEl(r.calls, 0).p, props)
})
