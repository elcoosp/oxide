Researched the 2026 Rust-JS toolchain landscape. Organized by pipeline stage, with maturity flags — because several of these are too early to bet a release on.

## Tier 1 — adopt now (mature, clear win)

### Lint + format: `oxlint` + `oxfmt`
You already depend on Oxc, so these are free. Oxlint runs **50–100× faster than ESLint**; Biome formats **35× faster than Prettier** with 97% syntax compat. Oxfmt is the formatter half of the same OXC toolchain.

More interesting: **Oxlint now does type-aware linting and full type checking** via `--type-aware --type-check`, which the docs explicitly say *"can replace a separate `tsc --noEmit` step in CI"*. That's 59/61 typescript-eslint type-aware rules covered.

> ⚠️ Caveat: the type-aware backend is `tsgolint`, which is **Go** (built on `typescript-go`), not Rust — Oxlint orchestrates it. Still one binary to invoke, but not pure-Rust.

### Type checking: `tsrs` (watch closely, don't ship yet)
`oxc-project/tsrs` is a **Rust port of the TypeScript 7 checker** — drop-in replacement for `tsc`, goal of 100% conformance, lower memory. As of Oct 2026 it's early; `--init`/`--showConfig` unsupported, and the Go-pinned compiler can produce different diagnostic ordering.

**Recommendation:** wire it behind `oxide check --tsrs` as an opt-in, keep `tsgo` (Go, ~10× faster than tsc) as the default. Note `tsgo` is **not an embeddable library** — it's a build driver, so you can't link it in-process.

### SVG: `oxvg`
Rust SVGO replacement, **up to 50× faster** on large file-sets, explicitly aims for SVGO compatibility but is not 1:1. Slots directly into your existing `?url` / SVG-as-component asset path (§6.7) — you currently have no SVG optimizer at all.

### Fonts: `fontcull`
You already have `fontations`/`skrifa` for reading. `fontcull` adds the missing half: **subsets fonts by the glyphs actually used on rendered pages**, pure-Rust `klippa` backend over fontations, WOFF2 via vendored Google woff2. It's a port of glyphhanger, which is exactly the §16.2 "subset and preload" requirement you're currently approximating with a static subset list.

Alternative if you want lower-level control: **`allsorts`** — parser + shaping + subsetter for OpenType/WOFF/WOFF2, entirely Rust. Heavier than fontcull; reach for it only if you need shaping too.

### Images: `rimage` (or `slimg-core`)
You already pull `image`, `fast_image_resize`, `ravif`, `webp`, `oxipng` individually. **`rimage`** bundles exactly those (`mozjpeg`, `oxipng`, `ravif`/`libavif`, `webp`, `imagequant`, `fast_image_resize` with rayon) behind one resumable pipeline API. **`slimg-core`** is the leaner alternative: MozJPEG, OxiPNG (Zopfli), WebP, AVIF (ravif), QOI.

**Recommendation:** don't swap — your current deps are the same codecs. But *do* copy rimage's **resumable/parallel job model** into your §16.1 transform worker; you're currently doing single-threaded per-request transforms.

### Markdown/MDX: `snapfire_fsr_mdx`
This is the closest thing to a drop-in `@mdx-js/mdx` in Rust: `.mdx`/`.md` → **TSX module whose default export renders it**, with byte→source-line mapping for diagnostics. Built on `markdown-rs` + `swc` (not Oxc — note the second parser), CommonMark + GFM, heading ids, TOC, excerpts, frontmatter (YAML/TOML), `syntect` highlighting via two-face grammars.

**Limitation:** remark/rehype plugins are JS and explicitly **not run**. Math is not parsed.

Alternative with a better DX story: **`@gentleduck/md`** — native Rust MDX with a velite-shaped typed-collections API (`s. object({ title: s.string() })` → generated `index.d.ts`), blake3 content cache, **9.5× velite** on the kitchen-sink benchmark, and it auto-strips JS remark/rehype plugins that have native equivalents. It's npm-distributed (0.4.1, 10 downloads/week — *very* early), so treat it as "watch", not "adopt".

Also on crates.io: **`unifast`** — "high-performance Markdown/MDX compiler" with a CLI. Least proven of the three.

## Tier 2 — adopt with a feature flag

### `pacquet` (pnpm's Rust port)
pnpm 12.x accepts `@pnpm/pacquet` in `configDependencies`, delegating the **materialization phase** of `pnpm install` to the Rust binary — resolution stays JS. Relevant to you only if `oxide` ever manages dependencies (e.g. `oxide add <module>`). Not a pipeline win today.

### `turbo-test` / `rjest`
- **`turbo-test`** — single self-contained Rust binary, Vitest drop-in, **5–12× faster than vitest+jsdom**. 0.3.x.
- **`rjest`** — Jest-compatible, long-lived Rust daemon caching SWC transforms and pre-warmed Node workers, **~14 ms repeat runs**.

Both attack the same problem: your §22 Playwright/JS-disabled test matrix is slow. Neither is mature enough for the conformance gate, but `turbo-test` is worth a spike for the adapter unit suites.

### Source maps: `oxc_sourcemap` or `srcmap`
`oxc_sourcemap 3.0.0` is your own toolchain's fork of `rust-sourcemap`, with parse/serialize/lookup-table/concat benchmarks. **`srcmap`** (fallow-rs) is the more complete new option: parse, generate, remap, **compose**, full ECMA-426 compliance.

You already generate sourcemaps (`build.sourcemap`), but `remap`/`compose` is what you need for the **plugin-transform chain** — a plugin that rewrites code must remap the incoming map, and right now you likely drop it.

### Content search: `tantivy`
Your §16.5 "generated search indexes" for content collections is the one place you have no Rust component. **`tantivy`** is the Rust Lucene — embedded full-text index, the engine under Meilisearch and Quickwit. It's a *library*, so it embeds in `oxide build` and emits a static index — no server, no JS.

## Tier 3 — watch (not ready)

| Tool | What | Why not yet |
|---|---|---|
| **`Rustwright`** | Playwright rewrite, **70% less memory, 2.55× faster boot** | YC startup, closed beta — cannot depend on it |
| **`OxiBrowser`** | Pure-Rust CDP-compatible headless browser, 697 tests | New; would replace Chromium in your Playwright matrix |
| **`navette`** | Tiny Rust binary driving the **OS WebView** (no Chromium) | Different rendering engine → conformance drift |
| **`voltiq`** | Single Rust binary driving a real browser to capture **Core Web Vitals + root-cause files**, no config | 0.1.0, May 2026 — exactly what §21 benchmarks want, but unproven |
| **`unisights`** | Analytics + web vitals, Rust + WASM, full data ownership | Overlaps your existing `/_oxide/vitals` beacon |

## Don't change

- **`hyper` + `tokio`** — §12 is already on the fastest practical stack. Axum 0.8 vs Actix 4 is a 10–15% RPS difference that isn't worth the rewrite.
- **`lightningcss`** — nothing beats it; it already replaces PostCSS + autoprefixer + cssnano + CSS Modules in one tool at ~100× speed.
- **`rolldown` + `oxc`** — Vite 8 is built on exactly this pair; you're on the same track.
- **Tailwind Oxide** — if you support Tailwind v4, its engine is already Rust+Lightning CSS. You likely get this free through Lightning CSS.

## Concrete additions to your workspace

```toml
# crates/assets
oxvg_optimiser = "0.0.5"          # §6.7 SVG, up to 50× vs SVGO
fontcull = "2.0"                  # §16.2 glyph-usage subsetting (fontations backend)
rimage = { version = "0.12", default-features = false, features = ["resize","oxipng","webp","avif","threads"] }

# crates/compiler (markdown path)
snapfire_fsr_mdx = { version = "0.1", features = ["highlight", "frontmatter"] }

# crates/graph (content search index)
tantivy = "0.25"

# crates/server (source map remap for plugin chains)
srcmap = "*"                      # or oxc_sourcemap 3.0

# CLI dev-tooling (feature-gated, not default)
# oxlint, oxfmt, tsrs, turbo-test — external binaries, not deps
```

## The one strategic note

Notice what's *absent* from this list: there is no mature Rust replacement for **Playwright** or **`web-vitals`**, and the `tsrs`/`tsgo` situation means type checking is the one stage still dominated by Go, not Rust. Your §21 benchmark gate and §22 conformance suite are therefore the two places where the Node/Chromium dependency is hardest to remove — and `voltiq` / `Rustwright` / `OxiBrowser` are the three names to re-check in six months.

Everything else — lint, format, minify, SVG, fonts, images, MDX, search, source maps — can go fully in-process today.
