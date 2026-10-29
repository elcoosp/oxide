# Contributing to Oxide

Thanks for contributing. This file covers the workflow and the two
non-obvious decisions (pnpm configuration, soak rule) that a new
contributor would otherwise trip over.

## Prerequisites

- Rust **1.98** (the `rust-toolchain.toml` pins it; `rustup` picks it up)
- Node **>= 24**
- pnpm **12.16.1** — exact pin in `package.json#packageManager`. Corepack
  activates it: `corepack enable && corepack prepare pnpm@12.16.1 --activate`
- A C linker (for `wasmtime` / `ring` / `ravif` native builds)

## Workflow

```bash
git clone https://github.com/elcoosp/oxide
cd oxide
corepack pnpm install --frozen-lockfile
cargo build --workspace --all-targets
```

A change is ready when every gate below passes **locally**:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --no-fail-fast
cargo audit
pnpm -r --filter '!./packages/oxide-website' --filter '!./examples/*' run build
for p in core react vue svelte solid; do pnpm --filter "@oxide-fw/$p" test; done
pnpm -C conformance test
./scripts/check-launch-content.sh
```

CI runs exactly these on Linux, macOS, and Windows. Do not open a PR
against a tree that fails any of them locally.

## Commit conventions

Conventional Commits, with a scope:

```
<type>(<scope>): <subject>
```

- Types: `feat`, `fix`, `chore`, `docs`, `refactor`, `test`, `ci`, `build`, `style`, `perf`
- Scopes: the crate or package (`cli`, `jshost`, `cache`, `react`, `ci`, …)
- Changelog is generated from these by `git-cliff` (`cliff.toml`); an
  unparseable message is silently dropped from the release notes.

## Two non-obvious decisions

### pnpm configuration lives in `pnpm-workspace.yaml`

pnpm 11+ **ignores** the `package.json#pnpm` field and only reads
auth/registry keys from `.npmrc`. Every project setting (node linker, peer
resolution, `allowBuilds`) lives in `pnpm-workspace.yaml`. Do not move them
back to `.npmrc` — they will be silently ignored.

`allowBuilds: { esbuild: false }` is a *reviewed denial*, not a bug.
`esbuild` resolves its platform binary through `optionalDependencies`
(`@esbuild/<platform>-<arch>`), not its postinstall. Denying the postinstall
keeps `pnpm install --frozen-lockfile` green without running arbitrary
install scripts. If you add a dependency that genuinely needs a postinstall,
add it here with `true` — do not remove `strictDepBuilds: false`.

`nodeLinker` is deliberately **not** set. pnpm 12's hoisted linker creates
peer-qualified `.pnpm` stubs without materialising package content, which
breaks `tsdown`'s `.bin` shim (`MODULE_NOT_FOUND: .../tsdown/dist/run.mjs`).
Isolated (the default) uses canonical entries and works.

### Soak rule before a tag

`PUBLISHING.md` step 2 requires **5 consecutive green CI runs across 48
hours** before a `v*` tag. A single red run resets the clock. Do not tag a
commit that CI has not soaked.

## Publishing

See `PUBLISHING.md`. The short version: crate and npm publishes are manual
(`scripts/publish-order.sh --publish`, then `publish-npm.yml` with
`dry_run: false`). CI creates the GitHub Release and regenerates
`CHANGELOG.md` on tag push; it does not publish to registries.

## Reporting bugs

Open a GitHub issue using the template. For security issues, see
`SECURITY.md` — do not open a public issue.
