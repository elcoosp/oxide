# oxide-og

Premium OpenGraph image generator for the **Oxide** project (`github.com/elcoosp/oxide`).

Turns the repo's brand assets — `docs/logo.png` plus the oxide-branded
`react.png` / `vue.png` / `svelte.png` / `solid.png` icons — into sleek premium
social preview images rendered with headless Chromium (Playwright), in a light
(warm paper) and a dark (midnight) theme.

## Output

| Size       | File                 | Use                                        |
| ---------- | -------------------- | ------------------------------------------ |
| 1200 × 630 | `out/og.png`         | Standard OpenGraph / Twitter / Discord     |
| 1280 × 640 | `out/og-github.png`  | GitHub repo **Settings → Social preview**  |
| 1200 × 630 | `out/og-dark.png`    | Dark-theme variant (`--theme dark`)        |
| 1280 × 640 | `out/og-github-dark.png` | Dark-theme GitHub social preview       |

The default theme is **light**: a warm paper canvas where the transparent brand
PNGs sit directly on white cards, so every glyph (including the darker parts of
the artwork) stays fully visible. `--theme dark` renders the midnight variant
with the same layout. Both are rendered at 2× (retina) and quantized to < 1 MB.
The layout uses the README tagline as the headline — **"One Rust core. Every
framework."** — with the second line in the brand's rust gradient, the four
adapters as cards showing the real upstream renderers (`react-dom/server`,
`@vue/server-renderer`, `svelte/server`, `@solidjs/web`), and honest,
spec-anchored footer stats.

## Assets

`assets/` contains verbatim copies of the repo's `docs/*.png` brand assets —
`logo.png` and the four framework icons, all with transparent backgrounds as
shipped. The template places every icon on a **white rounded tile** so the
glyphs (including the darker parts of the artwork) stay visible against the
dark canvas. Swap in updated `docs/*.png` copies at any time; no processing
required.

## Usage

```bash
npm install

# Generate both images from the bundled ./assets copies
npm run generate

# Or point at the live repo's docs directory
npx tsx src/cli.ts --input ../../oxide/docs --out out

# One size only, custom headline, 1x output
npx tsx src/cli.ts --sizes og --scale 1 \
  --headline "Ship every framework. One binary." \
  --out out
```

## Options

| Flag               | Default                                                        |
| ------------------ | -------------------------------------------------------------- |
| `--input <dir>`    | `./assets` — must contain `logo.png` + the four framework PNGs |
| `--out <dir>`      | `./out`                                                        |
| `--sizes <list>`   | `all` (or `og`, `github`)                                      |
| `--scale <n>`      | `2` (supersampling factor; `1` = exact pixels)                 |
| `--headline <text>`| `One Rust core. Every framework.` (first `.` splits the gradient line) |
| `--sub <text>`     | one-engine summary line                                        |
| `--repo <text>`    | `github.com/elcoosp/oxide` (top-right)                         |
| `--kicker <text>`  | `The Rust-core meta-framework`                                 |
| `--version <text>` | `v0.1` (header pill)                                           |
| `--footer-left`    | `HTTP/1+2+3 · Streaming SSR · ODF byte-parity · Single static binary` |
| `--footer-right`   | `MIT OR Apache-2.0`                                            |

## How it works

1. `src/assets.ts` — reads the five PNGs and the Inter / JetBrains Mono woff2
   fonts (via `@fontsource`) and inlines everything as base64 data URIs, so the
   render needs **zero network access**.
2. `src/template.ts` — builds the full HTML/CSS document: layered rust glows,
   center-masked grid, SVG `feTurbulence` film grain, vignette, luminous top
   hairline, glassmorphism chips.
3. `src/render.ts` — Playwright loads the document at the exact viewport size
   with `deviceScaleFactor: scale`, waits for `document.fonts.ready`, and
   screenshots a lossless PNG.
4. Compress afterwards with any palette quantizer to land under 1 MB
   (e.g. `sharp` with `palette: true, quality: 92, dither: 1.0`).

## Wiring it up

- **GitHub social preview:** repo → *Settings* → *General* → *Social preview* →
  upload `out/og-github.png`.
- **Website / link previews:** reference `out/og.png` in your meta tags:

```html
<meta property="og:image" content="https://oxide.dev/og.png" />
<meta property="og:image:width" content="1200" />
<meta property="og:image:height" content="630" />
<meta name="twitter:card" content="summary_large_image" />
```
