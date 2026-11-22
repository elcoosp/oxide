// §V2-R01 C41 — search returns ranked hits and works as static JSON (F09).
//
// Pins `packages/core/src/client/searchEngine.ts::search`: the static matcher
// scores by postings hit + title/excerpt prefix arms, sorts score desc with
// the doc-order tiebreak, and a loader function is memoized (runs once per
// identity). An empty query answers locally without a fetch.

import { search, type StaticDoc, type StaticIndex } from '../../packages/core/src/client/searchEngine.ts'
import type { Assertion } from './types.ts'

const docs: StaticDoc[] = [
  { id: 'd0', url: '/alpha', title: 'Alpha Router', excerpt: 'the router contract', tags: ['docs'] },
  { id: 'd1', url: '/beta', title: 'Beta Actions', excerpt: 'actions and enhance', tags: ['guide'] },
  { id: 'd2', url: '/gamma', title: 'Alpha Islands', excerpt: 'islands everywhere', tags: ['docs'] },
]
const index: StaticIndex = { docs, postings: { router: [0], islands: [2] } }

const c41: Assertion = {
  id: 'C41',
  name: 'search returns ranked hits and works as static JSON (F09)',
  capability: 'search',
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'client',
  async run() {
    // An empty query answers locally (no fetch) — a UI's empty input box.
    const empty = await search('   ')
    if (empty.total !== 0 || empty.hits.length !== 0) {
      throw new Error(`an empty query answers locally without a fetch (total=${empty.total})`)
    }

    // 'alpha' is a title prefix of d0 AND d2 — both score, ranked by score
    // desc with the doc-order tiebreak (d0 before d2).
    const res = await search('alpha', { staticIndex: index })
    if (res.total !== 2) {
      throw new Error(`both title-prefix docs scored: total=${res.total}, expected 2`)
    }
    const order = res.hits.map((h) => h.id)
    if (order[0] !== 'd0' || order[1] !== 'd2') {
      throw new Error(`ranked by score desc + doc-order tiebreak: got ${JSON.stringify(order)}`)
    }
    if (res.hits[0]?.score !== 3) {
      throw new Error(`the title-prefix arm scores 3 (got ${res.hits[0]?.score})`)
    }

    // A postings hit rides the body boost (router → d0 only).
    const postings = await search('router', { staticIndex: index })
    if (postings.hits.length !== 1 || postings.hits[0]?.id !== 'd0') {
      throw new Error(`a postings hit rides the body boost: got ${JSON.stringify(postings.hits.map((h) => h.id))}`)
    }

    // A loader function is memoized — runs once per identity, every search
    // after the first reuses it (the lazy-load seam the static export rides).
    let loads = 0
    const loader = async (): Promise<StaticIndex> => {
      loads++
      return index
    }
    await search('alpha', { staticIndex: loader })
    await search('router', { staticIndex: loader })
    await search('alpha', { staticIndex: loader })
    if (loads !== 1) {
      throw new Error(`the static-index loader runs once per identity (loads=${loads}, expected 1)`)
    }
    return { ok: true, detail: 'ranked(title prefix + postings) + doc-order tiebreak + loader memoized once' }
  },
}

export default c41
