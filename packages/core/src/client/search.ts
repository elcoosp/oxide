// §16.5: client-side route search.
//
// The build emits `dist/client/search-index.json` (see
// `crates/assets/src/route_search.rs`): one entry per prerendered route,
// with the page's title, `<h1>`–`<h6>` headings, and a short excerpt.
// This module is the *reader*: no network call, no server — just a
// scoring function the caller drives from its own UI.
//
// The plan's wording is "a client-side `oxide-search` island
// (subsequence scoring, no WASM needed for v1)". The island's *markup*
// is a user design decision — this module is what every such island
// needs and what the framework can ship without prescribing a UI.
//
// Scoring: per token in the query, +3 for a title match, +2 for any
// heading, +1 for the excerpt. Higher score first, ties broken by
// route id (byte-stable). An empty query returns an empty result — a
// UI that wants "show all" can do so itself.

export interface SearchEntry {
  route: string
  url: string
  title?: string
  headings?: string[]
  excerpt?: string
}

export interface SearchIndex {
  version: number
  entries: SearchEntry[]
}

export interface SearchResult {
  route: string
  url: string
  title: string | undefined
  headings: string[]
  excerpt: string | undefined
  score: number
}

/**
 * Tokenize by lowercasing and splitting on any non-alphanumeric run.
 * Drops single-character tokens (`a`, `of`) — a 1-char query rarely
 * discriminates and inflates the score for common words.
 */
export function tokenize(s: string): string[] {
  return s
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((t) => t.length >= 2)
}

/**
 * Prefix-match a query token against a set of index tokens. A query token
 * matches when it is a prefix of any index token (`install` matches
 * `installation`; `getting` matches `getting`). Substring matching
 * (`stal` matching `installation`) is deliberately not supported — it
 * produces so many false positives for short queries that a result list
 * becomes noise.
 */
function matchesAny(qt: string, tokens: Set<string>): boolean {
  for (const t of tokens) {
    if (t.startsWith(qt)) return true
  }
  return false
}

/**
 * Rank index entries against a free-text query. Multi-token queries are
 * AND-scored (all tokens contribute, each against the entry) — the
 * result list is filtered to entries with score > 0, so a token that
 * matches nothing in an entry adds nothing rather than excluding it.
 * That is the behaviour a search box wants: partial matches still rank,
 * just lower.
 *
 * Per query token, the score is:
 *   +3 if the token prefix-matches any title token
 *   +2 if it prefix-matches any heading token
 *   +1 if it prefix-matches any excerpt token
 *
 * Prefix matching is what makes `install` find a page whose heading is
 * `Installation`; without it a user typing a partial word gets zero
 * results from a genuinely relevant page.
 */
export function search(
  index: SearchIndex,
  query: string,
  opts: { limit?: number } = {},
): SearchResult[] {
  const tokens = tokenize(query)
  if (tokens.length === 0) return []

  const scored: SearchResult[] = []
  for (const e of index.entries) {
    const titleTokens = new Set(tokenize(e.title ?? ''))
    const headingTokens = new Set(tokenize((e.headings ?? []).join(' ')))
    const excerptTokens = new Set(tokenize(e.excerpt ?? ''))
    let score = 0
    for (const t of tokens) {
      if (matchesAny(t, titleTokens)) score += 3
      if (matchesAny(t, headingTokens)) score += 2
      if (matchesAny(t, excerptTokens)) score += 1
    }
    if (score === 0) continue
    scored.push({
      route: e.route,
      url: e.url,
      title: e.title,
      headings: e.headings ?? [],
      excerpt: e.excerpt,
      score,
    })
  }

  scored.sort((a, b) => {
    if (b.score !== a.score) return b.score - a.score
    return a.route < b.route ? -1 : a.route > b.route ? 1 : 0
  })

  const limit = opts.limit ?? 20
  return scored.slice(0, limit)
}
