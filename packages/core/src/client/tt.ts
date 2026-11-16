// V2-E04 — the Trusted Types policy, defined ONCE per realm (spec
// §V2-E04). The pinned CSP directive (`crates/server/src/lib.rs`
// `tt_directives`) emits `trusted-types oxide default` when
// `security.trustedTypes` is `report`/`enforce` — so this module creates
// the policy under the name `oxide`, and the inline deferred swapper
// (which cannot import a module) creates the page's `default` policy, the
// directive's second allowed name.
//
// The passthrough is the point: the policy exists so every core
// `innerHTML` assignment has ONE audited route (`tt.createHTML`), and so
// a platform with Trusted Types enforcement sees policy-created values
// instead of raw strings. When the platform has no `trustedTypes`, or the
// `oxide` name is already taken (the handle is not retrievable — a second
// boot bundle in the realm), the fallback is the identity object, never a
// throw at import time.

/**
 * The policy surface the core client uses (the HTML sink; Trusted Types
 * also defines script/scriptURL sinks the core never assigns).
 */
export interface OxideTrustedTypesPolicy {
  createHTML(s: string): string
}

interface TrustedTypesPlatform {
  createPolicy(name: string, rules: { createHTML(s: string): string }): unknown
}

/**
 * Create (or fall back to) the page's `oxide` policy. Import-time
 * side effect by design — "the core boot defines the policy once" is a
 * module-level statement, not a function the caller must remember to call.
 */
function createPolicy(): OxideTrustedTypesPolicy {
  const w = globalThis as { trustedTypes?: TrustedTypesPlatform }
  if (w.trustedTypes && typeof w.trustedTypes.createPolicy === 'function') {
    try {
      return w.trustedTypes.createPolicy('oxide', { createHTML: (s: string) => s }) as OxideTrustedTypesPolicy
    } catch {
      // The name is taken (a second boot bundle imported this module in
      // the same realm). The existing policy cannot be retrieved, so the
      // identity fallback keeps the sink call sites total.
    }
  }
  return { createHTML: (s: string) => s }
}

/** The realm's `oxide` policy — every core `innerHTML` sink routes through it. */
export const tt: OxideTrustedTypesPolicy = createPolicy()

// The realm-wide handle: inline scripts (the deferred swapper) and app
// code reach the same policy object without importing this module.
;(globalThis as { __oxide_tt?: OxideTrustedTypesPolicy }).__oxide_tt = tt
