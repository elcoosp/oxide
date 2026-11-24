// V2-E02 — the Svelte adapter's typed `Link` pins (the F01 twins
// pattern).
//
// The package ships `src/Link.svelte` as source; a consumer's Svelte
// compiler builds it. This test does that compile itself (the same
// `svelte/compiler` path the image/island tests use) and asserts the SSR
// output — proving the component is valid Svelte 5 and emits the right
// attributes. These ride CI (`pnpm --filter @oxide-fw/svelte test`; the
// workspace's node_modules resolves `svelte` and `@oxide-fw/core/client`)
// — the same posture the react `link.test.ts` pins record for this
// sandbox.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { compile } from 'svelte/compiler'
import type { Component } from 'svelte'
import { render } from 'svelte/server'
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
/** Every temp dir this test creates, so a `process.on('exit')` can clean
 *  them. `mkdtempSync` under the package (not `/tmp`) is required so
 *  `svelte/internal/server` resolves — that means a leaked dir lands in the
 *  repo, which is not acceptable. */
const createdDirs: string[] = []
process.on('exit', () => {
  for (const d of createdDirs) {
    try { rmSync(d, { recursive: true, force: true }) } catch { /* best effort */ }
  }
})
const SOURCE = readFileSync(join(here, '../src/Link.svelte'), 'utf8')

/** Compile the component once. The output lives beside the package so
 *  `svelte/internal/server` resolves (a `/tmp` path cannot see it). */
type SvelteComp = Component<Record<string, unknown>, Record<string, unknown>, string>
let cached: { default: SvelteComp } | null = null
async function load(): Promise<{ default: SvelteComp }> {
  if (cached) return cached
  const out = compile(SOURCE, { generate: 'server', name: 'Link', filename: 'Link.svelte' })
  const dir = mkdtempSync(join(here, 'link-'))
  createdDirs.push(dir)
  const path = join(dir, 'Link.mjs')
  writeFileSync(path, out.js.code)
  cached = (await import(pathToFileURL(path).href)) as { default: SvelteComp }
  return cached
}

test('renders an anchor with the built href and the interception mark', async () => {
  const mod = await load()
  const res = render(mod.default, {
    props: { route: '/blog/[slug]', params: { slug: 'hi' } },
  })
  assert.match(res.body, /<a/)
  assert.match(res.body, /href="\/blog\/hi"/)
  assert.match(res.body, /data-ox-link/)
})

test('the search and hash opts ride the href', async () => {
  const mod = await load()
  const res = render(mod.default, {
    props: { route: '/about', opts: { search: { page: 2 }, hash: 'top' } },
  })
  assert.match(res.body, /href="\/about\?page=2#top"/)
})

test('anchor props spread verbatim after the built ones', async () => {
  const mod = await load()
  const res = render(mod.default, {
    props: {
      route: '/about',
      anchorProps: { class: 'nav', 'aria-label': 'About page' },
    },
  })
  assert.match(res.body, /class="nav"/)
  assert.match(res.body, /aria-label="About page"/)
  assert.match(res.body, /href="\/about"/, 'the built href survives the spread')
})
