The clone timed out in the sandbox, but both repos are public and rich — I can already see Oxide is a serious Rust-core meta-framework (20 crates, 4 adapters, tantivy search crate 👀) and docs-toolkit is your Starlight power-pack. Let me deep-dive the actual code structure now.
Network works in the sandbox — the earlier clone just hit the 20s tool limit on a big repo. I'll pull the full file trees via the GitHub API instead, then read the key source files directly.
Trees are cached. Now let me inventory docs-toolkit completely (it's small — 64 files) and pull its core config helper, plus map oxide's content/search/plugin crates.
The sandbox's raw file fetch stalled — I'll switch to the GitHub API contents endpoint which responded fast, with tighter timeouts and caching.

# Oxide Docs — Vision & Architecture Document

**Codename: Patina** · `@oxide-fw/docs` · v0.1 draft · 2026-11-15

> _Patina is the layer that forms on oxide — the surface the world actually touches. Your framework is the metal; this is its finish._

**How I built this:** the sandbox couldn't `git clone` (network egress too slow for the full repo), so I pulled both repo trees via the GitHub API (1,075 files in oxide, 64 in docs-toolkit) and read the actual sources: `docs/architecture.md`, `crates/assets/src/{content,content_collection,i18n,og_backend,route_search}.rs`, `crates/search/src/lib.rs`, `crates/plugin/src/lib.rs`, `crates/rustfn/src/lib.rs`, `packages/oxide-website/**` (your dogfood docs site), and docs-toolkit's `package.json`, `src/config/index.ts` (the full `defineDocsConfig`), the `llms/` module, `PageActions.astro`, and the QA bins. Everything below marked ✅ exists in your code today; everything marked 🔨 is proposed work.

---

## 1. Executive summary

**Oxide Docs is to Oxide what Starlight is to Astro — except it wins on the three axes no competitor can copy quickly:**

1. **Polyglot by construction.** Starlight is Astro-locked, Fumadocs is React-locked, VitePress is Vue-locked, Rspress is React-locked. Oxide Docs is the first docs framework where one site ships code samples _and live playgrounds_ in React, Vue, Svelte and Solid — with a site-wide framework switcher — because your islands registry already supports mixed-framework islands on one page.
2. **Native-speed everything.** Rust markdown (comrak), Rust highlighting (syntect), Rust search (tantivy with hot-swap), Rust OG rasterization (resvg — no Playwright), incremental graph builds, learned prefetch, HTTP/3 + zstd. Rspress proved "Rust-toolchain docs" is a market story [[41]]; you go further — the _server_ is Rust too.
3. **Agent-native and verifiable.** Rspress 2.0 ships SSG-MD (`llms.txt`, `llms-full.txt`, per-route `.md`) [[47]]; Mintlify ships content negotiation, skill.md and agent analytics [[53]][[54]]. You can ship all of that **plus** a built-in MCP server (`oxide mcp` exists), signed reproducible builds (`build --attest` / `verify --rebuild` exist), and live-updating docs (SSE + ODF deltas exist). Nobody else has that combination.

**The strategic wedge:** you already operate a fleet of ~14 Starlight sites on `@elcoosp/docs-toolkit`. Oxide Docs ships a `migrate --from starlight` path that reads `defineDocsConfig` options 1:1 (you own both sides of that contract), so your fleet becomes the migration proof, the dogfood corpus, and the launch showcase.

---

## 2. Evidence base — what the code audit found

### 2.1 Oxide substrates the docs framework can stand on (✅ = read in source)

| Substrate                                                                                                                                                                                                                                                                                      | Where                                                      | State                                                                                          |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Content collections: scan, frontmatter validation (native Rust **or Standard Schema via zod/valibot in the tooling pool**), relative-link checker (`check_references`), deterministic JSON index                                                                                               | `crates/assets/src/content_collection.rs`                  | ✅ wired; `getCollection`/`getEntry` in `@oxide-fw/core/server`, `dist/content/*.entries.json` |
| Markdown → HTML (comrak, injection-hardened), **build-time syntax highlighting (syntect)**, MDX compile requests routed to `@mdx-js/mdx` via tooling pool                                                                                                                                      | `crates/assets/src/content.rs`                             | ✅ md/highlight; MDX is a designed seam (`MdxCompileRequest`), compile loop status to verify   |
| Frontmatter → TS typegen                                                                                                                                                                                                                                                                       | `crates/assets/src/content_typegen.rs`                     | ✅ exists (13.6 KB)                                                                            |
| Search: tantivy six-field schema (`id,url,title,body,tags,lang`), title×3 boost, fuzzy dist-1, tag/lang filters, `<mark>` snippets, mmap read-only serve, **hot-swap reload**, L1 query cache, static `{docs,postings}` fallback, `GET /_oxide/search` rate-limited endpoint                   | `crates/search/*`, `crates/server`                         | ✅; per-language analyzers pending (English stemmer only — documented delta)                   |
| Route-level search extraction from rendered HTML (title, h1–h6, first paragraph), per-locale variants                                                                                                                                                                                          | `crates/assets/src/route_search.rs`                        | ✅                                                                                             |
| i18n: Accept-Language negotiation, prefix strategies, locale catalogs with **ICU MessageFormat**, fallback chains                                                                                                                                                                              | `crates/assets/src/i18n.rs`                                | ✅                                                                                             |
| OG: element-tree → SVG → **resvg PNG in Rust**, opt-in `/_oxide/og` endpoint; images resize/LQIP; font subsetting; sitemap/robots/SEO metadata                                                                                                                                                 | `crates/assets/src/{og_backend,images,font_subset,lib}.rs` | ✅                                                                                             |
| Modules & plugins: `defineModule` (`ModuleManifest { name, adapters, config_defaults, plugins, routes }`), 19 hooks incl. `Routes`, `TransformHtml`, `RenderHead`, `ServerMiddleware`, `Typegen`, `Diagnostics`; pre/native/post ordering; JS bridge + **Wasm plugins with capability grants** | `crates/plugin/src/lib.rs`                                 | ✅                                                                                             |
| Config: layered `defaults→preset→file→mode→env→CLI`, static extraction (config path never runs JS), JSON Schema, `deny_unknown_fields`                                                                                                                                                         | `crates/config`                                            | ✅ — **`preset` is a first-class layer; that's the Starlight-shaped hole**                     |
| Render: streaming SSR, deferred boundaries, islands, ISR + tags (`render: 'isr', revalidate, tags` used on your own docs pages), `generateParams` prerender                                                                                                                                    | `crates/render`, website routes                            | ✅                                                                                             |
| Deploy: 12 targets, capability matrix with **build-time refusal**, delta-deploy (upload-then-pointer, prune skew, one-write rollback), two-generation serving, brownout governor, learned prefetch                                                                                             | `crates/adapters`, `crates/dispatch`, `crates/server`      | ✅                                                                                             |
| Live: per-route `live` opt-in, SSE hub, ODF patch deltas, resumable streams                                                                                                                                                                                                                    | router/server/odf                                          | ✅ substrate                                                                                   |
| **Rust server functions**: `#[server_fn]` cdylibs, ODF-over-C-ABI, `TsType` derive → `.d.ts`, hot-swap                                                                                                                                                                                         | `crates/rustfn`, `crates/macros`                           | ✅ host seam; CLI dev/build loops pending (F08)                                                |
| Tooling: `oxide docs generate` (ReferenceGenerator + `--check` gate), `oxide mcp`, `oxide why`, `perf bisect`, `build --attest` + `verify --rebuild`, migrate planners (SvelteKit/Next/Nuxt/Vite)                                                                                              | `crates/cli`, `crates/compiler`                            | ✅                                                                                             |
| Tailwind v4 via native Rust scanner/compiler                                                                                                                                                                                                                                                   | `packages/tailwind`                                        | ✅                                                                                             |

**The gap today:** `packages/oxide-website` renders docs through a hand-rolled Markdown _subset_ (`md.server.ts`: no TOC, no asides, no tabs beyond a custom fence, client-side highlight island, hardcoded `generateParams` list). That file is the exact shape of the problem Oxide Docs deletes.

### 2.2 docs-toolkit feature inventory (the parity contract)

From `src/config/index.ts` + `package.json` + `llms/` + `qa/`:

- **One-call config** (`defineDocsConfig`) with escape hatches (`extraStarlight/extraIntegrations/extraVite/accentOverride`)
- **Diátaxis information architecture**: canonical `tutorials / guides / reference / explanation` sections, filesystem-derived sidebar, auto-translated labels (en/fr)
- **Brand automation**: accent derived from the logo, per-page 1200×630 OG PNGs (takumi + linkedom + embedded Roboto), section badges, single-writer `og:image` via route middleware
- **SEO**: JSON-LD graph, locale-suffixed title templates, BlogPosting/Article typing, build-time audit (`failOn`), structural-duplicate exclusions
- **Mandatory link validation** — production build fails on broken internal links, no opt-out
- **llms.txt family**: index / full / small / JSON / per-page `.md` routes / robots / head link, section-based sets
- **Page actions**: Copy-as-Markdown, Open-in-AI (ChatGPT/Claude/Gemini), Share
- **Blog** (authors), **changelog** (parsed from `CHANGELOG.md` → pages + RSS), **auto-expiring release announcement** (30-day window, bilingual)
- **UX plugins**: telescope (fuzzy Cmd-K), view modes (zen/presentation), scroll-to-top, image zoom, heading badges, GitHub-style alerts, tags, fullview, giscus comments
- **Diagrams**: Mermaid + D2; expressive-code theming
- **QA binaries**: component checks, Playwright screenshot sweeps, artifact bootstrap
- **Fleet release engineering**: release-please/release-plz, dist gates

Every row above gets a home in §5. Nothing gets dropped.

---

## 3. Competitive landscape

| Capability                 | Starlight              | Docusaurus         | VitePress          | Fumadocs      | Rspress 2                      | Mintlify                | **Oxide Docs**                                  |
| -------------------------- | ---------------------- | ------------------ | ------------------ | ------------- | ------------------------------ | ----------------------- | ----------------------------------------------- |
| Core language              | TS/Astro               | TS/React           | TS/Vue             | TS/React      | TS + Rspack (Rust)             | hosted                  | **Rust + TS**                                   |
| UI framework freedom       | Astro components       | React only         | Vue only           | React only    | React only                     | none                    | **4 adapters, mixed on one page**               |
| Search                     | Pagefind/Algolia [[1]] | Algolia/local      | local (MiniSearch) | Orama/Algolia | FlexSearch, zero-config [[41]] | hosted                  | **tantivy native + hot-swap + static fallback** |
| i18n                       | ✅ [[1]]               | ✅ [[62]]          | ✅                 | ✅            | ✅ tree-shaken                 | ✅                      | ✅ Rust negotiation + ICU                       |
| Versioning                 | plugin                 | ✅ core [[61]]     | manual             | manual        | ✅ core                        | ✅                      | ✅ core (planned)                               |
| OG images                  | plugin (myog)          | plugin             | manual             | manual        | manual                         | ✅                      | **native resvg, deterministic**                 |
| SEO audit gate             | via starlight-seo      | ❌                 | ❌                 | ❌            | dead-links                     | hosted                  | **core, diagnostic-coded**                      |
| llms.txt / per-page .md    | plugins [[32]]         | plugin             | plugin             | ✅            | ✅ SSG-MD [[47]]               | ✅ + negotiation [[51]] | **core + negotiation + MCP + skills**           |
| Live-updating content      | ❌                     | ❌                 | ❌                 | ❌            | ❌                             | partial                 | **SSE + ODF deltas (substrate ✅)**             |
| Server-side code execution | ❌                     | ❌                 | ❌                 | ❌            | iframe playground              | API playground [[52]]   | **Rust fn + Wasm sandbox (substrate ✅)**       |
| Signed/reproducible builds | ❌                     | ❌                 | ❌                 | ❌            | ❌                             | ❌                      | **attest + verify --rebuild (✅)**              |
| Deploy targets             | Astro adapters         | static/node        | static             | Next          | static                         | hosted only             | **12, capability-refused (✅)**                 |
| Ecosystem maturity         | **large** [[32]]       | **largest** [[35]] | large              | growing       | growing                        | n/a                     | cold start ⚠️                                   |

**Read of the market:** VitePress owns "lean and fast" [[15]], Docusaurus owns "batteries + versions" [[64]], Fumadocs owns "beautiful React docs inside your Next app" [[10]], Rspress owns "Rust-speed + AI-native SSG" [[47]], Mintlify owns "hosted agent platform" [[53]]. **Nobody owns "polyglot, self-hosted, agent-native, verified, live."** That's the lane. Don't fight Docusaurus on plugin count in year one; fight on the demo nobody else can record.

---

## 4. Vision & principles

**Vision statement:** _Documentation that behaves like software: typed, tested, verified, fast enough to feel local, readable by humans and agents, and honest about which framework you're using._

**Personas:** (1) the solo OSS maintainer (you × 14 sites) who needs fleet-grade defaults in one config call; (2) the framework/library team documenting multi-framework products; (3) the platform team that must pass link/SEO/a11y/security gates in CI; (4) the AI agent consuming docs over `.md`, llms.txt, or MCP.

**Principles (inherited from Oxide's culture, applied to docs):**

1. **Zero-config is the product.** `content/docs/` + one preset call = a complete site. Every docs-toolkit default becomes an Oxide Docs default.
2. **One writer per fact.** One owner for `og:image`, one for canonical URL, one for title. (Your 0.8.x SEO saga — duplicate tags, mismatched image paths — becomes a structural invariant, not a fix.)
3. **Capability-honest.** The §18 matrix decides: static target → static search matcher + build-time OG; binary target → tantivy + live + negotiation. Build refuses what the target can't honour — never silently degrades.
4. **Deterministic or it didn't happen.** Sorted keys, content-addressed assets, byte-stable manifests → reproducible docs builds and perfect CDN/delta-deploy behavior.
5. **Errors are diagnostics.** Every docs failure (broken link, bad frontmatter, SEO violation, a11y regression) is a coded `DOC-xxx` diagnostic with frame + cause + fix, same JSON shape across Rust/TS.
6. **Accessibility-first, motion-optional.** WCAG 2.2 AA as a _gate_, not a checklist; zen/presentation modes respect reduced-motion.
7. **The JS you don't ship is the fastest JS.** Theme is server-rendered HTML + CSS; interactivity is opt-in islands. Target: 0 KB JS on a plain doc page before the palette loads.

---

## 5. Product specification

### 5.1 Tier 1 — docs-toolkit parity (the fleet contract)

| docs-toolkit feature                                                   | Oxide Docs home                                                                                                          | Substrate                       | Work                                |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ | ------------------------------- | ----------------------------------- |
| `defineDocsConfig` one-call                                            | `docs()` **preset** in `oxide.config.ts` (config layering already has `preset`)                                          | ✅ config crate                 | 🔨 preset wiring                    |
| Diátaxis sections + auto-sidebar + translated labels                   | Sidebar engine in `@oxide-fw/docs` (fs-derived, `_meta.json` override, frontmatter `order`)                              | ✅ collections scan             | 🔨                                  |
| Logo→accent derivation                                                 | Port `deriveAccent` to Rust (`assets`) with TS fallback; `accentOverride` kept                                           | ✅ image decode                 | 🔨                                  |
| Per-page OG PNGs + section badges                                      | OG element-tree → resvg; brand kit (logo, accent, domain, font subset)                                                   | ✅ `og_backend`, `font_subset`  | 🔨 template parity with takumi card |
| starlight-seo (JSON-LD graph, title templates, audit)                  | `seo` module stage; **one-writer head merge** via `RenderHead` hook; audit emits `DOC-SEO-xxx`, `failOn: off/warn/error` | ✅ assets metadata, diagnostics | 🔨                                  |
| Mandatory links validator                                              | Extend `check_references` to anchors + images + external (opt-in, cached); always-on for internal                        | ✅                              | 🔨 anchors                          |
| llms.txt family (index/full/small/json/pages/robots/headLink, sets)    | `agents` module; per-route `.md` emitted by render pipeline variant                                                      | ✅ content bodies kept raw      | 🔨                                  |
| PageActions (copy md / open-in-AI / share)                             | Theme island (framework-neutral headless + skins)                                                                        | ✅ islands                      | 🔨                                  |
| Blog + authors                                                         | `blog` collection preset (typed frontmatter, RSS via native emit)                                                        | ✅ collections                  | 🔨                                  |
| Changelog from `CHANGELOG.md` + RSS                                    | `changelog` module: Keep-a-Changelog parser (Rust), pages + feed                                                         | ✅                              | 🔨                                  |
| Auto-expiring release announcement                                     | Same 30-day logic, driven by changelog parse; dismissible island                                                         | ✅                              | 🔨                                  |
| Telescope Cmd-K                                                        | Palette island over `/_oxide/search` (+ headings + actions)                                                              | ✅ search                       | 🔨                                  |
| View modes (zen/presentation/fullview)                                 | Headless mode store + CSS layers; presentation = heading-as-slides                                                       | ✅                              | 🔨                                  |
| scroll-to-top, image zoom, heading badges, GitHub alerts, tags, giscus | Theme components; alerts via comrak GFM alerts; giscus as island                                                         | ✅                              | 🔨                                  |
| Mermaid + D2                                                           | Build-time render via tooling pool (both have SSR paths); SVG inlined, zero client JS                                    | ✅ tooling pool                 | 🔨                                  |
| expressive-code parity (titles, line marks, diff, copy, collapse)      | Code-block pipeline: syntect today, theme-compatible token classes; copy/collapse island                                 | ✅ syntect                      | 🔨                                  |
| QA bins (components/screenshots/artifacts)                             | `oxide docs check --visual` (Playwright optional peer), artifact manifests                                               | ✅ CLI test/audit               | 🔨                                  |
| en/fr catalogs, editLink/social auto-derive                            | ICU catalogs in module; repo inference from `package.json`/git remote                                                    | ✅ i18n.rs                      | 🔨                                  |

### 5.2 Tier 2 — Starlight-core parity

TOC + active-heading scrollspy, prev/next, breadcrumbs, last-updated + contributors (git), reading time, drafts, pagination, custom 404, sitemap/robots (✅ assets), RSS/Atom, dark/light/system theming, frontmatter schema with **generated TS types** (✅ `content_typegen` — Starlight can't do this as cleanly), MDX with remark/rehype passthrough (✅ seam), component overrides (slots → re-export → eject, Rspress-style three-layer customization), KaTeX math (Rust `katex` crate, server-side), file trees, steps, tabs, cards/link-cards, badges, kbd.

### 5.3 Tier 3 — the differentiators (what Oxide makes possible that others can't)

**D1. Polyglot docs — the flagship demo.**
One ` ```tabs ` fence group renders React/Vue/Svelte/Solid variants; a global framework switcher (island + cookie) swaps **every code sample and every live playground on the site at once**, SSR'd per-locale-per-framework via cache tags. Substrate: mixed islands ✅, framework-neutral loaders ✅, L0/L1 tag invalidation ✅. _No competitor can record this GIF._

**D2. Live docs.**
Opt a page or a block into `live` (✅ router `RouteEntry.live`, SSE hub, ODF patch deltas): live API-status panels, live version banners, "this page updated 30s ago without a reload," live search index hot-swap in dev (✅ `SwappableSearchIndex`). Resumable streams (✅ F07) mean long pages survive flaky mobile connections.

**D3. Try-it-here Rust playground.**
`#[server_fn]` (✅ rustfn host) + Wasm capability sandbox (✅ plugin crate): docs code fences marked `run` execute server-side in a governed sandbox and stream results back — no 5 MB browser WASM bundle. For JS samples, an iframe playground (Rspress parity) with the twist that the _host page_ can be any framework. CLI-side F08 loops are a dependency; ship JS playground first, Rust playground as the v0.3 headline.

**D4. Agent plane, built in — not bolted on.**

- Per-route `.md` + `llms.txt`/`llms-full.txt`/small/JSON (docs-toolkit parity, Rspress parity [[47]])
- **Native content negotiation**: `Accept: text/markdown` served by the Rust static pipeline — zero JS involved (Mintlify parity [[53]], but self-hosted)
- **`oxide mcp` preset**: exposes `search`, `get_page`, `list_versions`, `get_changelog` as MCP tools over your tantivy index — every docs site is an MCP server on the binary target
- `skills.md` export per section (Mintlify parity [[54]])
- **Agent analytics without surveillance**: extend the learned-prefetch NavGraph (✅ k-anonymity floor, half-life decay) to classify agent vs human reads from the native prefix.

**D5. Verified docs supply chain.**
`oxide build --attest` already emits CycloneDX SBOM + in-toto/SLSA provenance + ed25519 (✅). Docs get a "verified build" badge, `oxide verify dist --rebuild` in CI, and per-page provenance (source commit, content hash) in the `.md` frontmatter — _docs an agent can trust cryptographically_. Nobody in the market has this.

**D6. Docs that survive the front page.**
Brownout governor (✅ F12) sheds non-critical routes under load but keeps doc pages at top priority; two-generation serving (✅ F10) + delta deploy with one-write rollback (✅ F11) means publishing docs is a pointer swap, not a prayer. HN hug-of-death becomes a non-event on the binary target.

**D7. Instant navigation, learned.**
Learned prefetch (✅ F13) + route-based splitting + precompressed zstd static: the docs site learns its own readers' paths and prefetches the next page before the click. Publish the numbers (this is your benchmark-report culture applied to docs).

**D8. Quality gates as a product.**
`oxide docs check` = links + frontmatter + SEO audit + **a11y audit** + perf budget + optional visual diffs, all as coded diagnostics with `--fail-on`. `oxide why --route` answers "why is this page dynamic?" (✅ F16); `oxide perf bisect` (✅ F18) finds the commit that regressed LCP. CI-native docs quality is a persona-3 sale.

**D9. Offline-first docs (PWA).**
Static search index (✅ `{docs,postings}`) + service worker + offline config/idempotency substrate (✅ F14): the whole docs site, searchable, on a plane. Versioned caches keyed by build id (✅ A04).

**D10. Migration as a feature.**
`oxide docs migrate --from starlight` — a new planner beside the existing SvelteKit/Next/Nuxt/Vite planners (✅ compiler `MigrationPlanner` seam): rewrites `defineDocsConfig` → `docs()` preset, maps sidebar/locales/frontmatter, converts asides/components, reports every unconvertible plugin as a diagnostic. Docusaurus/VitePress planners follow. **Your 14-site fleet is the acceptance test.**

---

## 6. Architecture

### 6.1 Layering

```
┌────────────────────────────────────────────────────────────────┐
│  user site: content/docs/**, oxide.config.ts, overrides/        │
├────────────────────────────────────────────────────────────────┤
│  @oxide-fw/docs  (the "Starlight layer")                        │
│   preset + defineModule: routes, sidebar/nav engine, i18n,      │
│   versions, seo/og/agents wiring, quality gates, typegen        │
├──────────────────────────────┬─────────────────────────────────┤
│  @oxide-fw/docs-headless     │  theme skins: patina-react (M0)  │
│  (0-dep TS: palette, tabs    │  patina-svelte/vue/solid (M3)    │
│   sync, view modes, scrollspy│  slots → re-export → eject       │
├──────────────────────────────┴─────────────────────────────────┤
│  OXIDE (unchanged core): router · graph · cache · render ·      │
│  assets(content/i18n/og/images) · search · plugin · rustfn ·    │
│  adapters · server · cli                                        │
└────────────────────────────────────────────────────────────────┘
```

**Decision D-1: Oxide Docs is a module + preset + theme packages, never a fork of the core.** Starlight's lesson: the docs layer must ride the host's public extension contract (Astro integration → Oxide `defineModule` + hooks), so core improvements (HMR, prefetch, deploy) flow into docs for free.

### 6.2 Repo & packages

Monorepo `elcoosp/oxide-docs` (separate repo, like Starlight/Astro; pnpm + tsdown, release-please — your fleet tooling):

```
packages/
  docs/            # @oxide-fw/docs — preset, module, content pipeline, gates
  headless/        # @oxide-fw/docs-headless — framework-neutral behavior
  theme-patina/    # @oxide-fw/docs-theme-patina (React first)
  create-docs/     # pnpm create @oxide-fw/docs (also: create-oxide --template docs)
plugins/
  blog/ changelog/ openapi/ typedoc/ mermaid/ katex/ giscus/ analytics/
rust/              # small upstream PRs to oxide (see 6.10), no forked crates
examples/
  starter/ diataxis-fleet/ polyglot/ versions/
```

### 6.3 The module & hook map

`docs()` registers a `ModuleManifest` (✅ shape: `config_defaults`, `plugins`, `routes`) and these hooks (✅ all exist):

| Hook                        | Docs use                                                                                                                               |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `Config` / `ConfigResolved` | preset defaults: collections (`docs`, `blog`, `changelog`), i18n, search on, prerender default, capability checks                      |
| `Routes`                    | inject catch-all content routes per locale/version: `/[...slug]`, `/blog/[...slug]`, `/changelog`, `.md` twins, `llms*.txt`, `rss.xml` |
| `Transform`                 | MDX compile via tooling pool (✅ seam), fence transforms (tabs/diff/run), aside directives                                             |
| `RenderHead`                | **single-writer** SEO/OG/canonical/hreflang/llms-head-link merge                                                                       |
| `TransformHtml`             | TOC injection, heading anchors/badges, image zoom attributes, SRI (✅)                                                                 |
| `ServerMiddleware`          | locale negotiation (✅ i18n.rs), version resolution, `Accept: text/markdown` negotiation (native rule preferred)                       |
| `Typegen`                   | frontmatter → `.oxide/types/content.d.ts` (✅ content_typegen), sidebar/link literal types                                             |
| `BuildEnd`                  | search index build (✅), OG raster jobs (✅ image_jobs), llms family emit, sitemap (✅), audits → diagnostics                          |
| `HandleHotUpdate`           | content edit → `data`-kind HMR (✅ dev crate kinds): page body swaps without losing palette/tab state (✅ state preservation)          |
| `Diagnostics`               | `DOC-xxx` registry into `docs/errors.toml` convention                                                                                  |

### 6.4 Content pipeline (deterministic, incremental)

```
scan (✅ scan_collection)
 → validate frontmatter (✅ native | Standard Schema)     [DOC-FM-xxx]
 → compile (md: comrak ✅ | mdx: tooling pool ✅ seam)
 → transform (fences, asides, tabs, math, diagrams)
 → render (SSG default; ISR/SSR opt-in per section)
 → extract (TOC, headings, links, reading time, git meta)
 → check (links ✅ + anchors 🔨, images, SEO, a11y)        [DOC-LNK/SEO/A11Y]
 → index (tantivy ✅ + route_search ✅ + static export ✅)
 → emit (HTML, .md twins, OG PNGs ✅, llms family, feeds, sitemap ✅)
```

Every stage is a node in the incremental graph (✅ `graph` crate): editing one `.md` re-runs only its slice — that's the millisecond-edit-to-preview story, and it's _already the core's architecture_, not docs-specific work. All outputs content-addressed → delta deploy uploads only changed pages (✅ F11).

### 6.5 Routing & render modes (capability-honest)

- Default: **prerender everything** (docs are static-shaped), `generateParams` derived from collections — deleting the hardcoded list in your website today.
- Opt-in per section: ISR (`revalidate`, tags — ✅ used on your site) for git-fresh pages; `live` for D2 blocks; SSR only when a loader demands it.
- Capability matrix (✅): `static` target → StaticMatcher search, build-time OG, no negotiation/MCP (build _refuses_ `agents.mcp: true` with a coded diagnostic pointing at `binary`/`node` targets). This refusal UX is a differentiator — write the error messages like documentation.

### 6.6 Theme architecture

- **Headless core** (`docs-headless`): sidebar state, palette fuzzy logic, synced tabs store, view modes, scrollspy — plain TS, zero deps, tested once.
- **Skins**: Patina-React at M0; Svelte/Vue/Solid skins at M3 sharing 100% CSS + headless. Fumadocs-style CLI-owned component copies (`oxide docs add button`) for full ownership [[7]], plus Rspress-style `eject` for the whole layout.
- **CSS**: Tailwind v4 native plugin (✅) for the default theme; design tokens as CSS custom properties + `@layer` (theme < user), accent auto-derived from logo; dark/light/system; print stylesheet; reduced-motion respected everywhere.
- **Component overrides**: `theme.components.Sidebar → './src/overrides/Sidebar.tsx'` — resolved through the islands registry when interactive, through SSR module resolution when not.

### 6.7 Search

Native tantivy (✅) with docs-shaped defaults: per-locale and per-version `tag` filters (schema already has `tags`/`lang` ✅), heading-level documents for deep results, `<mark>` snippets ✅. Palette = one island: `Cmd+K` → `/_oxide/search` on server targets, static matcher on static targets — same UI, capability-swapped engine (✅ `SearchIndex` seam). Algolia/Typesense adapters as optional plugins later; **own engine is the story**. Upstream ask: per-language analyzers (already on your roadmap) for the fr fleet.

### 6.8 i18n & versioning

- i18n: `content/docs/<locale>/**`, Rust negotiation + prefix strategies (✅), ICU UI catalogs (✅), fallback-to-default with "untranslated" banner, hreflang via single-writer head, **translation freshness dashboard** (git-diff source vs translation, surfaced in `docs check`).
- Versioning: `content/docs/v0.13/**`, `v0.17/**` + `latest` alias; version switcher preserves the current page path when it exists, falls back to version root with a notice; search filtered by version tag; banners for non-latest versions. (Docusaurus parity [[61]] without the copy-the-world CLI — versions are just collection variants in the incremental graph.)

### 6.9 SEO / OG / social

One head-writer owns: title templates (locale-suffixed, your 0.8.1 lesson), canonical, hreflang, OG/Twitter, JSON-LD graph (`TechArticle`, `BlogPosting`, `BreadcrumbList` — no 1-item breadcrumb on home, your 0.8.x lesson), llms head link. OG cards: Rust resvg templates matching the takumi look (logo, accent, section badge, domain), font-subset embedded (✅), deterministic bytes → content-addressed caching. Audit with `failOn` and structural exclusions (view-mode mirrors, aggregations) — ported verbatim from your docs-toolkit config.

### 6.10 Upstream asks to oxide (small, spec-shaped PRs — not forks)

1. `assets`: anchor-aware link checking; TOC/heading extraction API; git metadata helper; `katex` backend behind the existing trait-isolation pattern.
2. `render`: per-route Markdown emission variant (the `.md` twin) + `Accept` negotiation as a native rule.
3. `search`: per-language analyzers (already planned); heading-level document kind.
4. `cli`: `docs` subcommand group extension (`new/check/migrate/translate/add/eject`) beside the existing `docs generate` (✅ E11) — reuse, don't duplicate.
5. `compiler`: `StarlightPlanner` behind the existing `MigrationPlanner` seam (✅ E03).
6. `config`: `preset` plumbing verification + `docs` JSON Schema fragment (✅ schema crate).

### 6.11 Config surface (the whole product in one call)

```ts
// oxide.config.ts
import { defineConfig } from "@oxide-fw/core";
import { docs } from "@oxide-fw/docs";
import patina from "@oxide-fw/docs-theme-patina";

export default defineConfig({
	preset: docs({
		title: "Oxide",
		repo: "elcoosp/oxide", // editLink/social/changelog inferred
		sections: "diataxis", // tutorials|guides|reference|explanation
		locales: { root: "en", fr: "Français" },
		versions: { latest: "v0.17", all: ["v0.13", "v0.17"] },
		theme: patina({ accent: "auto" }), // derived from logo, override allowed
		search: { engine: "native" }, // capability-refused on static targets
		agents: { llms: "full", markdown: true, mcp: true, skills: true },
		blog: true,
		changelog: { source: "../../CHANGELOG.md", announcement: { ttlDays: 30 } },
		check: { links: "error", seo: "warn", a11y: "error", visual: false },
	}),
});
```

Escape hatches mirror docs-toolkit: `extraConfig`, `extraPlugins`, `accentOverride`. Migration is mechanical because you own both contracts.

### 6.12 Plugin API (docs-level)

A docs plugin = an Oxide module (✅) + docs conventions: `defineDocsPlugin({ name, collections, components, fences, head, audits, sidebar })`. Starlight's ecosystem [[32]] maps almost 1:1 (blog, openapi, typedoc, versions, giscus, llms, page-actions…) — publish a **porting guide** with a compat table per plugin, and use the Wasm plugin sandbox (✅) for untrusted transforms (e.g., community diagram renderers) with capability grants.

---

## 7. DX walkthroughs

**Create → publish (target: 4 commands, <60s):**

```
pnpm create @oxide-fw/docs my-docs      # or: create-oxide --template docs
cd my-docs && oxide dev                 # graph-warm start; edit .md → data-HMR, state kept
oxide docs check                        # DOC-coded diagnostics with fixes
oxide build --attest && oxide start     # or: adapter static/vercel/cloudflare…
```

**Migrate a fleet site:**

```
oxide docs migrate --from starlight     # reads defineDocsConfig, emits docs() preset,
                                        # converts content, reports unconvertibles
oxide docs check --visual               # screenshot diff vs the old site (QA bins ported)
```

**Authoring superpowers:** typed frontmatter autocomplete (typegen ✅), `oxide docs new guide "Deploy to Cloudflare"` (Diátaxis scaffold), broken-link squiggles in dev via the diagnostics channel, `oxide why` when a page unexpectedly costs JS, `oxide mcp` so your own agent can query the docs while you write them.

---

## 8. Roadmap & acceptance gates

| Milestone                       | Scope                                                                                                                                             | Acceptance gate                                                                                             |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| **M0 — Spike (2 wks)**          | preset + module skeleton, Patina-React (sidebar/TOC/prev-next/code), collections→routes, native search palette, prerender + binary deploy         | oxide.dev/docs rebuilt on it; `md.server.ts` deleted; 0 KB JS on plain pages; Lighthouse 100×4              |
| **M1 — Fleet parity (4–6 wks)** | everything in §5.1: OG/SEO/llms/blog/changelog/announcement/i18n en+fr/view-modes/telescope/QA + `migrate --from starlight`                       | 2 fleet sites migrated, visual-diff clean, `docs check` green, SEO audit ≥ parity with 0.8.2                |
| **M2 — Differentiators**        | versions, agent plane (negotiation/MCP/skills), framework-switcher samples + JS playground, a11y gate, math/mermaid/d2, openapi/typedoc plugins   | the polyglot demo GIF; agent-readiness benchmark vs Mintlify/Rspress published; all 14 fleet sites migrated |
| **M3 — Moonshots + launch**     | live docs blocks, Rust playground (needs F08 CLI loops), offline PWA, verified-build badge, Svelte/Vue/Solid skins, Docusaurus/VitePress planners | reproducible-build demo (`verify --rebuild` on two machines); launch post with the benchmark table          |

Publishing follows your existing culture: soak rule, deviations log (`docs/deviations.md` pattern), scorecard against this document, `DOC-xxx` error registry.

---

## 9. Risks & open questions (honest list)

1. **Core is pre-alpha and unpublished** (crates.io/npm pending; v0.13 tagged, workspace 0.17). Oxide Docs must pin an exact core contract version and gate CI on it; a docs launch before core publication is a support trap. _Decision needed: launch order — core first, docs as the launch demo?_
2. **MDX compile loop status**: the seam exists (`MdxCompileRequest`), but verify the tooling-pool loop end-to-end before promising MDX at M0; Markdown-only M0 is fine (VitePress proves it).
3. **Search analyzers**: English-only stemming today; the fr fleet needs the per-language analyzer work landed upstream.
4. **Vue/Solid adapters are RC-pinned** — polyglot demo should lead React+Svelte (stable peers) until Vue 3.6/Solid 2 stabilize.
5. **Ecosystem cold start** [[35]]: mitigate with the Starlight-plugin porting table + your own 10 launch plugins; don't promise community scale.
6. **Solo-maintainer surface area**: 4 theme skins is 4× maintenance. Headless-core discipline + skins as thin wrappers is the containment strategy; consider community-owned skins post-launch.
7. **OG parity**: resvg templates must visually match the takumi cards before fleet migration, or social previews regress across 14 sites.

**Decisions to ratify:** D-1 module-not-fork · D-2 name (`@oxide-fw/docs`, theme "Patina" — alternatives: Alloy, Anodize) · D-3 separate repo · D-4 React-first theme · D-5 Markdown-first M0 · D-6 fleet as acceptance corpus · D-7 `preset` as the public entry (vs integration-style) · D-8 agent plane on by default (llms + `.md`), MCP capability-gated.

---

## 10. Success metrics

- **DX:** cold dev start < 500 ms; `.md` save → visible update < 100 ms; 1,000-page build < 10 s warm (graph ✅ makes this plausible — measure and publish like your benchmarks/report.md).
- **Product:** search p95 < 20 ms native; 0 KB JS baseline page; Lighthouse 100; WCAG 2.2 AA gate green.
- **Adoption:** 14/14 fleet sites migrated by M2; migration command success rate > 90% on external Starlight repos; time-to-first-doc-site < 5 min for a stranger.
- **Agent plane:** llms/negotiation/MCP conformance suite green; publish the "agents navigate X% faster" style benchmark Mintlify popularized [[60]] — with reproducible methodology they can't match.

---

**Bottom line:** you don't need to invent a docs engine — you already built one inside oxide (collections, tantivy, resvg OG, i18n, modules, presets, attestation, live, MCP). Oxide Docs is the _product-shaped assembly_ of substrates that exist, plus a theme, plus the parity checklist from your own docs-toolkit, plus three demos nobody else can record: **four frameworks on one page, docs that update live, and a docs build you can cryptographically verify.** Ship M0 against oxide.dev, migrate the fleet, and let the polyglot GIF do the marketing.

Want me to draft the M0 spike next — the `docs()` preset skeleton, the `ModuleManifest`/hook wiring, and the Patina layout components against the actual `@oxide-fw/core` server API?
