#!/usr/bin/env bash
# scripts/dist-digest.sh <app-dir> : deterministic digest of dist/ (builds are byte-stable
# per project). WP-A01 Step 0: the behavior-preserving proof for the cli split is that the
# four smoke apps' dist trees hash identically before and after the move; CI builds each
# examples/smoke-app-* and pins the four digests (see .github/workflows/ci.yml, the
# `cli phase-1 split dist-digest` job).
set -euo pipefail
cd "$1/dist"
find . -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1
