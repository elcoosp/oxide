// V2-E02 — the runtime half of typed search params (spec §V2-E02).
//
// The Rust typegen (`crates/router/src/typegen.rs`) already emits the
// per-route `searchSchema` imports and the `RouteSearch` map; this module
// is what CALLS the schema at runtime. The contract (the spec, verbatim
// in spirit): `parseSearch(schema, url.searchParams)` calls the schema's
// Standard Schema V1 `~standard.validate` and surfaces `issues` under the
// spec's `OX-SRC-001` code — the caller decides the 400 page — or returns
// the validated value plus, when validation CHANGED the value (a default
// the URL omitted, a coercion whose re-serialization differs), the
// `corrected` query string the caller may redirect to so the URL matches
// what the schema produced.
//
// Structural only: the runtime never imports a concrete validator — the
// Standard Schema V1 contract (`~standard.validate` returning a result or
// a promise of one) is the entire surface, mirroring the typegen's
// `import type { StandardSchemaV1 as S } from '@standard-schema/spec'`
// header without needing the package at runtime.

/**
 * The structural slice of Standard Schema V1 (`@standard-schema/spec`)
 * the runtime consumes. A schema is any object whose `~standard.validate`
 * takes the raw input and answers either `{ value }` or `{ issues }` —
 * synchronously or as a promise.
 */
export interface StandardSchemaV1Like<Input = Record<string, string>, Output = unknown> {
  '~standard': {
    validate(value: Input): StandardValidateResult<Output> | Promise<StandardValidateResult<Output>>
  }
}

/** The Standard Schema V1 result: a value or issues, never both. */
export type StandardValidateResult<T> =
  | { value: T; issues?: undefined }
  | { value?: undefined; issues: StandardIssue[] }

/** The Standard Schema V1 issue shape (message required, path optional). */
export interface StandardIssue {
  message: string
  path?: Array<PropertyKey> | undefined
}

/** The success half: the validated (typed) search plus the redirect hint. */
export type SearchParseSuccess<T> = {
  ok: true
  /** The schema's validated output — the loader's typed `search`. */
  search: T
  /**
   * The query string (no leading `?`) to redirect to when validation
   * changed the value (a default applied, a re-serialization differs);
   * `undefined` when the URL already matches, so the caller never
   * redirects an already-correct navigation.
   */
  corrected?: string | undefined
}

/** The failure half: the spec's `OX-SRC-001` code and the schema's issues. */
export type SearchParseFailure = {
  ok: false
  /** Spec Appendix V2.K: invalid search params for a schema-declaring route. */
  code: 'OX-SRC-001'
  issues: StandardIssue[]
}

/** The parse verdict: either half narrows on `ok` (discriminated union). */
export type SearchParseResult<T> = SearchParseSuccess<T> | SearchParseFailure

/**
 * Serialize a validated search value back into a query string (no leading
 * `?`). Flat values only: strings/numbers/booleans ride `String()`, arrays
 * repeat the key, `undefined`/`null` are skipped (a defaulted-absent key
 * must not reappear), and nested objects are skipped — search params are
 * flat by contract. Key order follows insertion order, which is exactly
 * the order `URLSearchParams` serializes, so a value-preserving validation
 * round-trips to the same string and never produces a spurious redirect.
 */
function searchToQueryString(value: unknown): string {
  if (value === undefined || value === null || typeof value !== 'object') return ''
  const sp = new URLSearchParams()
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    if (v === undefined || v === null) continue
    if (Array.isArray(v)) {
      for (const item of v) {
        if (item !== undefined && item !== null) sp.append(k, String(item))
      }
    } else {
      sp.append(k, String(v))
    }
  }
  return sp.toString()
}

/**
 * Normalize the input to the plain object the schema validates: a
 * `URLSearchParams` (or a record) → `Record<string, string>`. Repeated
 * keys collapse last-wins — the schema, not the transport, owns whether
 * duplicates are meaningful; a schema that wants arrays declares an array
 * output and the corrected-URL round-trip preserves it.
 */
function paramsToObject(params: URLSearchParams | Record<string, string>): Record<string, string> {
  if (!(params instanceof URLSearchParams)) return { ...params }
  const out: Record<string, string> = {}
  for (const [k, v] of params.entries()) out[k] = v
  return out
}

/**
 * Parse a route's search params through its declared Standard Schema.
 *
 * @param schema - The route's `searchSchema` export (any Standard Schema V1).
 * @param params - The raw search: a `URLSearchParams` or a plain record.
 * @returns The discriminated result; on success `search` is the typed
 *   output and `corrected` (when present) is the query string a caller may
 *   redirect to; on failure `code` is `OX-SRC-001` and `issues` carries the
 *   schema's own issues verbatim.
 */
export function parseSearch<Input, Output>(
  schema: StandardSchemaV1Like<Input, Output>,
  params: URLSearchParams | Record<string, string>,
): SearchParseResult<Output> | Promise<SearchParseResult<Output>> {
  const raw = paramsToObject(params)
  const rawQuery = searchToQueryString(raw)
  const settle = (res: StandardValidateResult<Output>): SearchParseResult<Output> => {
    if (res.issues !== undefined) {
      return { ok: false, code: 'OX-SRC-001', issues: res.issues }
    }
    const corrected = searchToQueryString(res.value)
    return {
      ok: true,
      search: res.value,
      // Only a DIFFERENT serialization is a redirect: `String()`-equal
      // coercions (`'1'` → `1`) round-trip to the same query and must not
      // bounce the navigation.
      corrected: corrected !== rawQuery ? corrected : undefined,
    }
  }
  const validated = schema['~standard'].validate(raw as Input)
  // A sync validate answers synchronously (the hot path — no promise
  // allocation); the Standard contract's async half resolves the promise.
  if (typeof (validated as { then?: unknown }).then === 'function') {
    return Promise.resolve(validated as Promise<StandardValidateResult<Output>>).then(settle)
  }
  return settle(validated as StandardValidateResult<Output>)
}
