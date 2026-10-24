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

Crates publish after their deps are on crates.io:

Layer 1 (leaf): oxide-diagnostics, oxide-odf, oxide-router, oxide-config
Layer 2:        oxide-graph, oxide-cache, oxide-compiler, oxide-assets
Layer 3:        oxide-bundler, oxide-jshost, oxide-plugin, oxide-render,
                oxide-server, oxide-dev, oxide-adapters, oxide-core-wasm
Layer 4 (top):  oxide-cli

Dry-run then publish:

    ORDER="oxide-diagnostics oxide-odf oxide-router oxide-config \
           oxide-graph oxide-cache oxide-compiler oxide-assets \
           oxide-bundler oxide-jshost oxide-plugin oxide-render \
           oxide-server oxide-dev oxide-adapters oxide-core-wasm \
           oxide-cli"
    for c in $ORDER; do
      cargo publish --dry-run -p "$c" || break
    done
    for c in $ORDER; do
      cargo publish -p "$c" || break
      sleep 30   # crates.io index propagation
    done

## 4. npm package publish

    for p in packages/create-oxide packages/oxide packages/react \
             packages/vue packages/svelte packages/solid; do
      ( cd "$p" && pnpm pack --dry-run 2>&1 | tail -10 )
    done

The tarball should show only src/ plus auto-included package.json,
README.md, LICENSE. If test/ or tsconfig.json appears, the files field
is wrong.

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
- The dry-run loops above have not run in CI
- Rollback: published versions cannot be unpublished

## Deprecation policy

Before v1.0: breaking changes announced in the CHANGELOG, may land in any
minor. After v1.0: breaking changes require a major bump; minor bumps are
additive only.
