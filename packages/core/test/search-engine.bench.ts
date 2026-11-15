// Static-matcher micro-bench (V2-F09). Zero-dependency, Node-only (type
// stripping); NOT part of the CI test glob (`*.test.ts`) — run it
// directly: `node --experimental-strip-types test/search-engine.bench.ts`
// (Node ≥ 24 strips types without the flag).
//
// Reports ns/op (median of runs) for the client matcher's two costs:
//   match/100_docs  — the full matchStatic over a 100-doc synthetic
//                     corpus (the per-keystroke cost a static site's
//                     search box pays; must stay well under a frame)
//   tokenize/query  — the tokenizer (the per-keystroke floor)
// The server-side equivalents are the `search_query` criterion benches;
// this pins the CLIENT's parity shape stays O(docs × tokens) with no
// accidental per-token allocation blowup.
import { performance } from 'node:perf_hooks'
import { search, tokenize, type StaticIndex } from '../src/client/searchEngine.ts'

function syntheticIndex(docs: number): StaticIndex {
  const terms = ['install', 'indexer', 'search', 'routes', 'postings', 'launch', 'oxide', 'guide', 'content', 'deploy']
  const all: StaticIndex = { docs: [], postings: {} }
  for (let i = 0; i < docs; i++) {
    const t = terms[i % terms.length]
    all.docs.push({
      id: `doc-${i}`,
      url: `/doc-${i}`,
      title: `${t} page ${i}`,
      excerpt: `The ${t} docs entry ${i} with more prose around it.`,
      tags: i % 2 === 0 ? ['docs'] : ['blog'],
    })
    for (const term of terms) {
      const list = (all.postings[term] ??= [])
      if (term === t || i % 3 === 0) list.push(i)
    }
  }
  return all
}

const index = syntheticIndex(100)

function medianNs(fn: () => void, runs = 200, iters = 50): number {
  const samples: number[] = []
  fn() // warmup
  for (let r = 0; r < runs; r++) {
    const t0 = performance.now()
    for (let i = 0; i < iters; i++) fn()
    const t1 = performance.now()
    samples.push((t1 - t0) / iters)
  }
  samples.sort((a, b) => a - b)
  const mid = Math.floor(samples.length / 2)
  const medianMs = samples.length % 2 === 1 ? samples[mid] : (samples[mid - 1] + samples[mid]) / 2
  return medianMs * 1e6
}

const matchNs = medianNs(() => {
  void search('search content postings', { mode: 'static', staticIndex: index })
})
const tokenizeNs = medianNs(() => {
  void tokenize('Install Oxide, then configure the routes')
})

console.log(`search-engine.bench`)
console.log(`  match/100_docs  ${matchNs.toFixed(0)} ns/op`)
console.log(`  tokenize/query  ${tokenizeNs.toFixed(0)} ns/op`)
