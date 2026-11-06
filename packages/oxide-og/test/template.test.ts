// buildTemplate — the OG image HTML (pure function, no browser needed).
//
// The template is the contract between the CLI flags and the rendered PNG:
// dimensions, theme variables, the headline's ink/gradient split, adapter
// chips with the real upstream renderer names, and HTML escaping of every
// user-provided string.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { buildTemplate, ADAPTER_CHIPS, THEMES } from '../src/template.ts'
import type { TemplateOptions } from '../src/template.ts'

const blankAssets = { logo: 'data:image/png;base64,AA==', react: 'x', vue: 'x', svelte: 'x', solid: 'x' }
const blankFonts = {
  inter500: 'f5', inter600: 'f6', inter700: 'f7', inter800: 'f8', mono400: 'm4', mono500: 'm5',
}

function options(overrides: Partial<TemplateOptions> = {}): TemplateOptions {
  return {
    width: 1200,
    height: 630,
    headline: 'One Rust core. Every framework.',
    sub: 'sub-headline',
    repo: 'github.com/elcoosp/oxide',
    kicker: 'kicker',
    version: 'v0.1',
    footerLeft: 'left text',
    footerRight: 'right text',
    ...overrides,
  }
}

test('og-template: emits the exact pixel dimensions', () => {
  const html = buildTemplate({ assets: blankAssets, fonts: blankFonts, options: options({ width: 1280, height: 640 }) })
  assert.ok(html.includes('html, body { width:1280px; height:640px; overflow:hidden; }'))
})

test('og-template: the first sentence splits ink (line 1) and gradient (line 2)', () => {
  const html = buildTemplate({
    assets: blankAssets,
    fonts: blankFonts,
    options: options({ headline: 'One Rust core. Every framework.' }),
  })
  assert.ok(html.includes('<h1>One Rust core.<br/><span class="accent">Every framework.</span></h1>'))
})

test('og-template: a headline without sentence punctuation falls back to a word split', () => {
  const html = buildTemplate({
    assets: blankAssets,
    fonts: blankFonts,
    options: options({ headline: 'One Rust core for everything' }),
  })
  assert.ok(html.includes('<h1>One Rust core<br/><span class="accent">for everything</span></h1>'))
})

test('og-template: a single-word headline leaves the accent line empty', () => {
  const html = buildTemplate({
    assets: blankAssets,
    fonts: blankFonts,
    options: options({ headline: 'Oxide' }),
  })
  assert.ok(html.includes('<h1>Oxide<br/><span class="accent"></span></h1>'))
})

test('og-template: every user-provided string is HTML-escaped', () => {
  const html = buildTemplate({
    assets: blankAssets,
    fonts: blankFonts,
    options: options({
      headline: 'Evil <script>alert("x")</script>. done',
      sub: 'a & b < c',
      repo: 'x"y',
      version: 'v1<b>',
    }),
  })
  assert.ok(html.includes('&lt;script&gt;'), 'headline tags are inert')
  assert.ok(html.includes('alert(&quot;x&quot;)'), 'double quotes are escaped')
  assert.ok(!html.includes('<script>alert'), 'no live script element can appear in the hero')
  assert.ok(html.includes('<p class="sub">a &amp; b &lt; c</p>'))
  assert.ok(html.includes('<span class="repo">x&quot;y</span>'))
})

test('og-template: light and dark themes emit their own CSS variables', () => {
  const light = buildTemplate({ assets: blankAssets, fonts: blankFonts, options: options(), theme: 'light' })
  const dark = buildTemplate({ assets: blankAssets, fonts: blankFonts, options: options(), theme: 'dark' })
  assert.ok(light.includes('--bg:linear-gradient(180deg, #fbf9f6'), 'light paper background')
  assert.ok(dark.includes('--bg:linear-gradient(180deg, #0c0c10'), 'dark midnight background')
  // Both must carry the SAME variable set — the CSS layout is theme-agnostic.
  assert.deepEqual(Object.keys(THEMES.dark), Object.keys(THEMES.light))
  for (const theme of ['light', 'dark'] as const) {
    const html = buildTemplate({ assets: blankAssets, fonts: blankFonts, options: options(), theme })
    for (const key of Object.keys(THEMES[theme])) {
      assert.ok(html.includes(`${key}:`), `${theme} template must define ${key}`)
    }
  }
})

test('og-template: the four adapter chips use the real upstream renderer names', () => {
  const html = buildTemplate({ assets: blankAssets, fonts: blankFonts, options: options() })
  assert.deepEqual(
    ADAPTER_CHIPS.map((c) => c.name),
    ['React', 'Vue', 'Svelte', 'Solid'],
  )
  for (const chip of ADAPTER_CHIPS) {
    assert.ok(html.includes(`alt="${chip.name}"/>`), `chip icon for ${chip.name}`)
    assert.ok(html.includes(`>${chip.bridge}</div>`), `chip bridge label for ${chip.name}`)
  }
})

test('og-template: fonts are inlined as woff2 @font-face rules', () => {
  const html = buildTemplate({ assets: blankAssets, fonts: blankFonts, options: options() })
  assert.ok(html.includes('src:url("f8") format("woff2")'), 'Inter 800 weight inlined')
  assert.ok(html.includes("font-family:'JetBrains Mono'"), 'mono family declared')
  const faces = html.match(/@font-face/g) ?? []
  assert.equal(faces.length, 6, 'six @font-face blocks (4 Inter + 2 JetBrains Mono)')
})
