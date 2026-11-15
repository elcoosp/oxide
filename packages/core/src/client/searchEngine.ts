// V2-F09 — the client half of the built-in search engine (the spec's
// "`import { search } from 'oxide/search'` … calls the endpoint when
// `window.__oxide.search === 'server'` and otherwise loads `static.json`
// lazily and matches prefix terms with a ten-line scorer (term hits
// weighted, title boost) so static sites work with no server").
//
// The degraded shape is a PARITY shape, not a different algorithm: the
// scorer mirrors the server's `StaticMatcher`
// (`crates/search/src/static_index.rs`) line for line — per token, a
// postings hit scores `bodyBoost`, a title word that STARTS WITH the
// token scores `max(titleBoost, 3)`, an excerpt word that starts with it
// scores `bodyBoost`; results sort by score desc with the doc order as
// the byte-stable tiebreak. The server pins a parity test
// (same corpus, top-3 equal), which is only honest if the client runs
// the same arithmetic — including the tag filter's landed post-take
// position, pinned as a quirk rather than silently "fixed".
//
// Wire contract (the endpoint's own constants,
// `crates/server/src/search_endpoint.rs`): `GET /_oxide/search` with
// `?q=&limit=&tag=&lang=`, limit default 10 / cap 100, empty-string
// filters = absent. An empty/whitespace query is refused LOCALLY (the
// server would 400 `SRC-104`) — a UI's empty input box is a normal
// state, so it returns an empty result without a fetch.

export type SearchHit = {
  id: string
  url: string
  title: string
  snippet: string
  score: number
}

export type SearchResponse = {
  total: number
  hits: SearchHit[]
}

export type StaticDoc = {
  id: string
  url: string
  title: string
  excerpt: string
  tags: string[]
}

export type StaticIndex = {
  docs: StaticDoc[]
  postings: Record<string, number[]>
}

export type SearchMode = 'server' | 'static'

export type SearchOptions = {
  /** `'server'` hits `/_oxide/search`; `'static'` matches the export. Default: the boot flag, else static. */
  mode?: SearchMode
  /** The endpoint's limit (default 10, cap 100 — the wire rules). */
  limit?: number
  /** Filter hits to one tag. */
  tag?: string
  /** Language filter (the engine's `search.languages`). */
  lang?: string
  /** The endpoint base. Default `/_oxide/search`. */
  endpoint?: string
  /** The static export's URL. Default `/search/static.json`. */
  staticUrl?: string
  /** The static index — a ready index or a loader (the lazy seam). */
  staticIndex?: StaticIndex | (() => Promise<StaticIndex>)
  /** The fetch to use (tests inject fakes; the default is global fetch). */
  fetchFn?: typeof fetch
}

/** The endpoint's wire default (`DEFAULT_RESULT_LIMIT`). */
const DEFAULT_LIMIT = 10
/** The endpoint's wire cap (`MAX_RESULT_LIMIT`). */
const MAX_LIMIT = 100
/** The static export's mount point (the build writes `dist/search/static.json`). */
const DEFAULT_STATIC_URL = '/search/static.json'
/** The endpoint's default path (the ops plane). */
const DEFAULT_ENDPOINT = '/_oxide/search'
/** The matcher's boosts (`DEFAULT_TITLE_BOOST` / `DEFAULT_BODY_BOOST`; the title arm takes the max with 3, the server's quirk). */
const BODY_BOOST = 1
const TITLE_BOOST = 3

/** The lazy static index: one load per page, every search after the first reuses it. */
let staticIndexPromise: Promise<StaticIndex> | undefined
/** The injected loaders' once-semantics: a loader runs once per function identity (a WeakMap keeps this leak-safe). */
const loaderMemo = new WeakMap<() => Promise<StaticIndex>, Promise<StaticIndex>>()

/**
 * Tokenize like the server matcher: lowercase, split on any
 * non-alphanumeric run, drop empties (the 2-char minimum the OLD route
 * search used does not apply here — the engine's tokenizer keeps short
 * terms, the postings carry the 2-to-4 char prefixes).
 */
export function tokenize(s: string): string[] {
  return s
    .split(/[^a-z0-9]+/i)
    .filter((t) => t.length > 0)
    .map((t) => t.toLowerCase())
}

/**
 * The built-in search's client entry: `search(q, opts)` resolves the mode
 * (explicit → the boot flag → static) and answers with the endpoint's
 * payload or the local matcher's.
 *
 * @param q - The query text (empty/whitespace returns an empty result without a fetch).
 * @param opts - The mode, wire params and injectable seams.
 * @returns The endpoint-shaped payload (`{ total, hits }`).
 */
export async function search(q: string, opts: SearchOptions = {}): Promise<SearchResponse> {
  if (q.trim() === '') {
    return { total: 0, hits: [] }
  }
  const mode: SearchMode = opts.mode ?? bootMode() ?? 'static'
  if (mode === 'server') {
    return searchServer(q, opts)
  }
  const index = await resolveStaticIndex(opts)
  return matchStatic(index, q, opts)
}

/** The boot flag's mode (`window.__oxide.search`), absent outside a browser or an unflagged boot. */
function bootMode(): SearchMode | undefined {
  const w = globalThis as { __oxide?: { search?: SearchMode } }
  return w.__oxide?.search
}

/** The endpoint call: the wire params ride their endpoint rules (limit default/cap, empty filters = absent). */
async function searchServer(q: string, opts: SearchOptions): Promise<SearchResponse> {
  const url = new URL(opts.endpoint ?? DEFAULT_ENDPOINT, 'http://oxide.local')
  url.searchParams.set('q', q)
  url.searchParams.set('limit', String(clampLimit(opts.limit)))
  if (opts.tag?.trim()) url.searchParams.set('tag', opts.tag)
  if (opts.lang?.trim()) url.searchParams.set('lang', opts.lang)
  const fetchFn = opts.fetchFn ?? fetch
  const href = opts.endpoint
    ? url.pathname + url.search
    : url.pathname.replace(/^http:\/\/oxide\.local/, '') + url.search
  const res = await fetchFn(opts.endpoint ? href : `${DEFAULT_ENDPOINT}${url.search}`)
  if (!res.ok) {
    throw new Error(`oxide search: the search endpoint answered ${res.status}`)
  }
  return (await res.json()) as SearchResponse
}

/** The endpoint's limit rules: default 10, cap 100 (a `?limit=` of 0/NaN rides the default). */
function clampLimit(limit: number | undefined): number {
  if (limit === undefined || !Number.isFinite(limit) || limit < 1) return DEFAULT_LIMIT
  return Math.min(Math.floor(limit), MAX_LIMIT)
}

/** The lazy load: a ready index resolves as-is; a loader runs ONCE (memoized), and the fetch of `staticUrl` is the default loader. */
async function resolveStaticIndex(opts: SearchOptions): Promise<StaticIndex> {
  if (typeof opts.staticIndex === 'function') {
    const memo = loaderMemo.get(opts.staticIndex)
    if (memo) return memo
    const p = opts.staticIndex()
    loaderMemo.set(opts.staticIndex, p)
    return p
  }
  if (opts.staticIndex) {
    return opts.staticIndex
  }
  if (!staticIndexPromise) {
    const fetchFn = opts.fetchFn ?? fetch
    staticIndexPromise = (async () => {
      const res = await fetchFn(opts.staticUrl ?? DEFAULT_STATIC_URL)
      if (!res.ok) {
        throw new Error(
          `oxide search: the static export at ${opts.staticUrl ?? DEFAULT_STATIC_URL} answered ${res.status}`,
        )
      }
      try {
        return (await res.json()) as StaticIndex
      } catch (e) {
        throw new Error(
          `oxide search: the static export at ${opts.staticUrl ?? DEFAULT_STATIC_URL} does not parse as JSON: ${String(e)}`,
        )
      }
    })()
  }
  return staticIndexPromise
}

/**
 * The static matcher — the server's `StaticMatcher` arithmetic, verbatim:
 * postings hits, the title/excerpt prefix arms, the score sort with the
 * doc order tiebreak, the limit take, then the tag filter (the landed
 * post-take position, pinned as a parity quirk).
 */
function matchStatic(index: StaticIndex, q: string, opts: SearchOptions): SearchResponse {
  const tokens = tokenize(q)
  if (tokens.length === 0) {
    return { total: 0, hits: [] }
  }
  const limit = clampLimit(opts.limit)
  const tag = opts.tag?.trim() ? opts.tag.trim() : undefined
  const scored: Array<{ idx: number; score: number }> = []
  index.docs.forEach((doc, idx) => {
    let score = 0
    for (const token of tokens) {
      const list = index.postings[token]
      if (list !== undefined && list.includes(idx)) {
        score += BODY_BOOST
      }
      if (wordStartsWith(doc.title, token)) {
        score += Math.max(TITLE_BOOST, TITLE_BOOST)
      }
      if (wordStartsWith(doc.excerpt, token)) {
        score += BODY_BOOST
      }
    }
    if (score > 0) {
      scored.push({ idx, score })
    }
  })
  scored.sort((a, b) => b.score - a.score || a.idx - b.idx)
  const total = scored.length
  const hits = scored
    .slice(0, limit)
    .filter(({ idx }) => (tag ? index.docs[idx]?.tags.includes(tag) : true))
    .map(({ idx, score }) => {
      const doc = index.docs[idx]
      return {
        id: doc.id,
        url: doc.url,
        title: doc.title,
        snippet: doc.excerpt,
        score,
      }
    })
  return { total, hits }
}

/** Whether any whitespace-delimited word of `text` starts with `token` (the matcher's prefix reach). */
function wordStartsWith(text: string, token: string): boolean {
  return tokenize(text).some((w) => w.startsWith(token))
}
