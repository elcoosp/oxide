# Publishing Oxide

Runbook for cutting a release.

## 0. Version bumps

Rust: crates inherit from `[workspace.package]` in root `Cargo.toml`:

    sed -i.bak 's/^version = "0\.1\.0"/version = "0.2.0"/' Cargo.toml

npm: each package declares its own version:

    for p in create-oxide oxide @oxide/react @oxide/vue @oxide/svelte @oxide/solid; do
      pnpm --filter "$p" version 0.2.0 --no-git-tag-version
    done

Verify: `cargo check --workspace && pnpm install --frozen-lockfile`.

## 1. Regenerate the CHANGELOG

    git-cliff --unreleased --tag v0.2.0 -o CHANGELOG.md

The release.yml workflow runs this automatically on a v*.*.* tag push.

## 2. Pre-release gates

    git status --short              # must be empty
    cargo fmt --all --check
    cargo test --workspace
    bash scripts/check-spec-anchors.sh
    bash scripts/check-launch-content.sh
    bash scripts/check-stale-strings.sh

Never tag from a red tree. Soak rule: main green for at least 5
consecutive runs across 48 hours before the tag.

## 3. Crate publish order

**The order is derived, not hand-maintained.** Run:

    ./scripts/publish-order.sh

to print the topological sort (leaf first). The script reads
`cargo metadata` — every crate appears after every crate it depends
on. Current output has 17 crates; `oxide-diagnostics` is the only
true leaf.

### 3a. Local packaging check — only the true leaf works pre-publish

**Hard fact about crates.io first-publish mechanics:** you cannot
dry-run a non-leaf crate until its dependencies are actually on
crates.io. `cargo package` needs to resolve every dependency to write
the publishable Cargo.toml (path → version); `cargo publish --dry-run`
additionally queries the registry for name availability.

What this means in practice:

- **`oxide-diagnostics`** (the only true leaf) can be validated before
  any publish:

      cargo package -p oxide-diagnostics --allow-dirty

- **Every other crate** (16 of 17) cannot be validated before its deps
  are published. `cargo package -p oxide-odf --no-verify --offline`
  still fails at dep resolution with "no matching package named
  oxide-diagnostics found". The `--offline` flag doesn't help because
  oxide-diagnostics genuinely isn't on the registry.

This is the **irreducible risk of the first publish**. The mitigation
is:

1. Validate the leaf crate thoroughly (it's the foundation).
2. Publish leaf crates one at a time, checking crates.io after each
   (cargo info <name>; ensure the version resolves).
3. If a mid-graph crate fails on its own manifest (missing field,
   invalid file pattern, forbidden dependency), that failure surfaces
   only after the earlier crates are already published — and
   crates.io does not allow unpublishing. The published crates at
   v0.1.0 will be superseded by v0.1.1, not retracted.

Run this against a **clean tree** — cargo refuses to package with
uncommitted changes unless `--allow-dirty` is passed. For a real
release, commit everything first.

### 3b. Real publish — in dependency order

    ./scripts/publish-order.sh --publish | while read -r cmd; do
      $cmd || break
      sleep 30   # crates.io index propagation between publishes
    done

The `sleep 30` matters: after publishing `oxide-diagnostics`, the
index needs a moment before `oxide-odf`'s publish can resolve it.

## 4. npm package publish

    for p in packages/create-oxide packages/oxide packages/react \
             packages/vue packages/svelte packages/solid; do
      ( cd "$p" && pnpm pack --dry-run 2>&1 | tail -10 )
    done

The tarball should show only src/ plus auto-included package.json,
README.md, LICENSE. If test/ or tsconfig.json appears, the files
field is wrong.

Publish:

    pnpm -r publish --access public --no-git-checks

oxide-website and oxide-og are private: true and skipped.

## 5. Tag and GitHub Release

    git tag -a v0.2.0 -m "v0.2.0"
    git push origin v0.2.0

release.yml on tag push regenerates CHANGELOG, commits it back, and
creates the GitHub Release. It does NOT publish crates or packages —
those stay manual so a failure can be inspected.

## What has not been exercised

- Crate publishing: requires a crates.io token in ~/.cargo/credentials.toml
- npm publishing: requires the @oxide scope registered and a token in ~/.npmrc
- The publish-order script has been verified locally; the publish loop
  itself has not been run against crates.io
- Only the true leaf crate (oxide-diagnostics) is fully validatable
  pre-publish; every other crate's manifest is validated only the
  moment it publishes (see 3a for why)
- Rollback: published versions cannot be unpublished

## Deprecation policy

Before v1.0: breaking changes announced in the CHANGELOG, may land in any
minor. After v1.0: breaking changes require a major bump; minor bumps are
additive only.
