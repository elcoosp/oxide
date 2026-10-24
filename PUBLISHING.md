# Publishing Oxide

Runbook for cutting a release.

## 0. Version bumps

Rust: crates inherit from `[workspace.package]` in root `Cargo.toml`:

    sed -i.bak 's/^version = "0\.1\.0"/version = "0.2.0"/' Cargo.toml

npm: each package declares its own version:

    for p in create-oxide @oxide-fw/core @oxide-fw/react @oxide-fw/vue @oxide-fw/svelte @oxide-fw/solid; do
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

## Authentication — OIDC Trusted Publishing

Both registries support OpenID Connect Trusted Publishing, which removes
the need for long-lived tokens. The exchange: the CI job requests a
short-lived OIDC token from GitHub, the registry validates it against
the trusted-publisher config for the package, and issues a write
credential scoped to that job.

### One-time prerequisite: publish v0.1.0 manually

Trusted Publishing only applies to packages that **already exist** on
the registry. The first publish of every crate and every npm package
must use a token.

**Crates (17)** — from a clean tree, in dependency order:

    ORDER=$(./scripts/publish-order.sh)
    for c in $ORDER; do
      cargo publish -p "$c" || break
      sleep 30
    done

`~/.cargo/credentials.toml` must have a valid `crates-io` token.
`crates.io` does not allow unpublishing; a mistake in this loop
supersedes with a patch release, never overwrites.

**npm (6)** — after the crates are published (the `oxide` package
imports nothing from crates.io, but the semantic version of the crates
matches):

    for pkg in create-oxide oxide react vue svelte solid; do
      ( cd "packages/$pkg" && pnpm publish --access public --no-git-checks )
    done

`~/.npmrc` must have `//registry.npmjs.org/:_authToken=<token>`. The
`@oxide` scope must be registered on npmjs.com before publishing.

### Configure trusted publishers (manual UI, once per package)

**crates.io** — for each of the 17 crates, after the first publish:
1. Go to `https://crates.io/crates/<crate>/settings`
2. Add a Trusted Publisher:
   - Repository owner: `elcoosp`
   - Repository name: `oxide`
   - Workflow filename: `release-plz.yml` (or whatever workflow will
     run `cargo publish`)
   - Environment: leave blank unless you use GitHub Environments

**npmjs.com** — for each of the 6 packages, after the first publish:
1. Go to the package page → Settings → Trusted Publisher
2. Choose "GitHub Actions"
3. Fill in:
   - Organization or user: `elcoosp`
   - Repository: `oxide`
   - Workflow filename: `publish-npm.yml`
   - Environment: leave blank

Once configured, `publish-npm.yml` (this repo) authenticates via OIDC —
no `NPM_TOKEN` secret. The workflow uses `id-token: write` and leaves
`NODE_AUTH_TOKEN` empty; `pnpm publish --provenance` does the OIDC
exchange and attaches an attestation the registry records.

Crate publishing has no CI workflow yet. `release-plz.yml` is the
natural home — its `cargo publish` step uses the same OIDC trust. Until
that workflow exists, crate publishing is manual from a developer
machine, and the trusted publisher config is dormant.

## Using publish-npm.yml

After a tag push and the `release.yml` job runs:

1. Verify the crates are on crates.io (`cargo info oxide-fw-cli`)
2. Trigger the workflow manually:
   - GitHub → Actions → publish-npm → Run workflow
   - **version**: `0.1.0` (must equal every package.json)
   - **dry_run**: leave `true` on the first run
3. The dry-run verifies:
   - every package.json is at the input version
   - the tarballs contain exactly `src/` + automatic inclusions
   - OIDC login succeeds (proves trusted publishers configured)
   - `pnpm -r publish --dry-run` would publish the six packages
4. If the dry-run is clean, re-trigger with **dry_run**: `false`

`pnpm -r publish` walks every non-private workspace package, so
`oxide-website` and `oxide-og` (both `private: true`) are skipped
automatically.

## What has not been exercised

- **The crate publish loop**: the `scripts/publish-order.sh` script has
  been verified locally; the actual `cargo publish` loop has not been
  run against crates.io. Only the true leaf (`oxide-diagnostics`) is
  fully validatable pre-publish; every other crate's manifest is
  validated only the moment it publishes (see 3a).
- **The npm OIDC publish**: `publish-npm.yml` has never run. It will
  fail on the first `npm whoami` if the trusted publisher is not
  configured for the package being published.
- **The first publish itself**: everything is at 0.1.0, nothing is
  published to either registry yet. The first publish of each crate
  and package is a token-driven manual action; OIDC takes over from
  the second release onward.
- **Rollback**: published versions cannot be unpublished. A bad
  release supersedes with a patch release.

## Deprecation policy

Before v1.0: breaking changes announced in the CHANGELOG, may land in any
minor. After v1.0: breaking changes require a major bump; minor bumps are
additive only.
