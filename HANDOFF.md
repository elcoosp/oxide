# Oxide session handoff — §4.4 lint, §21 findings re-measured, SSR externals

**Date written:** 2026-10-15
**Repo:** `oxide` (meta-framework, spec v0.1)
**Working tree:** clean. HEAD is `c19c56a`.
**Session:** 12 commits since `ac52789`; +433 / −8 across 6 files.

Continues session 15 (handoff at `ac52789`). Verified every claim by building
and running, per the standing rule.

---

## 1. What landed

| Area | Spec | Result |
| --- | --- | --- |
| Snapshot side-effect lint | §4.4 | `emit_snapshot_lint` wired into `cmd_build`. Reports `setTimeout`/`setInterval`/`new WebSocket`/`new EventSource` at module scope with file:line. Advisory (never fails the build). |
| Build fingerprint fix | §5.1 | `*.oxide.ts` compiled temp files (written into the source tree) were counted as inputs, so a build's own temp output flipped the fingerprint and forced a spurious full rebuild — warm builds alternating between ~38 ms and ~3800 ms. Now skipped. |
| SSR externals | §21 f2 | `build.ssrExternals` (opt-in). Vue's SSR bundle: **951 049 B → 522 B**, still rendering through real Vue SSR. |

Plus test repairs from session 15's `build_dispatch` signature change
(`c16acc0`, `0899a4c`).

### §21 report re-measured — the report is stale, findings partly real

Re-ran the whole table on the current binary:

| fixture | warm build | clientJS | serverJS | routes |
| --- | ---: | ---: | ---: | ---: |
| smoke-app-react | 33 ms | 247165 | 39445 | 2 |
| smoke-app-vue | 37 ms | 6919 | 951049 | 1 |
| smoke-app-svelte | 35 ms | 30012 | 1435 | 1 |
| smoke-app-solid | 41 ms | 351 | 11982 | 1 |

- **Build time is fixed** — was 5.6–16.8 s, now 33–41 ms (report column stale).
- **Cold start was stale too**: report says 782–2871 ms; measured 426–597 ms
  (health is a *native* branch, so ~430 ms is the pre-bind framework-stack
  boot, not a worker).
- **JS-byte columns match exactly** — so finding 2 (Vue 951 KB) was real, and
  is now resolved by externals.

---

## 2. The proof (not the log)

    # §21 f2 — smoke-app-vue with ssrExternals: ['vue', '@vue/server-renderer', ...]
    server JS: 951049 B → 522 B
    GET / → 200, body has data-v-app + "Hello from Oxide (vue)"

    # §5.1 — six warm builds with a temp file present
    warm 1..6: 50/38/38/33/32/39 ms, all "build up to date — skipping"

    # §4.4 — fixture with `const t = setTimeout(() => {}, 1000)` at module top
    warn: snapshot side effect at src/routes/index/page.server.ts:3 — setTimeout ...
    snapshot: 1 top-level side effect(s) — move them behind a lazy initializer
    (exit 0); clean module → no output

---

## 3. Test state

| suite | result |
| --- | --- |
| `cargo check --workspace` | green |
| oxide-cli --lib | 91 |
| oxide-config | 18 + 26 |
| oxide-cache | 18 + 26 |
| oxide-jshost | 22 + 39 |
| oxide-server | 40 + 57 + 21 |
| four-fixture CLI gate | PASS |

---

## 4. Conventions in force (unchanged from session 15)

1. **One commit per file.** Fixtures restored with `git checkout --` before `git add`.
2. **Verify the artifact, not the log.** Every bug this session was found by running.
3. **Anchors must be short and unique**; a multi-line anchor with em-dashes/backticks
   does not survive a shell/Python round-trip.
4. **The `redb` CAS is single-open** — kill stale `oxide start` before `oxide build`.
5. **Heredocs with backticks** need `<<'EOF'`.
6. **`git commit` under a concurrent lock can double-apply** — a repeated
   `git add` + `commit` in one script wrote the same WORKLOG hunk twice. Prefer
   one commit per script, or check `git diff --cached` before committing.

---

## 5. Where to start next

1. **§4.4 proper — the V8 snapshot.** The lint is the piece §4.4 can have today;
   the snapshot itself needs the render path on `V8Host`. `V8Host`'s
   `__oxideDispatch` is a **toy** (2 hardcoded kinds: `render` → a stub HTML
   string, `tooling:compile` → uppercases input) versus the Node worker's 14 real
   kinds. Closing it means a `deno_core::ModuleLoader` over the bundle →
   needs `deno_node` module loading (D2's "not landed"). **The largest item on
   the board; multi-session.**
2. **§9.3 revalidate → cache.** Actions return `x-oxide-revalidate`; the CLI
   applies `path` to L0 but not `tag`, and L1 (`'use cache'`) is not connected.
3. **§16.2 font subsetting + WOFF2** — backend copies verbatim; `write-fonts`
   is a dep but no subsetter wired; providers fetch nothing.
4. **§16.6 web-vitals / `<Script>` / PWA service worker** — unimplemented.
5. **The worker loader refuses `.css`** (session-14 loose end).
6. **§12 HTTP/3, §15.1 JS plugin bridge, §19.3 editor tooling** — untouched.

Sections not touched: §4.4 proper (above), §12, §15.1, §19.3, §22 full set.

---

## 6. Commit history (last 12)

```
c19c56a docs(worklog): dedupe the finding-2 note
b8bbd10 docs(worklog): §21 finding 2 — SSR externals cut Vue's bundle 951 KB → 522 B
80faf32 docs(worklog): §21 finding 2 — SSR externals cut Vue's bundle 951 KB → 522 B
2ae25e8 feat(cli): apply build.ssrExternals to the SSR bundle (§21)
631aad1 feat(config): build.ssrExternals for a slim Node server bundle (§21)
678e8ca docs(worklog): session 16 — §4.4 lint; §21 cold start re-measured; fingerprint temp-file fix
0899a4c test(cli): make the L0 stale-entry test deterministic (§11.1)
c16acc0 test(cli): build_dispatch fixtures carry the L0 + action args (§9.3/§11.1)
ace46e3 fix(cli): exclude *.oxide.ts build temp files from the build fingerprint (§5.1)
483c56e feat(cli): lint snapshot-hostile top-level side effects at build (§4.4)
ac52789 docs(handoff): session 15 — actions, L0 cache, warm build, fonts, sessions
6e7f93e docs(worklog): §9.3 form actions on the server
```

Working tree clean at handoff. (Note: `80faf32`/`b8bbd10` are a duplicate
worklog apply; `c19c56a` removed the doubled section.)
