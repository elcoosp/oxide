# Oxide session handoff — §9.3 actions, §11 L0 cache, §5.1 warm build, §16.2 fonts, §16.6 sessions

**Date written:** 2026-10-15
**Repo:** `oxide` (meta-framework, spec v0.1)
**Working tree:** clean. HEAD is `6e7f93e`.
**Session:** 42 commits since `11fbdfd`; +3651 / −426 across 29 files.

Everything below was verified against a real build, a real HTTP response, or a
real worker call — not just `cargo check`. That discipline is what found every
bug this session (see "The recurring lesson").

---

## 1. What this session landed (all committed, all green)

| Area | Spec | What shipped |
| --- | --- | --- |
| Warm build | §5.1 | `build_cache.rs` — a blake3 project fingerprint; an unchanged `oxide build` **skips entirely** (~3.3 s → ~90 ms; target ≤ 500 ms). `OXIDE_FORCE_BUILD=1` bypasses. |
| Precompile cache | §5.4 / §6.3 | `precompile_cache.rs` — a Tier-B CAS cache keyed by `blake3(path‖source)‖framework‖mode‖compiler-version`. An unchanged SFC is not recompiled. |
| L0 response cache | §11.1 | `l0_cache.rs` — a response cache for `static`/`isr` routes, with **CSP-nonce substitution** (bodies stored with a sentinel, the live nonce swapped in on serve) and `x-oxide-cache: L0-hit\|L0-miss\|bypass`. |
| Cookies + sessions | §9.2 / §16.6 | `session.rs` — ChaCha20-Poly1305 encrypted, rotatable cookies (`ring`). Worker builds a real `CookieStore`; `LoaderOutput` carries `set_cookies`/`headers`; the CLI applies them to the response. |
| Form actions | §9.3 | End-to-end: worker `action:run` op, `ActionHost`, `ActionDispatcher` seam, CLI detection of `POST ?/<name>`. `error()`/`redirect()` sentinels, `cookies`, `setHeaders`, `revalidate`, Standard-Schema input validation. |
| Fonts | §16.2 | `FontsConfig.families` + `emit_fonts` → `dist/fonts/` + `dist/fonts.css` with metric-override fallback faces. Fixed a latent bug: the `@font-face` `format()` hint was hard-coded `woff2`. |

Plus earlier-session carry-overs committed here: §10.1 destructured-request-param
inference, §14.2 non-callback-dynamic config → V8, §17.1 ODF stack-overflow fixes.

### Verified end-to-end (the proof, not the log)

    # §11 L0
    GET /  → x-oxide-cache: L0-miss ; GET / → L0-hit ; GET / → L0-hit
    # §9.3 actions
    POST /?/greet  (name=Ada) → 200 {"message":"Hello Ada"}
                                set-cookie: greeted=Ada; Path=/; HttpOnly
                                x-oxide-greeted: Ada
    POST /?/boom              → 400 {"message":"bad input"}
    POST /?/missing           → 404 ACT-NOT-FOUND
    # §16.2 fonts
    fonts: 1 file(s), dist/fonts.css   (size-adjust: 285.80% from real skrifa metrics)
    # §5.1
    warm build: 90 ms ("build up to date — skipping")

---

## 2. The recurring lesson (rule 8, again and again)

Every real bug this session was caught by **running the thing**, never by
`cargo check`:

- §6.6 (prior session's slice) was **dead code**: the hook was wired into the
  in-house `ChunkBundler`, but `oxide build` calls `RolldownBundler`, which
  ignored it and refused `.css` outright. Only a real build exposed it.
- §9.3 actions returned `ENOENT` because `start_server_inner_full` never
  canonicalized its root — invisible until a POST.
- §11 L0 never cached because the **serve** path used raw `scan_routes` (no
  AST refinement), so a static route read `render_mode: None`. Only the
  `x-oxide-cache` header made it visible.
- §16.2 fonts labelled a `.ttf` as `format('woff2')` — a browser would skip
  the face. Only reading the emitted CSS caught it.

**Do not trust a green compile. Build it, fetch it, read the artifact.**

---

## 3. Test state at handoff

| Suite | Result |
| --- | --- |
| `cargo check --workspace` | green |
| oxide-server | 40 + 57 + 21 |
| oxide-jshost | 39 + 22 |
| oxide-cli --lib | 90 |
| oxide-assets | 98 + 11 + 9 |
| oxide-bundler | 47 across six binaries |
| Four-fixture CLI gate | PASS |

Known flaky: `node_host_recycles_worker_after_request_threshold` under parallel
worker spawns (passes alone). Test-harness race, not a product bug.

---

## 4. Conventions in force

1. **One commit per file.** Conventional style. Fixture files restored with
   `git checkout --` before `git add`.
2. **Verify the artifact, not the log** (section 2).
3. **Anchors for patches must be short and unique** — a one-line anchor
   matches; a multi-line block with em-dashes/backticks does not survive a
   shell/Python round-trip. Guards must be more specific than the field name.
4. **`CARGO_TERM_PROGRESS_WHEN=always`** for a non-TTY.
5. **Multiple `async move` blocks in one fn do not unify** — compute eagerly,
   end with one `async move`.
6. **Rolldown 1.2.13 refuses CSS** — any `.css` handling goes through a plugin
   `load` hook.
7. **The CAS (`redb`) is single-open.** Two `oxide` processes on one project
   (a lingering `start` + a `build`) deadlock the index and a build fails with
   `Database already open`. Kill stale servers before building.
8. **Heredocs with backticks** need a quoted delimiter (`<<'EOF'`).

---

## 5. Where to start next (largest gaps first)

1. **§4.4 app snapshots + wiring `V8Host` into the render path.** This is the
   §21 cold-start finding (782–2871 ms vs ≤ 50 ms target). `V8Host` is *real*
   (`deno_core`, worker pool, warm replacement), and `snapshot.rs`
   (`snapshot_build` / `loader_options`) is written — but the CLI uses
   `NodeHost` exclusively (`grep V8Host crates/cli` → 0), and `snapshot_build`
   is never called. This is the largest single slice and needs the render path
   to run on V8. Multi-session.
2. **§21 finding 2 — Vue's 951 KB SSR bundle.** Mark Node built-ins + the
   large framework runtimes external for the SSR env. **Decide the deploy
   policy first** (does `binary` ship `node_modules`?).
3. **§9.3 `revalidate` → cache.** Actions now return `x-oxide-revalidate`, and
   the CLI applies `path` invalidation to L0, but **tag** invalidation and L1
   (`'use cache'`) are not yet connected.
4. **§16.2 font subsetting + WOFF2.** The backend copies files verbatim;
   `write-fonts` is a dep but no subsetter is wired; providers fetch nothing.
5. **§16.6 web-vitals / `<Script>` / PWA service worker** — all unimplemented.
6. **The worker loader refuses `.css`** (session-14 loose end) — a page that
   imports a stylesheet degrades the framework renderer.
7. **§12 HTTP/3, §15 JS plugin bridge, §19 editor tooling** — untouched.

Sections not touched this session: §4.4 (above), §5 graph engine *node-by-node*
(the build-level early cutoff shipped, but per-chunk cutoff needs the graph
driving the bundler), §12 HTTP/3, §15.1 JS plugins, §19.3 editor tooling,
§22 full assertion set.

---

## 6. Commit history (last 20)

```
6e7f93e docs(worklog): §9.3 form actions on the server
af9fc1b feat(cli): run form actions through the render pool (§9.3)
d3bf1a5 feat(server): ActionDispatcher seam + run_action, honour its status (§9.3)
f18755c chore(jshost): export ActionHost + ActionOutcome (§9.3)
d139f7e feat(jshost): ActionHost runs a route action over the pool (§9.3)
c15b645 feat(jshost-worker): an action:run op for form actions (§9.3)
ff6ba2c docs(worklog): §11 L0 response cache on the request path
850e53f feat(cli): serve static routes from the L0 cache (§11.1)
a5aec6c feat(cli): §11 L0 response cache with nonce substitution
34cd0d7 docs(worklog): session 15 — cookies/sessions, precompile CAS, fonts, warm build
879eac9 feat(cli): bundle font families at build — dist/fonts + fonts.css (§16.2)
8cf9644 test(assets): font format hint + fallback-face metric override (§16.2)
300be26 fix(assets): @font-face format() hint matches the file container (§16.2)
88c2628 feat(config): FontsConfig carries families for §16.2 bundling
c2aadac perf(cli): content-addressed cache for the §6.3 precompile pass (§5.4)
44b13b4 test(cli): LoaderOutput literals carry the new side-effect fields (§9.2)
10c6f70 feat(cli): apply loader set-cookies + headers to the response (§16.6)
702fe92 feat(jshost-worker): build the full LoadEvent — cookies, setHeaders, parent (§9.2)
42b3819 feat(server): LoaderOutput carries set-cookies + headers (§9.2)
e15b83f feat(server): encrypted, rotatable session cookies (§16.6)
```

Working tree clean at handoff.
