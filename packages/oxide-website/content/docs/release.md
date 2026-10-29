---
title: Release process
description: The soak rule, why the first publish is manual, and what CI does — and does not — do on a tag.
order: 9
---

# Release process

`PUBLISHING.md` at the repository root is the authoritative procedure. This
page is the shape of it.

## The soak rule

> **Never tag from a red tree. Soak rule: main green for at least 5 consecutive
> runs across 48 hours before the tag.**

A single red run resets the clock. The rule exists because the workflows that
publish are the ones least exercised — `release.yml` fires on a tag and
`publish-npm.yml` fires on manual dispatch, so neither runs on a normal push.
A tag is the first time they execute.

## What CI does on a tag

`release.yml` triggers on `v[0-9]+.[0-9]+.[0-9]+` and:

1. regenerates `CHANGELOG.md` with git-cliff from the commit history
2. extracts this tag's section for the release body
3. commits the regenerated changelog back to `main`
4. creates the GitHub Release

It does **not** publish to crates.io or npm. A CI publish of an immutable
registry is not safely retryable, so a human drives it.

## Why the first publish is manual

Both registries require the name to exist before trusted publishing can be
configured for it:

- **crates.io** — a token minted for an existing crate cannot create a new
  crate name. `oxide-fw-diagnostics` goes first (it is the only true leaf);
  each other crate publishes once its dependencies are on the registry.
- **npm** — the package must exist before a Trusted Publisher can be attached.
  The first publish uses an `NPM_TOKEN` from your machine; every publish after
  uses OIDC.

`scripts/publish-order.sh` derives the crate order from `cargo metadata`: leaf
first, every crate after every crate it depends on. The loop sleeps 30 seconds
between publishes because the crates.io index needs a moment before a
dependent crate's publish can resolve its parent.

## The npm workflow

`publish-npm.yml` is a `workflow_dispatch` with two inputs: `version` and
`dry_run`. `dry_run` defaults to **true**. It verifies that every publishable
package is at the input version, builds the workspace (each package's `files`
field is `["dist", …]` and a fresh checkout has no `dist/`), packs each to
confirm the tarball is not empty, and only then publishes.

Run it with `dry_run: true` first. Always.
