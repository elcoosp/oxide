// V2-E04 — the Trusted Types client half pins. Written FIRST (TDD red)
// against the missing `packages/core/src/client/tt.ts` module and the
// swapper's policy routing.
//
// The contract (spec §V2-E04):
// - The core boot defines the policy ONCE: `createPolicy('oxide', …)` —
//   the name the pinned CSP directive emits (`tt_directives`:
//   `trusted-types oxide default`) — with a passthrough fallback when the
//   platform has no Trusted Types or the name is taken (the policy handle
//   is not retrievable, so a collision degrades to the passthrough object
//   rather than throwing at import time).
// - Every core `innerHTML` ASSIGNMENT routes through the policy
//   (`tt.createHTML`); reads are exempt (Trusted Types governs sinks).
//   The lint test below enforces it over `packages/core/src`.
// - The inline deferred swapper (the byte-pinned `SWAPPER_SRC` mirror of
//   `render::SWAPPER_SRC`) cannot import the module — it creates the
//   page's `'default'` policy (the second name the CSP directive allows),
//   memoized on `window.__oxDP`, and routes its template copy through it.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { tt } from '../src/client/tt.ts'
import { SWAPPER_SRC } from '../src/server/boundary.ts'

const HERE = dirname(fileURLToPath(import.meta.url))
const CORE_SRC = join(HERE, '..', 'src')

// ------------------------------------------------------ the policy

test('tt: createHTML passes content through unchanged', () => {
  assert.equal(tt.createHTML('<b>x</b>'), '<b>x</b>')
  assert.equal(tt.createHTML(''), '')
})

test('tt: the policy registers once on globalThis.__oxide_tt', () => {
  const w = globalThis as { __oxide_tt?: typeof tt }
  assert.equal(w.__oxide_tt, tt)
})

test('tt: a Trusted Types platform routes createHTML through the oxide policy', async () => {
  const created: Array<{ name: string; rules: unknown }> = []
  const fakePolicy = { createHTML: (s: string) => `[tt]${s}` }
  const g = globalThis as unknown as { trustedTypes?: unknown; __oxide_tt?: unknown }
  const saved = g.trustedTypes
  const savedTT = g.__oxide_tt
  g.trustedTypes = {
    createPolicy(name: string, rules: { createHTML(s: string): string }): unknown {
      created.push({ name, rules })
      return fakePolicy
    },
  }
  try {
    // A query-string import is a DISTINCT module instance in Node's ESM
    // cache, so the module's own top-level policy creation runs against
    // the fake platform — the real code path, not a reimplementation.
    const mod = (await import(`../src/client/tt.ts?platform=${Date.now()}`)) as {
      tt: { createHTML(s: string): string }
    }
    assert.equal(created.length, 1)
    assert.equal(created[0]?.name, 'oxide')
    assert.equal(mod.tt.createHTML('<i>y</i>'), '[tt]<i>y</i>')
  } finally {
    g.trustedTypes = saved
    g.__oxide_tt = savedTT
  }
})

test('tt: a name collision degrades to the passthrough, never throws', async () => {
  const g = globalThis as unknown as { trustedTypes?: unknown; __oxide_tt?: unknown }
  const saved = g.trustedTypes
  const savedTT = g.__oxide_tt
  g.trustedTypes = {
    createPolicy(): unknown {
      throw new TypeError('Policy oxide already exists')
    },
  }
  try {
    const mod = (await import(`../src/client/tt.ts?collision=${Date.now()}`)) as {
      tt: { createHTML(s: string): string }
    }
    assert.equal(mod.tt.createHTML('<i>z</i>'), '<i>z</i>')
  } finally {
    g.trustedTypes = saved
    g.__oxide_tt = savedTT
  }
})

// ------------------------------------------------------ the swapper

test('swapper: the deferred swap routes innerHTML through the realm policy handle', () => {
  assert.ok(
    SWAPPER_SRC.includes('W=window.__oxide_tt;d.innerHTML=W?W.createHTML(t.innerHTML):t.innerHTML'),
    'the template copy goes through the boot-registered oxide policy when present, raw otherwise',
  )
})

test('swapper: byte-identical to the Rust render crate mirror', () => {
  const rust = readFileSync(join(HERE, '..', '..', '..', 'crates', 'render', 'src', 'lib.rs'), 'utf8')
  const m = /pub const SWAPPER_SRC: &str = r#"(.*)"#;/.exec(rust)
  assert.ok(m?.[1], 'SWAPPER_SRC found in crates/render/src/lib.rs')
  assert.equal(SWAPPER_SRC, m[1], 'boundary.ts mirror equals render::SWAPPER_SRC byte for byte')
})

// ------------------------------------------------------ the lint

test('lint: every innerHTML assignment in packages/core/src routes through a TT policy', () => {
  const offenders: string[] = []
  const files = collectFiles(CORE_SRC)
  const SINK = /\.innerHTML\s*=/g
  for (const f of files) {
    for (const [i, line] of readFileSync(f, 'utf8').split('\n').entries()) {
      // Reads (`x.innerHTML` without an assignment) are exempt — Trusted
      // Types governs sinks, not reads.
      if (!SINK.test(line)) continue
      SINK.lastIndex = 0
      if (!/createHTML\s*\(|__oxDP|__oxide_tt|\btt\b/.test(line)) {
        offenders.push(`${relative(f)}:${i + 1}: ${line.trim()}`)
      }
      SINK.lastIndex = 0
    }
  }
  assert.deepEqual(offenders, [])
})

// ------------------------------------------------------ helpers

function collectFiles(dir: string): string[] {
  const out: string[] = []
  const stack = [dir]
  while (stack.length > 0) {
    const cur = stack.pop() as string
    for (const e of readdirSync(cur, { withFileTypes: true })) {
      const p = join(cur, e.name)
      if (e.isDirectory()) stack.push(p)
      else if (e.name.endsWith('.ts')) out.push(p)
    }
  }
  return out
}

function relative(f: string): string {
  return f.slice(CORE_SRC.length + 1)
}
