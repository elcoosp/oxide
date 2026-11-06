// §7.1 / §9.2 — `resolveDeferredPlaceholders`: the SSR-side half of the
// deferred-boundary protocol.
//
// A loader returns `{ __deferred: id }` placeholders; before any component
// tree is built (real SSR *and* every adapter's IR renderer) the raw data is
// walked once so components receive values, not placeholders. Every branch
// of the walker is a regression risk for a specific render path:
//
//   - arrays / plain objects   (the common loader shapes)
//   - Map / Set                (ODF round-trip types — Date/Map/Set/BigInt)
//   - missing ids              (a promise that rejected before wiring)
//   - non-placeholder objects  (`{ __deferred: 'x' }` is NOT a marker)
//   - depth cap + cycles       (hostile / recursive loader data)
//   - `__proto__`/`constructor` keys (prototype-pollution guard)
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { resolveDeferredPlaceholders } from '../src/server/deferred.ts'

test('deferred: replaces a top-level placeholder with the resolved value', () => {
  const out = resolveDeferredPlaceholders({ __deferred: 1 }, { '1': 'resolved' })
  assert.equal(out, 'resolved')
})

test('deferred: replaces placeholders nested in plain objects and arrays', () => {
  const out = resolveDeferredPlaceholders(
    { a: { __deferred: 1 }, list: [{ __deferred: 2 }, 'keep'] },
    { '1': 'one', '2': 'two' },
  ) as { a: string; list: string[] }
  assert.equal(out.a, 'one')
  assert.deepEqual(out.list, ['two', 'keep'])
})

test('deferred: resolves ids inside Map values (in place)', () => {
  const map = new Map<string, unknown>([['k', { __deferred: 5 }]])
  const out = resolveDeferredPlaceholders(map, { '5': 'five' }) as Map<string, unknown>
  assert.ok(out instanceof Map, 'Map identity is preserved')
  assert.equal(out.get('k'), 'five')
})

test('deferred: resolves ids inside Set members (copy)', () => {
  const set = new Set([{ __deferred: 6 }])
  const out = resolveDeferredPlaceholders(set, { '6': 'six' })
  assert.ok(out instanceof Set, 'Set identity is preserved')
  assert.ok([...out].includes('six'))
})

test('deferred: placeholder id 0 resolves (String(0) === "0")', () => {
  const out = resolveDeferredPlaceholders({ __deferred: 0 }, { '0': 'zero' })
  assert.equal(out, 'zero')
})

test('deferred: a missing id passes the placeholder through untouched', () => {
  // A promise that rejected before wiring must surface as the marker, not
  // disappear — the boundary then shows its fallback/error state (§7.1).
  const placeholder = { __deferred: 8 }
  const out = resolveDeferredPlaceholders(placeholder, { '9': 'other' })
  assert.deepEqual(out, placeholder)
})

test('deferred: no deferred map at all passes placeholders through', () => {
  const out = resolveDeferredPlaceholders({ x: { __deferred: 3 } }, undefined) as {
    x: { __deferred: number }
  }
  assert.deepEqual(out.x, { __deferred: 3 })
})

test('deferred: an object with extra keys is NOT a marker', () => {
  // Only the exact one-key shape `{ __deferred: <number> }` counts; anything
  // else is user data (e.g. a record that happens to carry a deferred count).
  const out = resolveDeferredPlaceholders(
    { __deferred: 1, extra: 'user data' },
    { '1': 'WRONG' },
  ) as { __deferred: number; extra: string }
  assert.equal(out.extra, 'user data')
  assert.equal(out.__deferred, 1)
})

test('deferred: a non-numeric __deferred is not a marker', () => {
  const out = resolveDeferredPlaceholders({ __deferred: 'not-a-number' }, { NaN: 'WRONG' })
  assert.deepEqual(out, { __deferred: 'not-a-number' })
})

test('deferred: primitives pass through unchanged', () => {
  assert.equal(resolveDeferredPlaceholders('plain', { '1': 'x' }), 'plain')
  assert.equal(resolveDeferredPlaceholders(42, { '42': 'x' }), 42)
  assert.equal(resolveDeferredPlaceholders(null, { '0': 'x' }), null)
  assert.equal(resolveDeferredPlaceholders(undefined, { '0': 'x' }), undefined)
})

test('deferred: cyclic references do not hang the walk', () => {
  const a: Record<string, unknown> = { name: 'a' }
  a.self = a
  a.child = { __deferred: 1 }
  const out = resolveDeferredPlaceholders(a, { '1': 'one' }) as Record<string, unknown>
  assert.equal(out.name, 'a')
  assert.equal(out.child, 'one')
  assert.equal(out.self, out, 'the cycle resolves to the same walked object')
})

test('deferred: placeholders deeper than the 512 cap are left alone', () => {
  // Build a 600-deep chain with a placeholder at the bottom, plus a shallow
  // control — the cap must stop the walk without touching shallow data.
  const deep: Record<string, unknown> = { __deferred: 1 }
  for (let i = 0; i < 600; i++) {
    deep.nest = { __deferred: 1 }
    // keep re-wrapping so the placeholder sits at depth 600+
  }
  let deepValue: unknown = { __deferred: 1 }
  for (let i = 0; i < 600; i++) deepValue = { nest: deepValue }
  let shallowValue: unknown = { __deferred: 1 }
  for (let i = 0; i < 10; i++) shallowValue = { nest: shallowValue }

  const out = resolveDeferredPlaceholders(
    { deep: deepValue, shallow: shallowValue },
    { '1': 'one' },
  ) as { deep: unknown; shallow: unknown }

  let probe: { nest?: unknown } | undefined = out.deep as { nest?: unknown }
  for (let i = 0; probe && i < 600; i++) probe = probe.nest as { nest?: unknown } | undefined
  assert.deepEqual(probe, { __deferred: 1 }, 'beyond the cap the marker is untouched')

  let shallow: unknown = out.shallow as { nest?: unknown }
  for (let i = 0; i < 10; i++) shallow = (shallow as { nest?: unknown }).nest
  assert.equal(shallow, 'one', 'within the cap the placeholder resolves')
})

test('deferred: __proto__ and constructor keys are skipped, not walked', () => {
  // Prototype-pollution guard: a hostile loader payload must not have its
  // `__proto__`/`constructor` branches re-assigned by the walker.
  const hostile: Record<string, unknown> = JSON.parse(
    '{"__deferred": 9, "__proto__": {"__deferred": 1}, "constructor": {"__deferred": 2}}',
  ) as Record<string, unknown>
  const out = resolveDeferredPlaceholders(hostile, {
    '9': 'outer',
    '1': 'POLLUTED',
    '2': 'POLLUTED',
  }) as Record<string, unknown>
  assert.equal(out.__deferred, 9, 'plain data keys are walked, numeric values pass through')
  const proto = Object.getOwnPropertyDescriptor(out, '__proto__')
  assert.ok(proto, 'own __proto__ data key survives untouched')
  // The skipped keys are NOT walked — their nested placeholder (would map to
  // 'POLLUTED') stays the raw marker object.
  assert.deepEqual(proto?.value, { __deferred: 1 })
  assert.deepEqual(out.constructor, { __deferred: 2 })
})
