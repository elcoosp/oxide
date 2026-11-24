<!-- GENERATED FILE — `oxide docs generate` (spec v0.2 §V2-E11). DO NOT EDIT.
Regenerate: cargo run -p oxide-fw-cli -- docs generate
Drift gate: crates/cli/tests/docs_tdd.rs -->

# `oxide.config.ts` reference

Generated from `oxide_fw_config::config_schema()` — the §19.3 JSON Schema editors consume via `$schema`. One row per key; the `default` column shows the schema default when one exists.
A key without a description refuses the render (`DOC-001`) — the E11 acceptance: adding a config key without a schema description fails `docs-check`.

## Keys

### `adapter`

- `adapter` — `enum: `"binary"` \| `"node"` \| `"static"` \| `"docker"` \| `"cloudflare"` \| `"lambda"` \| `"vercel"` \| `"netlify"` \| `"deno"` \| `"bun"`` — default —: §18: the deploy target the build emits for; the §18 capability matrix decides what that target honours (the config-key refusal refuses a build the target cannot serve).
### `brownout`

- `brownout` — `object` — default —: V2-F12: brownout mode. Under overload the governor degrades deliberately - stale content past the SWR window behind warning 110, low-priority routes shed with 503 + x-oxide-brownout, readiness flips to 503 at Emergency. Off by default - with enabled false no governor is constructed. Validation (DSP-113): ascending threshold ladders, a staleCeiling multiplier >= 1, hedge.afterFactor > 1.
- `brownout.enabled` — `boolean` — default `false`: Master switch; false keeps the request path at its pre-F12 shape.
- `brownout.hedge` — `object` — default —: The opt-in hedger: duplicate a still-running idempotent GET render after afterFactor x the route's recent p95; first response wins. Idempotency is the user's responsibility.
- `brownout.hedge.afterFactor` — `number` — default `2.0`: Hedge after this multiple of the recent p95 (DSP-113 rejects <= 1).
- `brownout.hedge.enabled` — `boolean` — default `false`: Off by default.
- `brownout.staleCeiling` — `string` — default `"10x"`: How far past the TTL an L0 entry may serve at level Stale, as a multiplier: '10x', plain '10', or fractional '1.5x' (DSP-113 rejects < 1).
- `brownout.thresholds` — `object` — default —: The three breach ladders; rung i is the level-(i+1) threshold, a signal at or above it breaches that level. Ladders must be ascending (DSP-113).
- `brownout.thresholds.loopLagMs` — `integer[]` — default `[20,100,300]`: Event-loop lag rungs in ms (spec defaults).
- `brownout.thresholds.queueWaitMs` — `integer[]` — default `[50,200,500]`: Pool queue-wait p99 rungs in ms (spec defaults).
- `brownout.thresholds.rssMb` — `integer[]` — default `[0,0,0]`: Resident-memory rungs in MiB; 0 disables the RSS gate (the spec default is never).
### `budgets`

- `budgets.clientJs` — `string` — default —: §21: the per-entry client JS budget (e.g. `170kb`); `oxide analyze` fails when the bundle exceeds it.
- `budgets.routeJs` — `string` — default —: §21: the per-route JS budget (e.g. `90kb`).
### `build`

- `build.edgeExternals` — `string[]` — default —: §18 / D18: specifiers left external in the edge bundle. Empty by default — the Worker must be self-contained unless it declares a node-compat layer.
- `build.remoteCache` — `string` — default —: §5.4: remote build-cache URL (http://). Read-through on a local miss, write-back on store.
- `build.routeSearch` — `boolean` — default `true`: §16.5: emit the route-search index (dist/client/search-index.json). On by default when prerendered pages exist; set false for projects that do not ship a client-side search UI.
- `build.sourcemap` — `enum: `"hidden"` \| `"inline"` \| `"external"` \| `"off"`` — default —: §6: the sourcemap mode the client bundle emits (`hidden` ships the map without linking it).
- `build.ssrExternals` — `string[]` — default —: §6: packages kept external in the SSR bundle and required at runtime (the CLI fills a framework default when unset).
- `build.target` — `string` — default —: §6: the browser target the client bundle compiles for (e.g. `baseline-widely-available`).
- `build.writeBranches` — `string[]` — default —: §5.4 / SPEC-5D: git branches trusted to write the shared remote cache. Default main/master + release/* when remoteCache is set.
### `cache`

- `cache.coverageMetrics` — `boolean` — default `false`: V2-F46: the §20 scrape's coverage arm — the hot-key ledger's coverage exposition (the unattributed score's counter, the occupancy and bound gauges) renders beside the labeled window family when the operator opts in. Precedence: the --cache-plane-coverage flag wins, the OXIDE_CACHE_PLANE_COVERAGE env var keeps the operator's spelling, this key is the project's baseline, and the default-off shape answers byte-identically with none set (the F40 doctrine).
- `cache.handler.bucket` — `string` — default —: Bucket name for `type: "s3"`.
- `cache.handler.type` — `enum: `"memory"` \| `"disk"` \| `"redis"` \| `"s3"` \| `"http"`` — default —: §5.3: the L2 cache-handler backend the build wires (memory/disk are local; redis/s3/http are remote).
- `cache.handler.url` — `string` — default —: Remote base URL for `type: "http"` or `type: "redis"`.
- `cache.healthMetrics` — `boolean` — default `false`: V2-F48: the §20 scrape's health arm — the posture pull's F32 gauge exposition (TieredCache::health() rendered as the nine oxide_cache_plane_* gauge families: the memory tier's occupancy and cap, the disk/remote attachment flags, the tag-index pressure, the observability seam's own state) renders after the labeled window and coverage families when the operator opts in. Precedence: the --cache-plane-health flag wins, the OXIDE_CACHE_PLANE_HEALTH env var keeps the operator's spelling, this key is the project's baseline, and the default-off shape answers byte-identically with none set (the F40 doctrine).
- `cache.hotKeyTop` — `integer` — default —: V2-F43: the §20 scrape's hot-key top window — how many labeled oxide_cache_plane_hot_key_total rows one scrape renders beside the exporter's families. Precedence: the --cache-hot-key-top flag wins, the OXIDE_CACHE_PLANE_HOT_KEY_TOP env var keeps the operator's spelling, this key is the project's baseline, the F40 default (10) answers with none set. 0 renders the bare family pair.
- `cache.l0.maxSize` — `string` — default `"512mb"`: §5.3: the L0 in-process cache budget (`kb`/`mb`/`gb`); over-cap entries are GC'd (SPEC-5D).
- `cache.l0.ttlSecs` — `integer` — default —: P-05: L0 freshness window in seconds (default 300).
- `cache.l0.vary` — `string[]` — default —: Request headers the L0 fingerprint varies on. The primary language tag of accept-language is normalized automatically.
- `cache.planeMount` — `boolean` — default `false`: V2-F59: the F57 cache plane's MOUNT — the boot composes HttpCacheService over its own tier, so the §11.2 shared plane answers /entry/, /tag/, /invalidate/ and (the tier doubling as the F58 provider) GET /health on the site's own socket; a second node's cache.handler = { type: 'http', url } points at the mount. Precedence: the --cache-plane-mount flag wins, the OXIDE_CACHE_PLANE_MOUNT env var keeps the operator's spelling, this key is the project's baseline, and the default-off shape answers byte-identically with none set (the F40 doctrine — the plane answers only where the boot opts in). Both spellings are legal: true mounts the plane, false pins it off.
- `cache.postureProbePath` — `string` — default `"/-/health"`: V2-F50: the F49 posture probe's surface location — the GET /-/health JSON pull (the live CacheHealth value, the F48 handoff's recorded tail). Precedence: the --cache-posture-path flag wins, the OXIDE_CACHE_POSTURE_PATH env var keeps the operator's spelling, this key is the project's baseline, and the recorded default (/-/health) answers with none set. A /-led path relocates the probe; the empty string pins the surface off (the ready-path shape); garbage degrades loudly to the recorded default.
### `check`

- `check.typecheck` — `enum: `"tsgo"` \| `"oxlint"` \| `"tsrs"` \| `"tsc"` \| `"off"`` — default —: V2-C11: the typecheck backend `oxide check` runs. `tsgo` (the default) runs `tsgo --noEmit`; `oxlint` runs `oxlint --type-aware --type-check` (it orchestrates the Go-based tsgolint — `oxide doctor` says so); `tsrs` is opt-in and experimental; `tsc` runs plain `tsc --noEmit`; `off` skips the typecheck arm. A bad spelling fails the load with CFG-CHECK.
### `content`

- `content.collections[].dir` — `string` — default —: §16.5: the content directory (Markdown + typed frontmatter) the collection maps to.
- `content.collections[].indexedFields` — `string[]` — default —: §16.5: the frontmatter fields the per-collection static JSON index includes.
- `content.collections[].name` — `string` — default —: §16.5: the collection name (also the static JSON index file's stem).
### `deploy`

- `deploy` — `object` — default —: V2-F10/F11: the deploy machinery's knobs. Wired: the serving pools boot from dist/ per generation (the F10 CLI adoption) and `oxide deploy` reads both knobs over the delta/objstore engines (F11); the engine-side default stays the plumbing default and this section is its config face. A window literal that does not parse, a zero window or a zero generation count fails the load (CFG-DEPLOY).
- `deploy.generations` — `integer` — default `3`: How many past generations stay roll-back-able (the F11 releases/ retention; the spec's N=3 default). CFG-DEPLOY refuses zero.
- `deploy.skewWindow` — `string` — default `"15m"`: How long a demoted generation keeps serving after the promote — the duration grammar <n>s\|m\|h\|d, plain integers are seconds ('15m' is the spec default). Parsed by parse_duration_secs and validated at load (CFG-DEPLOY); a zero window is refused.
### `dev`

- `dev` — `object` — default —: §13.5: dev-server options (https, host, qr, open, proxy).
### `environments`

- `environments` — `object` — default —: §14.2: per-environment Rolldown overrides (client / ssr).
### `experiments`

- `experiments` — `array` — default —: V2-F04 / §15.4: A/B experiments evaluated natively per request before the L0 lookup, so an experiment is a cache-key dimension (the \|exp: vary suffix) instead of forcing the route dynamic. Validation (DSP-110): traffic is a 0-100 percentage and each experiment needs at least two variants. Off by default - with no experiments configured every L0 key stays byte-identical to the pre-F04 baseline.
### `experiments[]`

- `experiments[].id` — `string` — default —: The experiment id - also the locals key and the vary-suffix name.
- `experiments[].routes` — `string[]` — default —: URL-pattern prefixes the experiment applies to; empty = all routes.
- `experiments[].sticky` — `any` — default —: V2-F04: how an eligible identity sticks to its variant — a cookie, a request header, or a hash of the peer IP.
- `experiments[].traffic` — `integer` — default —: Percentage of eligible identities in the experiment (DSP-110).
- `experiments[].variants` — `string[]` — default —: Variant names, in config order (DSP-110: at least two).
### `extends`

- `extends` — `string[]` — default —: §14.2: shared configs to merge before this file.
### `fonts`

- `fonts.families[].family` — `string` — default —: §16.2: the CSS family name the emitted @font-face declares.
- `fonts.families[].localPaths` — `string[]` — default —: §16.2: local font files used instead of a provider download.
- `fonts.families[].provider` — `string` — default —: §16.2: the provider this family downloads from (one of `fonts.providers`).
- `fonts.families[].styles` — `string[]` — default —: §16.2: the styles to fetch (normal, italic).
- `fonts.families[].weights` — `integer[]` — default —: §16.2: the weights to fetch (e.g. 400, 700).
- `fonts.providers` — `string[]` — default —: §16.2: the font providers families may reference.
- `fonts.subset` — `string[]` — default —: §16.2 / C08: the subsetting strategy — `latin`, `unicode-range`, or the glyph-usage spelling `used` (sound only when every route is `static`; otherwise the build refuses with AST-FNT-001).
### `framework`

- `framework.adapter` — `enum: `"react"` \| `"vue"` \| `"svelte"` \| `"solid"`` — default —: §7: the component dialect the adapter compiles (the loader in `page.server.ts` stays framework-neutral).
- `framework.autoIslands` — `boolean` — default —: V2-F02: the auto-islands gate. When true the build runs the compiler's interactivity scan over the client tree, writes .oxide/island-manifest.json and hydrates exactly the components the scanner found interactive; an explicit islands registration always wins and a pure classification is the only skip. Default false — the spec defers by default because a wrong inference silently breaks interactivity.
- `framework.compiler` — `boolean` — default —: C01: the native compiler pass for the adapter. true prefers the native backend when its toolchain is healthy; the node backend is the fallback (C01 / `compilerBackend`).
- `framework.compilerBackend` — `enum: `"auto"` \| `"native"` \| `"node"`` — default —: V2-C01 (D26): the compiler backend the precompile pass consults per file. `auto` (the default) picks a native compiler when one is compiled in and healthy and uses the Node tooling pool otherwise; `native` requires the native path and refuses with CMP-NAT-001 when no healthy backend is available; `node` always uses the pool. Overridable via `OXIDE_COMPILER_BACKEND`.
- `framework.islands` — `object` — default —: §10.3 islands registry: component name → source file (V2-F01: string, or { src, framework? } for a mixed-framework island). The CLI emits a synthetic __boot__ client entry that registers each island with its own framework's adapter; `<Island chunk="X">` on a page resolves to X in this map.
- `framework.name` — `string` — default —: §7: the framework adapter package the config exports (`@oxide-fw/react`, `@oxide-fw/vue`, ...).
### `i18n`

- `i18n.default` — `string` — default —: §11: the default locale — no path prefix under `prefix-except-default`.
- `i18n.locales` — `string[]` — default —: §11: the locales the site serves (the route prefix follows `strategy`).
- `i18n.strategy` — `enum: `"prefix"` \| `"prefix-except-default"` \| `"none"`` — default —: Locale routing strategy: `prefix` (always), `prefix-except-default` (omit the default locale), `none` (no path prefix; cookie/Accept-Language only).
### `images`

- `images.avifSpeed` — `integer` — default `6`: V2-C09: the AVIF encoder speed (ravif's `with_speed`, 1..=10 — higher is faster at lower quality; the default is 6, the documented middle rate). Validated at load (`CFG-AVIF`).
- `images.formats` — `string[]` — default —: §16.1: the encodings `/_oxide/image` emits, in preference order (avif via ravif).
- `images.maxConcurrency` — `integer` — default `4`: §16.1: maximum concurrent rasterizations. Each transform is CPU-bound; unbounded concurrency lets a few clients saturate every core.
- `images.maxDimension` — `integer` — default `8192`: §16.1: largest accepted `w`/`h`. A user-supplied size is not trusted to allocate an arbitrarily large canvas.
- `images.remotePatterns` — `array` — default —: §16.1: the remote image hosts the endpoint may fetch; a host not listed is refused.
- `images.remotePatterns[].host` — `string` — default —: §16.1: a remote host the image endpoint may fetch from (the SSRF allow-list for remote images).
### `imagesEndpoint`

- `imagesEndpoint` — `object` — default —: S-09: on-demand image transform endpoint (GET /_oxide/image). On by default; gateable.
### `interactionReplay`

- `interactionReplay` — `boolean` — default `true`: V2-F03: the pre-hydration interaction replay opt-out. Default true (flag absent = enabled): the boot capture records interactions until each island hydrates and replays them on done. false advertises interactionReplay:false in window.__oxide so boot skips startReplay. A non-boolean value fails the load (DSP-114).
### `live`

- `live` — `object` — default —: V2-F05: live-loader transport knobs (the SSE hub's capacity, cadence and transport). Defaults are inert - with no route opting in (page.server.ts live: true) nothing listens and no hub is constructed. Validation (DSP-111): keepaliveSeconds >= 1, maxSubscribers >= 1, maxRerunsPerSecond >= 1, perIpMax <= maxSubscribers.
- `live.debounceMs` — `integer` — default `50`: Per-subscriber rerun debounce in ms.
- `live.keepaliveSeconds` — `integer` — default `20`: SSE keepalive ping period in seconds (DSP-111 rejects 0).
- `live.maxRerunsPerSecond` — `integer` — default `20`: Per-subscriber rerun rate cap per second.
- `live.maxSubscribers` — `integer` — default `5000`: The hub's total subscriber capacity; the endpoint answers 503 + retry-after above it.
- `live.perIpMax` — `integer` — default `20`: Per-peer subscriber cap (security); cannot exceed maxSubscribers.
- `live.transport` — `any` — default `"sse"`: sse streams snapshots over GET /__oxide/live; poll is the serverless fallback.
### `llms`

- `llms.enabled` — `boolean` — default `false`: V2-E07: emit dist/llms.txt (title, summary, the included routes with their meta descriptions) and dist/llms-full.txt (the routes' text from the content collections, Markdown) at build. Default false: neither file is written. A bad include pattern fails the load (CFG-LLMS).
- `llms.include` — `string[]` — default —: Route patterns the files cover — glob-lite: ** matches any run of segments, * within one segment ('/docs/**' = every /docs page). Empty means every prerendered page.
- `llms.summary` — `string` — default —: The blockquote summary under the title.
- `llms.title` — `string` — default —: The llms.txt H1 title. Defaults to the site name (the siteUrl host or the project directory name) when omitted.
### `manifest`

- `manifest` — `object` — default —: §16.3: web app manifest (PWA). Present -> oxide build emits manifest.webmanifest + a service worker.
### `modules`

- `modules` — `string[]` — default —: §15.4: distributable modules to install.
### `modulesConfig`

- `modulesConfig` — `object` — default —: §V2-E06: per-module config defaults (merged from each module's `oxide.module.configDefaults` under `modules.<name>`). The key is the sanitized module name (`@scope/pkg` → `scope-pkg`).
### `offline`

- `offline` — `object` — default —: V2-F14: the offline-first knobs. Wired: the CLI emission gate writes dist/sw.js from the produced client manifest and the boot module answers the worker's activate fan-out with a queue flush; the server-side stage the outbox's retries hit is dispatch::idem::stage_idempotency (the x-oxide-idem replay key). A bad precache spelling or a wrong-typed knob fails the load (CFG-OFFLINE).
- `offline.enabled` — `boolean` — default `false`: Master switch. false (the default) installs no service worker and boots no outbox.
- `offline.outbox` — `boolean` — default `true`: The offline action outbox (packages/core's enqueue/flush, the x-oxide-idem replay header). On inside an enabled section (the spec's example shape); false keeps the precache-only posture.
- `offline.precache` — `enum: `"routes"` \| `"all"`` — default `"routes"`: The precache breadth: 'routes' caches the shell + the prerendered routes (the navigation-fallback surface), 'all' caches every hashed asset the client manifest carries.
### `og`

- `og` — `object` — default —: S-09: on-demand OG-image endpoint (GET /_oxide/og). Off by default; opt in with og.enabled = true.
- `og.enabled` — `boolean` — default `false`: S-09: opt in to the OG-image endpoint (`GET /_oxide/og`; off by default).
- `og.rateLimit` — `object` — default —: A3: per-peer token-bucket limit applied before rasterization.
- `og.rateLimit.max` — `integer` — default `10`: A3: the images per window per peer before the bucket refills.
- `og.rateLimit.windowSecs` — `integer` — default `60`: A3: the rate-limit window length in seconds.
### `permissions`

- `permissions.default` — `enum: `"allow"` \| `"deny"`` — default —: V2-F19: the default posture when a route declares no `permissions` block. `allow` (the v0.2 default) permits every capability; `deny` is the opt-in so an undeclared route gets no capability by default.
- `permissions.env` — `any` — default —: V2-F19: the env-var-name allow-list for `process.env` reads. `null` = unrestricted; `[]` = no name. Names match exactly (case-sensitive).
- `permissions.fs` — `any` — default —: V2-F19: the fs-path-prefix allow-list for file reads. `null` = unrestricted; `[]` = no path. A `./data` entry allows `./data/anything` (prefix match, `./`-normalized).
- `permissions.net` — `any` — default —: V2-F19: the host-pattern allow-list for `fetch`. `null` = unrestricted; `[]` = no host. A `*.internal` pattern matches `api.internal` (label-boundary, not bare suffix).
### `plugins`

- `plugins` — `object[]` — default —: §15: plugin entries (native, wasm, or js).
### `prefetch`

- `prefetch.learned` — `boolean` — default `false`: V2-F13: the learned prefetcher. Default false: no nav graph is constructed, the data plane observes nothing and GET /_oxide/nav answers 404. true keeps the aggregate route-transition graph (route ids only, k-anonymity floor 20 observations, half-life 7 days, hourly snapshot to .oxide/nav-graph.json) and answers GET /_oxide/nav?from=<route> with the top edges as probabilities. A non-boolean value fails the load (DSP-115).
- `prefetch.minProbability` — `number` — default `0.35`: V2-F13: the client prefetcher's threshold — an edge whose /_oxide/nav probability is at or above this value gets its chunks (and, for param-free route ids, its data) prefetched after idle. Out-of-range values fail the load (DSP-115).
### `record`

- `record` — `any` — default —: V2-F15: the request recorder (the record-and-replay substrate). false (the default) records nothing; 'all' captures every request, 'errors' captures failed (5xx) requests, and { sample, onError } is the prod sampling shape. The key is inert at the wiring until the CLI's record/replay adoption lands. A bad spelling, an out-of-range sample or a wrong-typed knob fails the load (CFG-REC).
### `render`

- `render.default` — `enum: `"auto"` \| `"static"` \| `"isr"` \| `"dynamic"` \| `"spa"` \| `"islands"`` — default —: §10.1: the render mode routes inherit when `page.server.ts` exports none (`auto` infers per route).
- `render.ppr` — `boolean` — default `false`: §10.8 partial prerendering: serve the cached static shell instantly and stream holes live
### `routes`

- `routes.dir` — `string` — default `"src/routes"`: §10.1: the route directory the scanner walks (file conventions decide the route tree).
### `rules[]`

- `rules[].destination` — `string` — default —: §10.4: the rewrite destination (evaluated natively before the route match).
- `rules[].priority` — `enum: `"low"` \| `"normal"` \| `"critical"`` — default `"normal"`: V2-F12: the route's overload priority; low-priority routes shed at brownout level Shed, everything but critical at Emergency (DSP-113 rejects other spellings).
- `rules[].redirect` — `string` — default —: §10.4: the redirect destination (mutually exclusive with `destination`).
- `rules[].source` — `string` — default —: §10.4: the path pattern the native rule matches.
- `rules[].status` — `integer` — default —: §10.4: the redirect status (301/302/307/308).
### `search`

- `search` — `object` — default —: V2-F09: the built-in search surface. Inert by default - with no collections configured nothing indexes and the §18 config-key refusal reports the surface as unconfigured. A non-empty collections list is the 'search configured' fact the refusal consults (AST-SRC-001 on a target without the Search capability); the F09 engine consumes the query knobs from this same key.
- `search.collections` — `string[]` — default —: The content collections (§16.5 names) the built-in search indexes. Empty = search not configured; a non-empty list is the §18 refusal's 'search configured' surface.
- `search.fields` — `object` — default —: Query-time field boosts (F09 defaults: title 3, body 1).
- `search.fields.body` — `number` — default —: The body-field boost.
- `search.fields.title` — `number` — default —: The title-field boost; title matches outrank body matches.
- `search.languages` — `string[]` — default —: Index languages (F09 default: ['en']).
- `search.snippet` — `integer` — default —: Snippet length in characters (F09 default: 160).
### `security`

- `security.cookieKeys` — `string[]` — default —: SEC-05 / §16.6: cookie-signing keys (32+ bytes each, first signs). Empty = derive from `.oxide/secret`.
- `security.csp` — `enum: `"strict"` \| `"hash"` \| `"off"` \| `"report-only"`` — default —: §S-20 / V2-E04: the Content-Security-Policy mode — `strict` (nonces), `hash`, `report-only` (the policy rides `Content-Security-Policy-Report-Only` with a `report-to` group the `/_oxide/csp-report` endpoint backs), `off`.
- `security.csrf` — `enum: `"origin"` \| `"token"` \| `"off"`` — default —: §S-20: the CSRF protection mode — `origin`, `token`, `off`.
- `security.exposeCacheHeader` — `boolean` — default —: S-21.3: emit the `x-oxide-fw-cache` status header. Default true.
- `security.hostAllowList` — `string[]` — default —: §S-20: the hosts the server answers for; a request with another Host is a 421 misdirected request.
- `security.sessionKeys` — `string[]` — default —: ≥32-byte base64 keys for the ChaCha20-Poly1305 session codec; first seals, all decrypt (rotation).
- `security.sessionTtlSecs` — `integer` — default `1800`: §16.6: session cookie lifetime in seconds.
- `security.sri` — `boolean` — default `true`: V2-E04: emit sha384 subresource-integrity digests for every emitted client JS/CSS chunk in `client.manifest.json` (`integrity`) and stamp the boot `<script type=module>` tag with `integrity` + `crossorigin`. Default true (production builds); dev output is unaffected (the `/@boot/` virtual module has no digest).
- `security.ssrfAllowList` — `string[]` — default —: §S-20: the hosts server-side fetches (image endpoint, loaders) may reach.
- `security.trustedProxies` — `string[]` — default —: SEC-05 / S-10: the CIDRs whose `X-Forwarded-For` is trusted for the client IP (rate-limit keying); empty trusts no proxy.
- `security.trustedTypes` — `enum: `"off"` \| `"report"` \| `"enforce"`` — default `"off"`: V2-E04: the Trusted Types mode. `report`/`enforce` add `require-trusted-types-for 'script'; trusted-types oxide default` to the emitted CSP — `report` rides an additional Report-Only policy, `enforce` the enforced header. The boot-side policy creation is the client half (docs/deviations.md).
### `server`

- `server.backend` — `enum: `"node"` \| `"v8"`` — default `"node"`: §12: the SSR backend — `node` (the supervised Node ≥ 24 worker pool, the production default) or `v8` (embedded V8, opt-in).
- `server.compression` — `string[]` — default —: §12.1 step 8: the precompressed static encodings in preference order (zstd preferred, then br, then gzip; negotiated per request).
- `server.h3Advertise` — `boolean` — default `true`: §12.1 / C3: emit the `Alt-Svc: h3` header on the TLS listener. Default true when http3 is enabled; set false to keep the QUIC listener silent while browsers are not ready for it.
- `server.host` — `string` — default `"localhost"`: §12: the bind address of the production server.
- `server.http3` — `boolean` — default `false`: §12.1: serve HTTP/3 over QUIC alongside h1/h2 (opt-in; `h3Advertise` controls the Alt-Svc header).
- `server.port` — `integer` — default `3000`: §12: the TCP port the production server binds (h1/h2; the QUIC listener follows §12.1).
### `siteUrl`

- `siteUrl` — `string` — default —: §16.3: absolute site origin used by sitemap.xml and robots.txt.
### `static`

- `static.fallback` — `any` — default —: Path to a fallback HTML file emitted for dynamic routes without prerendered output (§10.5).
### `stream`

- `stream` — `object` — default —: V2-F07: resumable-stream bounds. A streamed response marked x-oxide-stream stays replayable for the ttl at GET /__oxide/stream/<id> with Range: bytes=<n>-. Defaults are inert - the store only exists while a resumable response is being teed. Size literals use the section-14 grammar (b\|kb\|mb\|gb\|tb, plain digits are bytes). Validation (DSP-112): ttlSecs >= 1, maxBytesEach >= 1 byte, maxBytesTotal >= maxBytesEach; a bad literal fails with CFG-SIZE.
- `stream.resume` — `object` — default —: The reattach/replay bounds.
- `stream.resume.maxBytesEach` — `string` — default `"4mb"`: Per-stream replay cap; past it the buffer loses replay and the client falls back to a fresh fetch.
- `stream.resume.maxBytesTotal` — `string` — default `"256mb"`: Store-wide cap; the accounting evicts the oldest completed buffer first, then the oldest buffer.
- `stream.resume.ttlSecs` — `integer` — default `60`: How long a buffer stays reattachable, in seconds (DSP-112 rejects 0).
### `svg`

- `svg.optimize` — `boolean` — default `false`: V2-C07: run the SVG optimizer over SVG documents that enter the build (imports and SVG-as-component sources). The backend is the OXVG optimiser's correctness preset behind the `svg-oxvg` feature; with the knob off (and in every feature-off build) documents ship byte-identical. Every optimizer output is held to the preservation rules — the source's `viewBox`, every `<use>`/`url(#…)`/`aria-*` referenced id, well-formed output — and a violation is refused with `AST-SVG-001`, the source kept.
### `sw`

- `sw` — `object` — default —: §16.6: service worker configuration. Present only when the project wants to control emission beyond the default (manifest presence).
- `sw.enabled` — `boolean` — default —: §16.6: emit the service worker at build. Default true when `manifest` is present; an explicit `false` disables emission even with a manifest.
- `sw.offlineFallback` — `string` — default —: §16.6: the offline-fallback route the worker serves for a navigation miss. Default `/offline` when the build produced that route; otherwise `/`.
