// V2-R01: the TS bridge over the R02 capability matrix. The matrix is
// generated from `oxide_adapters::capability_matrix` by
// `cargo run -p oxide-fw-adapters --example dump_matrix` (ONE source of
// truth — the Rust `support` / `degradation` functions ARE the matrix); the
// committed `conformance/matrix.json` is what this module reads at runtime.
// `crates/adapters/tests/matrix_json.rs` is the drift guard that reloads the
// JSON and asserts every cell matches the Rust functions, so a Rust-side row
// change cannot land without the TS bridge moving too.
//
// Zero runtime deps: `node:fs` reads the JSON (no import-attribute edge
// cases), the rest is pure function over the parsed shape.

import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import type { CapabilityId, FeatureSupport, MatrixBridge, Target } from './types.ts'

/** The JSON shape `dump_matrix` emits (kept here so the bridge and the drift
 *  guard agree on the wire format). */
export interface MatrixShape {
  targets: string[]
  capabilities: string[]
  cells: Record<string, Record<string, FeatureSupport>>
  degradations: Record<string, Record<string, string | null>>
}

const here = dirname(fileURLToPath(import.meta.url))
const matrixPath = join(here, '..', 'matrix.json')
const matrix = JSON.parse(readFileSync(matrixPath, 'utf8')) as MatrixShape

/**
 * `support(target, capability)` — the R02 cell, bridged from Rust. Throws if
 * the cell is absent (the drift guard keeps the JSON and the Rust functions
 * in lockstep, so this is a fail-loud signal that the JSON is stale, not a
 * silent default).
 */
export function support(target: Target, cap: CapabilityId): FeatureSupport {
  const cell = matrix.cells[target]?.[cap]
  if (!cell) {
    throw new Error(
      `matrix bridge: no cell for target '${target}' × capability '${cap}' — regenerate conformance/matrix.json`,
    )
  }
  return cell
}

/**
 * `degradation(target, capability)` — the documented reason a Degraded cell
 * degrades, or `null` for Full/Unsupported (mirrors the Rust `degradation`:
 * absence is not degradation — the §18 refusal rule covers Unsupported at
 * build time).
 */
export function degradation(target: Target, cap: CapabilityId): string | null {
  return matrix.degradations[target]?.[cap] ?? null
}

/** The bridge object handed to every assertion's `run` as `ctx.matrix`. */
export const matrixBridge: MatrixBridge = { support, degradation }

/** Every target the matrix declares (the R02 row set, in declaration order). */
export const ALL_TARGETS = matrix.targets as readonly Target[]

/** Every capability the matrix declares (the R02 column set, in declaration order). */
export const ALL_CAPABILITIES = matrix.capabilities as readonly CapabilityId[]
