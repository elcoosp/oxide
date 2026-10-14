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

## 7. Follow-ups — most closed on 2026-10-14

> See WORKLOG.md session 8 for the full slice list.

| Item | Status |
| --- | --- |
| Vue / Svelte server compilation in the bundler | **done** (`74cd442`, `d89ab42`); Solid uses Oxc JSX with the Solid pragma |
| Per-build server-fn secret | **done** (`a133612`) |
| Conformance §22 CLI-level E2Es | **done** (`9e7d25b`) |
| Deno Deploy config + Cloudflare Pages target | **done** (`1bd8749`) |
| Vercel `functions` layout | **done** (`365e12c`) |
| V8Host migration for `render_pool` | **spec-gated**; post-MLP, needs `deno_node` for npm resolution |
| Playwright E2E for islands hydration | **open**; needs browser install |
| Real sourcemaps (rolldown backend) | **open** |

The bundler transform gap from §3 is closed (`a3ba56b`).



- **V8Host migration** for `render_pool`. Post-MLP per the spec,
  gated on `deno_node` integration for npm resolution. One-line swap
  in `build_framework_stack` once that lands. The `JsHost` / `Pool`
  abstraction already supports it.
- **Vue / Svelte / Solid server compilation** in the bundler (see §3.5).
  Their runtime adapters and TSX-equivalent transforms work through the
  tooling pool at dev time; the build path needs the same wiring.
- **Real Playwright E2E** for islands hydration. The current tests use
  a hand-rolled fake DOM in each adapter package; a browser-based
  variant would catch hydration mismatches the fake cannot.
- **Conformance suite: CLI-level E2Es.** The current §22 suite drives
  adapters in-process. Adding a variant that runs the CLI against the
  smoke-app and asserts on the output would tighten the top-level
  contract.
- **Deployment adapters** for the platforms not yet covered
  (Lambda full templates, Vercel `functions` layout, Deno Deploy
  config, Cloudflare Pages).

---

## 8. Where to start

Read `WORKLOG.md` (the session log) and `CONVENTIONS.md` (the seven
rules above, plus the path-canonicalization rule). Then run
`pnpm test:framework-adapter` — that's the full read-path E2E suite in
one command. If it's green, the environment is healthy and the next
slice (§3) is ready to start.
