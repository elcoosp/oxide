// §7.1 wire shapes — `packages/svelte/src/protocol.ts` against the Rust
// render crate (the single source of truth for every streamed byte shape).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { SWAPPER_SRC, boundaryContent, islandElement } from '../src/protocol.ts'

function rustSwapperSrc(): string {
  const src = readFileSync(
    fileURLToPath(new URL('../../../crates/render/src/lib.rs', import.meta.url)),
    'utf8',
  )
  const m = /pub const SWAPPER_SRC: &str = r#"(.*)"#;/.exec(src)
  if (!m?.[1]) throw new Error('SWAPPER_SRC not found in crates/render/src/lib.rs — re-pin protocol.ts')
  return m[1]
}

test('protocol(svelte): SWAPPER_SRC is byte-identical to the Rust render crate', () => {
  assert.equal(SWAPPER_SRC, rustSwapperSrc())
})

test('protocol(svelte): boundaryContent + islandElement use single-quoted, escaped attrs', () => {
  const content = boundaryContent(1, '<p>ok</p>')
  assert.ok(content.startsWith(`<template data-ox-c='1'><p>ok</p></template>`))
  const island = islandElement(
    { component: `Q'x`, chunk: '/a.js', strategy: 'idle', propsB64: 'e30=' },
    '<p>ssr</p>',
  )
  // SEC-07: single quotes in injected attribute values must be escaped or
  // they break out of the attribute (parity with the render crate).
  assert.ok(island.startsWith(`<ox-island data-ox-island='Q&#39;x'`))
  assert.ok(island.endsWith(`><p>ssr</p></ox-island>`))
})
