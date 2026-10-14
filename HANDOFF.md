# Oxide session handoff — framework adapter read path complete; bundler transform gap open

**Date written:** 2026-10-13
**Repo:** `oxide` (meta-framework, spec v0.1)
**Working tree:** clean at handoff time

The framework-adapter **read path** (dev / serve / render / build)
is functionally complete and covered end-to-end. The **serverless
deploy artifacts** are textually correct but not yet runtime-runnable:
the bundler emits raw TypeScript + JSX into the SSR chunks instead of
compiling them. That is the next slice and it is scoped below.

---

## 1. What this session accomplished

### The big milestone — framework-adapter read path closed

Every HTML-producing command now dispatches through the project's
`oxide.config.{ts,mjs,js}` framework adapter:

| Command | Behavior | Status |
| --- | --- | --- |
| `oxide config` | Loads `.ts` config through the worker, emits descriptor JSON | ✅ |
| `oxide render <route>` | Runs loader, dispatches through adapter, prints HTML | ✅ |
| `oxide render <route> --stream` | Same, but consumes the stream chunk-by-chunk | ✅ |
| `oxide build` | Bundles, prerenders static routes (incl. runtime `generateParams`), emits per-target artifacts | ✅ |
| `oxide dev` | HTTP dev server, chunked streaming, HMR invalidation on file change | ✅ |
| `oxide start` | Production server, chunked streaming | ✅ |

All four framework adapters (`@oxide/react`, `@oxide/vue`, `@oxide/svelte`,
`@oxide/solid`) are wired with current upstreams (React 19.3, Vue 3.6.0-rc.11,
Svelte 5.57, Solid 2.0.0-rc.14 + `@solidjs/web`).

### Streaming (trait → wire → transport → HTTP)

- `FrameworkRenderer::render_streaming` (jshost) — default collects;
  `FrameworkHostRenderer` overrides to forward each worker `CallStream`
  chunk (no buffering).
- `PageRenderer::render_streaming` (server) — same default.
- `AdapterPageRenderer` (cli shim) — forwards through.
- Worker emits each adapter chunk as its own `call_chunk` frame; raw HTML
  bytes on the wire (no JSON envelope).
- `OxideResponse::stream(status, BodyStream)` — new response body
  variant. `to_hyper_response` handles both branches: streaming uses
  chunked transfer, collected sets `content-length`. Body error type
  moved from `Infallible` to `io::Error` so mid-response failures abort
  the HTTP connection instead of silently truncating.
- `build_dispatch` calls `renderer.render_streaming()` when a renderer is
  attached; bridges the `Send`-only `RenderStream` to the `Send + Sync`
  `BodyStream` via a small mpsc pump helper (`render_stream_to_body_stream`).
- HTTP E2Es assert the transfer encoding explicitly:
  - `GET /` → no `Content-Length`, `Transfer-Encoding: chunked`
  - `GET /_oxide/health` → `Content-Length`, no `Transfer-Encoding`

### Runtime `generateParams` (spec §10.5)

- Worker op `generate-params:run` imports the module, calls the export
  (function or value), returns a flat string map per entry.
- `LoaderHost::generate_params` returns the enumerated entries.
- `cmd_build`'s `Runtime` arm evaluates through the worker instead of
  printing "requires runtime evaluation (post-MLP)".

### TSX compilation (spec §6.3)

- Worker `register-hooks.mjs` calls `module.register()` on
  `tsx-loader.mjs` via `node --import` — warning-free replacement for
  the deprecated `--experimental-loader`.
- `tsx-loader.mjs` transforms `.tsx` / `.jsx` imports to plain JS via
  sucrase at import time. **This is why `oxide dev` / `render` / `serve`
  work**: the worker compiles at module-load time.

### Deploy target fixes (spec §18)

All 10 targets in `oxide_adapters::Target` now emit their characteristic
files (verified by `crates/adapters/tests/deploy_matrix.rs`):

- Binary / Docker / Node / Cloudflare / Deno / Bun — were already correct.
- Static — copies prerendered outputs, emits `_headers` / `_redirects`.
- **Vercel** — handler at `api/handler.mjs`; `vercel.json` uses modern
  `rewrites` routing every path to `/api/handler`.
- **Netlify** — handler at `netlify/functions/handler.mjs`; `netlify.toml`
  declares `[build].functions`.
- **Lambda** — handler at `handler.mjs`; `template.yaml` (SAM) declares
  `Runtime: nodejs20.x`, `Handler: handler.handlerWrapper`, `HttpApi`
  event.
- `copy_server_bundle` copies the whole `dist/server/` tree (bundle +
  chunks + manifest) into `<deploy>/server/`, so the handler's relative
  import resolves at deploy time.
- `serverless_handler` is parametrized with the import path — Lambda
  `./server/ssr/index.js`, Vercel `../server/ssr/index.js`, Netlify
  `../../server/ssr/index.js` — matching the handler file's depth.

### CLI + fixture work

- `defineConfig` helper + `OxideUserConfig` type in `packages/oxide`.
- `examples/smoke-app` is a workspace member with a real `.ts` config,
  real `.tsx` pages, a real loader, a runtime `generateParams`, and both
  static and dynamic routes.
- `cmd_render` runs the route's `page.server.ts` loader.
- `cmd_build`'s config load uses `load_config_or_default` so a config
  the Rust extractor can't fully parse still renders.
- Minimal config scan (`extract_minimal_framework`) picks up `framework`,
  `adapter`, and `routes.dir` from dynamic configs (create-oxide shape
  with `componentFor` callbacks).
- All `cmd_dev` / `cmd_serve` / `cmd_build` canonicalize `root` at entry
  (the path-doubling bug bit three times — see `CONVENTIONS.md`).

---

## 2. Test state (all green at handoff)

| Suite | Command | Status |
| --- | --- | --- |
| Workspace Rust | `cargo test --workspace` | ✅ 746 tests |
| TS packages | `pnpm run test:packages` | ✅ 7 packages |
| Conformance §22 | `cd conformance && node --test test/*.test.ts` | ✅ |
| jshost framework E2E | `cargo test -p oxide-jshost --test framework_host_e2e -- --ignored` | ✅ 3 |
| jshost loader E2E | `cargo test -p oxide-jshost --test loader_host_e2e -- --ignored` | ✅ 5 |
| jshost hmr E2E | `cargo test -p oxide-jshost --test hmr_e2e -- --ignored` | ✅ 2 |
| adapters deploy matrix | `cargo test -p oxide-adapters` | ✅ 25 + 5 |
| smoke-app CLI E2Es | `pnpm test:smoke` | ✅ 5 |
| Full framework-adapter E2E | `pnpm test:framework-adapter` | ✅ |

The `#[ignore]`d E2Es spawn a Node worker and require `pnpm install`
(workspace `node_modules`). They're excluded from the default test run
by design.

**Known intermittent flake (fixed):** `ws_decode_close_payload_rules`
in `crates/server/tests/server_tdd.rs` was non-deterministic — it
mutated a *masked* payload byte and asserted invalid UTF-8, which only
held when the random mask key's high bit was clear. Fixed by
hand-building the frame with mask key `[0,0,0,0]` so the payload byte
is unambiguously `0xFF`.

---

## 3. Bundler transform gap — CLOSED on 2026-10-13

> **Resolved.** `bundle_impl` now runs every non-asset module through
> `oxide_compiler::OxcTransformer` (Oxc parse + TypeScript + JSX +
> codegen) before it lands in a chunk file. Regression coverage lives
> in `crates/bundler/tests/bundler_jsx_lowering_tdd.rs`, which shells
> out to `node --check` on every emitted `.js` chunk for TSX pages
> (SSR + client) and a TS loader (SSR). See `WORKLOG.md` session 7
> for the full slice. Remaining follow-ups (Vue / Svelte / Solid
> compilation, per-build server-fn secret) are tracked in §7.



### The bug (confirmed by diagnostic)

`crates/bundler/src/lib.rs`'s `bundle_impl` reads source, hashes it,
scans imports, resolves dependencies, then writes the **raw source**
into each chunk. It never calls `oxide_compiler::OxcTransformer`. So
the emitted server chunks are TypeScript + JSX, not valid JavaScript.

Verified with `node --check`:

```
$ node --check dist/server/routes-index-page-*.js
SyntaxError: Unexpected token '<'    # JSX not lowered

$ node --check dist/server/routes-blog-\[slug\]-page-*.js
SyntaxError: Unexpected strict mode reserved word    # `interface Props {`
```

Every serverless target (Lambda / Vercel / Netlify) emits a deploy
artifact that fails on first module load. `oxide dev` / `render` /
`serve` are unaffected — those compile via the worker's sucrase hook
(`tsx-loader.mjs`) at import time, which operates against source on
disk, not the emitted chunks.

### What's broken vs what works

| Build path | Compiled to JS? | Deployable? |
| --- | --- | --- |
| `oxide render` / `dev` / `serve` (worker) | yes (sucrase at import) | n/a — HTTP |
| `oxide build` → **serverless** target | **NO** | **no** |
| `oxide build` → static target | n/a (HTML) | yes |
| `oxide build` → binary / docker | n/a (Rust binary) | yes |

### Scope of the fix

1. **Recon** the exact site in `bundle_impl` where source becomes a
   chunk (see below) plus the Oxc `TransformInput` / `TransformOutput`
   shapes.
2. **Patch `bundle_impl`** to call `oxide_compiler::OxcTransformer::transform`
   on every non-asset module before emitting. Use `output.code` in place
   of the raw source. `oxide-compiler` is already a bundler dep.
3. **Handle diagnostics** from the transform — push parse errors into
   the existing `diagnostics` vector.
4. **Update tests** in `crates/bundler/tests/` to assert `node --check`
   passes on every emitted server chunk (write a test helper that shells
   out to `node --check`).
5. **Follow-up (documented, not blocking):** Vue / Svelte / Solid server
   compilation via the tooling pool. React via Oxc covers the smoke-app
   today; the other three need `@vue/compiler-sfc`, `svelte/compiler`,
   and `babel-preset-solid` in the worker, which is a separate slice.

### Recon commands to run first

```sh
# Where does the source become a chunk?
grep -n "chunks.push\|Chunk {\|file_of\|let mut chunks" crates/bundler/src/lib.rs | head -30

# The bundle_impl body:
python3 - << 'PYEOF'
import pathlib, re
src = pathlib.Path('crates/bundler/src/lib.rs').read_text()
m = re.search(r'fn bundle_impl\([\s\S]*?\n    \}\n', src)
if m:
    print(m.group(0)[:5000])
PYEOF

# Oxc transformer signature:
grep -n -A 20 "impl Transformer for OxcTransformer" crates/compiler/src/oxc_backend.rs | head -30
grep -n -A 12 "pub struct TransformInput" crates/compiler/src/lib.rs
grep -n -A 10 "pub struct TransformOutput" crates/compiler/src/lib.rs

# Env enum (Client / Ssr / Edge):
grep -n -A 8 "pub enum Env" crates/compiler/src/lib.rs

# BundleEnv mapping:
grep -n -A 8 "pub enum BundleEnv" crates/bundler/src/lib.rs

# Where the raw source gets written into the chunk:
grep -n "source.clone\|source.as_bytes\|chunk.*source\|module.content" crates/bundler/src/lib.rs | head -20
```

---

## 4. Conventions that worked this session

1. **Narrow anchors, not large block literals.** A one-line anchor like
   `match renderer.render(input).await {` matches byte-exactly; a
   multi-line block with em-dashes and `§` characters does not survive
   the shell/Python round-trip. When in doubt, print the file's repr
   before writing the patch.

2. **`set -euo pipefail` at the top of every patch script.** Without it,
   a failing step prints an error and continues, landing a commit whose
   message describes work that didn't happen. That happened twice.

3. **Raw strings for line-oriented output.** Rust's `\`-at-end-of-line
   strips both the newline *and* the leading whitespace of the next
   line. This bug hit four times: Dockerfile `USER` glued to `COPY`,
   Netlify TOML keys flattened, SAM YAML resource outdented,
   serverless handler quotes over-escaped. Use `r#"..."#` for any
   emitted config or script.

4. **Brace-matching for struct/arm extraction.** When replacing a
   whole `match` arm or a struct literal, count braces from the opening
   line to find the exact close. Never use `.*?` regexes with nested
   braces — they miss on the first `}`.

5. **Recon before patch.** Every slice that skipped this needed a
   second script to recover. The shapes that hurt: `RouteEntry` field
   names, `Pipeline` struct literals, `CallStream` variant fields,
   `OxcTransformer` signature. Print the struct / fn body first.

6. **Commit after each green slice, one coherent unit per commit.**
   Convention: `feat(scope):` / `fix(scope):` / `test(scope):` /
   `docs(scope):`. The git log is the project's narrative; keep it
   honest, including "this commit actually does what the previous
   message claimed" when you need to correct the record.

7. **Idempotency checks scoped to the function, not the file.** A
   whole-file `if 'let page_renderer' in src` check returns True when
   the pattern exists in *another* function. Extract the function body
   by name and check within it.

---

## 5. Environment notes

- **Machine:** macOS arm64. Disk went from 25 GB free to 166 MB free
  and back across the session; cleanup scripts (`find target -name
  '*.dSYM' -delete`, drop test binaries, `cargo clean -p <crate>`)
  are in the git history if needed again.
- **Node:** v22.23.2 in PATH (from `~/.local/bin/node`). Node 24 is
  installed via Homebrew as a keg-only formula at
  `/opt/homebrew/opt/node@24/bin/node`. The workspace declares
  `engines.node: ">=24"` and pnpm warns; adding the keg to PATH
  suppresses the warning. Type stripping (`.ts` → JS) works on 22.18+
  without a flag; the `tsx-loader.mjs` hook handles `.tsx`.
- **`--experimental-loader` is deprecated** in Node 22. Use
  `--import <register-shim.mjs>` with `module.register()`. The shim is
  `crates/jshost/worker/register-hooks.mjs`.
- **`cargo test --workspace`** takes ~2-4 minutes on this hardware with
  a warm target; longer on cold.

---

## 6. Commit history (last 20)

```
ff4779d feat(adapters): copy server bundle to deploy + target-relative imports
e36e295 fix(adapters): real § in raw-string configs + deploy_matrix tests
28062e0 fix(adapters): preserve indentation in Netlify toml + SAM yaml
4701036 fix(adapters): Dockerfile missing newline before USER
b7987a1 fix(config): minimal scan picks up adapter + routes.dir
42ca4a0 test(server): deterministic hand-built frame in ws UTF-8 test
a80061c docs: reference README for smoke-app + test:smoke shortcut
091e814 docs(worklog): session 6 — streaming + runtime generateParams
6ba89b7 feat(cli): cmd_build evaluates runtime generateParams via worker
7a558b4 feat(jshost): generate-params:run worker op + LoaderHost::generate_params
61d6c51 test(cli): 'oxide start' E2E — production server + chunked streaming
e489b82 test(cli): assert chunked transfer for streaming responses
e4c0e89 feat(cli): build_dispatch streams renderer output to hyper
42bd5ad feat(cli): build_dispatch streams renderer output to hyper
ea6fb1b feat(server): OxideResponse streaming body + to_hyper_response
7255bd3 feat(jshost): worker preserves adapter chunk boundaries on the wire
01b8fe9 fix(jshost): raw-HTML wire + 'oxide render --stream' exercises it
f020394 feat(cli): 'oxide render' runs the loader + smoke-app uses loader data
0a13a9f feat: streaming render trait across the framework adapter stack
bf7d939 docs(worklog): session 5 — framework adapter read path complete
```

(`42bd5ad` and `e4c0e89` have the same subject; the second corrects the
first, whose patch failed to apply but whose message described the
intended work. The commit body of `e4c0e89` names the correction.)

---

## 7. Status at end of session 10 (2026-10-14)

> See `WORKLOG.md` session 10 for the full slice list with per-feature proofs.

### Closed this session

| Item | Commit | Verified by |
| --- | --- | --- |
| Islands in a real browser (React `hydrateRoot`) | `59dc9e8` | Playwright 7/7, incl. a click |
| Client bundle 960 KB → 6.8 KB | `b59f733` | live build |
| Streaming injector position + crate move | `27ab15e` | `oxide-server` tests |
| Four framework fixtures | `b92ac0d` | four builds, `tests/fixtures/check.sh` |
| Rust-native Solid JSX (`solid-jsx-oxc`) | `b92ac0d` | smoke-app-solid renders its page |
| §10.1 mode inference | `b92ac0d` | `oxide routes` + prerender |
| CLI-level conformance gate | `a36d22f` | 4 fixtures, 13 s |
| §16.1 `/_oxide/image` endpoint | `fdff7b3` | live curl |
| §16.1 remote-source gating | `b9ff957` | unit tests |
| §16.3 sitemap + robots | `8f3b504` | four fixtures |
| §9.4 server functions end-to-end | `65acb8f` + `aa8efad` + `43e28fa` | live POST → `Hello Grace…` |
| §9.3 `'use cache'` directive installed | `75d16f1` | emitted shape in a real build |

### The one rule this session learned

**If a test only needs the built binary, do not put it inside a target
that links V8.** `oxide-cli`'s test target statically links V8 (~140 MB)
and relinks on every source change — ~5 min per iteration. Two slices
were moved out of it: the four-fixture gate to
`tests/fixtures/check.sh` (5 min → 13 s), the streaming injector to
`oxide-server` (a crate with no V8 dependency).

Also: `[profile.test] opt-level` now matches `[profile.dev]` (`0`).
Different opt-levels mean different rlibs, so `cargo test` after
`cargo check` recompiled the whole dependency tree in silence.

### Genuinely open, ordered by leverage

1. **§9.3 tags + L1 cache on the request path.** The `'use cache'`
   wrapper is installed but (a) drops a non-literal `cacheTag` argument
   and (b) caches in-process only. The fix: the compiler emits a tag
   thunk, the runtime registers resolved tags, the worker
   `cache_get`/`cache_set` ops (in the `Op` table, unimplemented) bridge
   to `oxide_cache::CacheHandler`.
2. **§16.1 remote image fetch.** Validation is done; `serve_request`'s
   native prefix must become awaitable for the fetch itself.
3. **§15 plugin host.** `crates/plugin/src/wasm.rs` has a working
   `WasmPlugin`; wire it to the bundler's transform chain.
4. **§10.1 inference as a real pass.** Today a marker regex; the spec
   wants source spans + a diagnostic naming the read.
5. **§21 benchmark harness.** The four fixtures are the reference apps;
   `benchmarks/` is a README.
6. **§4.4 snapshots, §12 HTTP/3, §19 editor tooling, §22 full
   assertion set.** Larger, spec-named, not started.

## 8. Where to start

Read `WORKLOG.md` (the session log) and `CONVENTIONS.md` (the seven
rules above, plus the path-canonicalization rule). Then run
`pnpm test:framework-adapter` — that's the full read-path E2E suite in
one command. If it's green, the environment is healthy and the next
slice (§3) is ready to start.
