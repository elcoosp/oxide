# Oxide Contributor Conventions (agents MUST read this)

Spec: `/home/z/my-project/upload/Oxide Technical Specification v0.1.md` — section references (§) point there.
Repo: `/home/z/my-project/oxide/` (git). Toolchain: Rust 1.99 stable (already installed; `. "$HOME/.cargo/env"` first).

## Ground rules

1. **TDD is mandatory.** For each feature: write the failing test first, run it (`cargo test -p <pkg>`),
   implement until green. Every public function in your crate MUST have at least one test exercising it.
2. **Own your crate.** You may ONLY edit files under your assigned crate directories (and, when explicitly
   stated, a designated shared file). Never edit another crate — if you need a change there, work around it
   and note it in the worklog.
3. **Do not change any public API signature in the stub files.** They are cross-crate contracts other
   agents already build against. You may ADD items (extra pub fns, modules, tests, internal types) freely.
4. **Replace `todo!()` with real implementations.** Every `todo!()` in your crate must be gone when you finish.
5. Run scoped commands only: `cargo test -p oxide-<crate>` (unit) and `cargo test -p oxide-<crate> --test <name>`.
   The workspace shares one target dir — builds may wait on the lock; use `timeout 550` on cargo commands.
   DO NOT run `cargo test --workspace` (the integrator runs it).
6. Error handling: return `Result<T>` (crate-local alias over `oxide_diagnostics::OxideError`) with stable
   codes `PREFIX-NNN` (e.g. `ODF-104`). Use `crate::err(code, msg)`. Add `.with_fix()` when a known fix exists (§19.4).
7. Style: rustfmt defaults (`cargo fmt -p oxide-<crate>` before finishing). No `unsafe`. No new external
   dependencies without a workspace entry — workspace deps are pinned in the root `Cargo.toml`
   (`use workspace = true`). If you truly need an unpinned dep, prefer implementing it yourself.
8. Tests must be deterministic (no sleeps > 50ms, no network). Use `tempfile` for FS fixtures.
9. Doc comments: every pub item gets a `///` with the spec § reference where applicable.
10. When done: `cargo test -p ...` green → append your worklog section (template at the bottom of
    `/home/z/my-project/worklog.md`) → commit with `git add <your paths> && git commit -m "<crate>: ..."`.

## Shared idioms

- `Value` types are `Clone` + `PartialEq` where practical (tests compare deeply).
- Determinism: any hash-based ordering must sort before use; iteration over maps uses `BTreeMap` unless
  insertion order is contractual.
- Serialization shapes use `serde` with `camelCase` rename to match the TS config keys (§14).
- Bytes on the wire/storage: `blake3` content hashing everywhere (D6), zstd compression for storage (§5.4).
- The JS boundary speaks JSON (diagnostics) and ODF bytes (data, §9.5).

## Golden fixtures

Crate `oxide-odf` writes golden fixtures to `crates/odf/tests/fixtures/*.odf.bin` + `*.expected.json`
(a small binary `examples/odf_fixture.rs` or test writes them). The TS decoder test suite
(`packages/oxide/test/odf.test.ts`) consumes the same files — keep them committed.

## Worklog protocol

Append to `/home/z/my-project/worklog.md` (never overwrite):

```
---
Task ID: <id>
Agent: <name>
Task: <what you were asked>

Work Log:
- <steps>

Stage Summary:
- <results, decisions, deviations>
```

## Path canonicalization at command entry

Every CLI command that takes a `--root` argument must canonicalize it
once at the top of the function:

    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));

Downstream joins (`root.join("dist")`, `routes_root.join(e.file)`,
bundler entry resolution) all expect absolute paths. When `root` is
relative and a downstream layer joins it to a path that already carries
the root prefix, the result is `examples/smoke-app-react/examples/smoke-app-react/...`
— a silently doubled path that produces "file not found" errors far
from the actual bug.

`PathBuf::join` returns the right-hand side unchanged when it is
absolute, so canonicalizing once at entry short-circuits the doubling
in every downstream join. The `unwrap_or_else` fallback preserves prior
behavior for paths that don't exist yet (tempdirs in tests).

This bug has bitten three times in the same session — cmd_build's
route scan, its bundler invocation, and cmd_serve's config lookup. If
a new command takes a root, canonicalize before it does anything else.
