// V2-E01 phase 4: the client half of state-preserving HMR. The server's
// `preserve` verdict (`keep` / `clear`) drives the snapshot → apply →
// restore cycle around a boundary update, and the fallback-reload stash
// carries the snapshot across a `location.reload()` through sessionStorage.
// Rust side: `crates/dev/src/state.rs` (+ the wire pins in
// `crates/dev/tests/state_tdd.rs`).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  HMR_STATE_STASH_KEY,
  applyHmrMessage,
  restoreFormState,
  restoreStashedFormState,
  scheduleStashRestore,
  snapshotFormState,
  stashFormStateToSession,
} from '../src/client/hmr.ts'
import type { FormStateStash } from '../src/client/hmr.ts'

interface FakeFieldEl {
  tagName: string
  attrs: Record<string, string>
  value: string
  focused: boolean
  getAttribute(k: string): string | null
  setAttribute(k: string, v: string): void
  focus(): void
}

function fakeField(tag: string, attrs: Record<string, string>, value = ''): FakeFieldEl {
  return {
    tagName: tag,
    attrs,
    value,
    focused: false,
    getAttribute(k: string) {
      return attrs[k] ?? null
    },
    setAttribute(k: string, v: string) {
      attrs[k] = v
    },
    focus() {
      this.focused = true
    },
  }
}

function fakeStateDoc(fields: FakeFieldEl[], focusedIndex: number | null = null) {
  const doc = {
    body: { appendChild() {} },
    activeElement: focusedIndex === null ? undefined : fields[focusedIndex],
    querySelectorAll(selector: string) {
      // Only the form-field selector reaches this fake in these tests.
      assert.ok(selector.includes('input'), `unexpected selector: ${selector}`)
      return fields
    },
    createElement: () => ({ style: { cssText: '' }, appendChild() {}, remove() {} }),
    getElementById: () => null,
    dispatchEvent() {},
  }
  return doc
}

/** A storage double that satisfies the `Storage` surface the module reads. */
function fakeStorage() {
  const map = new Map<string, string>()
  return {
    setItem: (k: string, v: string) => void map.set(k, String(v)),
    getItem: (k: string) => (map.has(k) ? (map.get(k) as string) : null),
    removeItem: (k: string) => void map.delete(k),
    size: () => map.size,
  }
}

test('a keep verdict restores form state after the runtime swap', async () => {
  const field = fakeField('input', { name: 'email' }, 'typed-before-swap')
  const doc = fakeStateDoc([field])
  const savedDoc = (globalThis as Record<string, unknown>).document
  ;(globalThis as Record<string, unknown>).document = doc
  ;(globalThis as Record<string, unknown>).__oxide_hmr__ = {
    // The "swap" re-renders the field empty — the accident the substrate
    // exists to undo.
    reloadModule: () => {
      field.value = 're-rendered-empty'
      return Promise.resolve()
    },
  }
  let restored = 0
  try {
    const handled = applyHmrMessage(
      {
        type: 'update',
        preserve: { policy: 'keep' },
        updates: [{ kind: 'js', path: 'src/comp/a.tsx', acceptedPath: '/src/comp/a.tsx' }],
      },
      { onStateRestore: () => void restored++ },
    )
    assert.equal(handled, true)
    // The restore is gated on the async apply settling.
    await new Promise((resolve) => setTimeout(resolve, 0))
    assert.equal(field.value, 'typed-before-swap', 'the input value survives the swap')
    assert.equal(restored, 1, 'onStateRestore fired once')
  } finally {
    if (savedDoc === undefined) delete (globalThis as Record<string, unknown>).document
    else (globalThis as Record<string, unknown>).document = savedDoc
    delete (globalThis as Record<string, unknown>).__oxide_hmr__
  }
})

test('a clear verdict (and an absent field) keeps the pre-substrate behavior', async () => {
  for (const preserve of [{ policy: 'clear', reason: 'src/a.tsx is not imported' }, undefined]) {
    const field = fakeField('input', { name: 'email' }, 'typed-before-swap')
    const doc = fakeStateDoc([field])
    const savedDoc = (globalThis as Record<string, unknown>).document
    ;(globalThis as Record<string, unknown>).document = doc
    ;(globalThis as Record<string, unknown>).__oxide_hmr__ = {
      reloadModule: () => {
        field.value = 're-rendered-empty'
        return Promise.resolve()
      },
    }
    let restored = 0
    try {
      applyHmrMessage(
        {
          type: 'update',
          ...(preserve === undefined ? {} : { preserve }),
          updates: [{ kind: 'js', path: 'src/comp/a.tsx', acceptedPath: '/src/comp/a.tsx' }],
        },
        { onStateRestore: () => void restored++ },
      )
      await new Promise((resolve) => setTimeout(resolve, 0))
      assert.equal(field.value, 're-rendered-empty', 'no snapshot: the swap result stands')
      assert.equal(restored, 0, 'no restore callback without a keep verdict')
    } finally {
      if (savedDoc === undefined) delete (globalThis as Record<string, unknown>).document
      else (globalThis as Record<string, unknown>).document = savedDoc
      delete (globalThis as Record<string, unknown>).__oxide_hmr__
    }
  }
})

test('a keep update that falls back to reload stashes to sessionStorage first', () => {
  const field = fakeField('textarea', { id: 'draft' }, 'half-written')
  const doc = fakeStateDoc([field])
  const savedDoc = (globalThis as Record<string, unknown>).document
  const storage = fakeStorage()
  const savedStorage = (globalThis as Record<string, unknown>).sessionStorage
  ;(globalThis as Record<string, unknown>).document = doc
  ;(globalThis as Record<string, unknown>).sessionStorage = storage
  let reloaded = 0
  try {
    applyHmrMessage(
      {
        type: 'update',
        preserve: { policy: 'keep' },
        updates: [{ kind: 'js', path: 'src/lib/util.ts', acceptedPath: '/src/lib/util.ts' }],
      },
      { reload: () => void reloaded++ },
    )
    assert.equal(reloaded, 1, 'the fallback reload still fires exactly once')
    const raw = storage.getItem(HMR_STATE_STASH_KEY)
    assert.ok(raw, 'the stash was written before the reload')
    const stash = JSON.parse(raw as string) as FormStateStash
    assert.equal(stash.fields.length, 1)
    assert.equal(stash.fields[0]?.key, 'id:draft')
    assert.equal(stash.fields[0]?.value, 'half-written')
  } finally {
    if (savedDoc === undefined) delete (globalThis as Record<string, unknown>).document
    else (globalThis as Record<string, unknown>).document = savedDoc
    if (savedStorage === undefined) delete (globalThis as Record<string, unknown>).sessionStorage
    else (globalThis as Record<string, unknown>).sessionStorage = savedStorage
  }
})

test('a full-reload frame never stashes — the server said clear', () => {
  const field = fakeField('input', { name: 'q' }, 'typed')
  const doc = fakeStateDoc([field])
  const savedDoc = (globalThis as Record<string, unknown>).document
  const storage = fakeStorage()
  const savedStorage = (globalThis as Record<string, unknown>).sessionStorage
  ;(globalThis as Record<string, unknown>).document = doc
  ;(globalThis as Record<string, unknown>).sessionStorage = storage
  try {
    applyHmrMessage({ type: 'full-reload', reason: 'config' }, { reload: () => {} })
    assert.equal(storage.size(), 0, 'no stash for a full-reload frame')
  } finally {
    if (savedDoc === undefined) delete (globalThis as Record<string, unknown>).document
    else (globalThis as Record<string, unknown>).document = savedDoc
    if (savedStorage === undefined) delete (globalThis as Record<string, unknown>).sessionStorage
    else (globalThis as Record<string, unknown>).sessionStorage = savedStorage
  }
})

test('snapshot and restore round-trip fields, tags and focus', () => {
  const a = fakeField('input', { name: 'q' }, 'search terms')
  const b = fakeField('textarea', {}, 'body draft')
  const c = fakeField('select', { id: 'plan' }, 'pro')
  const doc = fakeStateDoc([a, b, c], 2)
  const stash = snapshotFormState(doc)
  assert.equal(stash.fields.length, 3)
  assert.deepEqual(
    stash.fields.map((f) => f.key),
    ['name:q', 'nth:1', 'id:plan'],
    'keys prefer name, then id, then the ordinal path',
  )
  // The "re-render" wipes everything; the restore puts it back.
  a.value = ''
  b.value = ''
  c.value = ''
  restoreFormState(doc, stash)
  assert.equal(a.value, 'search terms')
  assert.equal(b.value, 'body draft')
  assert.equal(c.value, 'pro')
  assert.equal(c.focused, true, 'the focused field is re-focused')
})

test('a mismatched tag never receives another field’s value', () => {
  const original = fakeField('input', { name: 'x' }, 'typed')
  const doc = fakeStateDoc([original])
  const stash = snapshotFormState(doc)
  // The re-render replaced the input with a select under the same name.
  const swapped = fakeField('select', { name: 'x' }, '')
  const doc2 = fakeStateDoc([swapped])
  restoreFormState(doc2, stash)
  assert.equal(swapped.value, '', 'the select is untouched')
})

test('restoreStashedFormState applies a fresh stash and clears the key', () => {
  const field = fakeField('input', { name: 'email' }, 'typed')
  const doc = fakeStateDoc([field])
  const storage = fakeStorage()
  assert.equal(stashFormStateToSession(doc, storage), true, 'stash written')
  field.value = ''
  assert.equal(restoreStashedFormState(doc, storage), true, 'stash applied')
  assert.equal(field.value, 'typed', 'the value came back')
  assert.equal(storage.getItem(HMR_STATE_STASH_KEY), null, 'the key is cleared after restore')
  assert.equal(restoreStashedFormState(doc, storage), false, 'a second pass is a no-op')
})

test('a stale stash (older than the TTL) is dropped, not applied', () => {
  const field = fakeField('input', { name: 'email' }, '')
  const doc = fakeStateDoc([field])
  const storage = fakeStorage()
  storage.setItem(
    HMR_STATE_STASH_KEY,
    JSON.stringify({ at: Date.now() - 60_000, fields: [{ key: 'name:email', value: 'ancient', tag: 'input' }], focus: null }),
  )
  assert.equal(restoreStashedFormState(doc, storage), false, 'stale stash refused')
  assert.equal(field.value, '', 'nothing applied')
  assert.equal(storage.getItem(HMR_STATE_STASH_KEY), null, 'the stale stash is cleared')
})

test('scheduleStashRestore applies the stash without throwing in Node', async () => {
  const field = fakeField('input', { name: 'email' }, 'typed')
  const doc = fakeStateDoc([field])
  const savedDoc = (globalThis as Record<string, unknown>).document
  const storage = fakeStorage()
  const savedStorage = (globalThis as Record<string, unknown>).sessionStorage
  ;(globalThis as Record<string, unknown>).document = doc
  ;(globalThis as Record<string, unknown>).sessionStorage = storage
  stashFormStateToSession(doc, storage)
  field.value = ''
  try {
    scheduleStashRestore(doc)
    await new Promise((resolve) => setTimeout(resolve, 60))
    assert.equal(field.value, 'typed', 'the scheduled pass restored the stash')
  } finally {
    if (savedDoc === undefined) delete (globalThis as Record<string, unknown>).document
    else (globalThis as Record<string, unknown>).document = savedDoc
    if (savedStorage === undefined) delete (globalThis as Record<string, unknown>).sessionStorage
    else (globalThis as Record<string, unknown>).sessionStorage = savedStorage
  }
})
