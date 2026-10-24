// `<Image>` — the Svelte adapter's image component (§16.1).
//
// The package ships `src/Image.svelte` as source; a consumer's Svelte
// compiler builds it. This test does that compile itself (the same
// `svelte/compiler` path the consumer's bundler uses) and asserts the SSR
// output — proving the component is valid Svelte 5 and emits the right
// attributes.
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
const SOURCE = readFileSync(join(here, '../src/Image.svelte'), 'utf8')

/** Compile the component once. The output lives beside the package so
 *  `svelte/internal/server` resolves (a `/tmp` path cannot see it). */
type SvelteComp = Component<Record<string, unknown>, Record<string, unknown>, string>
let cached: { default: SvelteComp } | null = null
async function load(): Promise<{ default: SvelteComp }> {
  if (cached) return cached
  const out = compile(SOURCE, { generate: 'server', name: 'Image', filename: 'Image.svelte' })
  const dir = mkdtempSync(join(here, 'img-'))
  createdDirs.push(dir)
  const path = join(dir, 'Image.mjs')
  writeFileSync(path, out.js.code)
  cached = (await import(pathToFileURL(path).href)) as { default: SvelteComp }
  return cached
}

/** Render and return both halves. `<svelte:head>` content (a `priority`
 *  image's `<link rel="preload">`) lands in `head`, not `body`. */
function renderBoth(
  mod: { default: SvelteComp },
  props: Record<string, unknown>,
): { body: string; head: string } {
  const res = render(mod.default, { props })
  return { body: res.body, head: res.head }
}

function renderToString(mod: { default: SvelteComp }, props: Record<string, unknown>): string {
  return renderBoth(mod, props).body
}

test('renders an img with src, alt and default lazy loading', async () => {
  const mod = await load()
  const body = renderToString(mod, { src: '/a.png', alt: 'A' })
  assert.match(body, /<img[^>]*src="\/a\.png"/, body)
  assert.match(body, /alt="A"/, body)
  assert.match(body, /loading="lazy"/, body)
})

test('a variant module spreads srcset/sizes/width/height', async () => {
  const mod = await load()
  const body = renderToString(mod, {
    src: '/_oxide/image?src=hero.png&w=1200',
    srcset: '/_oxide/image?src=hero.png&w=400 400w, /_oxide/image?src=hero.png&w=1200 1200w',
    sizes: '100vw',
    width: 1200,
    height: 630,
    alt: 'Hero',
  })
  assert.match(body, /srcset="[^"]*400w[^"]*1200w"/, body)
  assert.match(body, /sizes="100vw"/, body)
  assert.match(body, /width="1200"/, body)
  assert.match(body, /height="630"/, body)
})

test('priority loads eagerly and emits a preload link in head', async () => {
  const mod = await load()
  const { body, head } = renderBoth(mod, { src: '/hero.png', alt: 'Hero', priority: true })
  assert.match(body, /loading="eager"/, body)
  assert.match(body, /fetchpriority="high"/, body)
  // `<svelte:head>` content lands in `head`, not `body`.
  assert.match(head, /rel="preload"/, head)
  assert.match(head, /as="image"/, head)
  assert.match(head, /href="\/hero\.png"/, head)
})

// §16.1: sizes derived from srcset when absent.
test('sizes is derived from srcset when absent', async () => {
  const mod = await load()
  const body = renderToString(mod, {
    src: '/_oxide/image?src=hero.png&w=1200',
    srcset: '/_oxide/image?src=hero.png?w=400 400w, /_oxide/image?src=hero.png?w=800 800w, /_oxide/image?src=hero.png?w=1200 1200w',
    width: 1200,
    height: 630,
    alt: 'Hero',
  })
  assert.match(body, /sizes="\(min-width: 1200px\) 1200px, \(min-width: 800px\) 800px, \(min-width: 400px\) 400px, 100vw"/, body)
})
