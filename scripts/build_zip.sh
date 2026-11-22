#!/usr/bin/env bash
# V2-R01 deliverable zip — the FULL tree (git ls-files + the explicit
# untracked enumeration: ship.toml, .github/pr-body.md, and every new file
# this milestone created). Excludes ONLY .git/, target/, node_modules/,
# caches — never src/tests/docs/config/lock/CI. Not changed-only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
SLUG="conformance-v2-registry"
OUT="$HOME/Downloads/oxide-${SLUG}.zip"

# 1. The tracked oxide repo (git ls-files excludes gitignored node_modules/
#    target/, so the tracked set is already clean).
git ls-files > /tmp/oxide_zip_tracked.txt

# 2. The explicit untracked enumeration: every new file this milestone
#    created (the `??` files from git status, MINUS the agent-workspace dirs
#    download/upload/skills/.env which are sandbox scaffolding, not oxide
#    content), PLUS ship.toml + .github/pr-body.md (gitignored, but the
#    deliverable manifest + the PR body the ship tooling consumes).
git status --porcelain --untracked-files=all \
  | grep '^??' \
  | sed 's/^?? //' \
  | grep -vE '^(download/|upload/|skills/|\.env$|node_modules/|target/)' \
  > /tmp/oxide_zip_untracked.txt
printf 'ship.toml\n.github/pr-body.md\n' >> /tmp/oxide_zip_untracked.txt

# 3. The union, deduped, with the excludes belt-and-braced out.
cat /tmp/oxide_zip_tracked.txt /tmp/oxide_zip_untracked.txt \
  | sort -u \
  | grep -vE '^(\.git/|target/|node_modules/|\.oxide/cache/|dist/|__pycache__/)' \
  > /tmp/oxide_zip_list.txt

# 4. Zip (store paths relative to the repo root).
mkdir -p "$HOME/Downloads"
rm -f "$OUT"
zip -q -r "$OUT" -@ < /tmp/oxide_zip_list.txt

# 5. Also write to the sandbox's user-visible download dir.
cp "$OUT" "$ROOT/download/oxide-${SLUG}.zip"

# 6. Prove: the must-haves are present, the excludes are absent.
echo "=== zip proof ==="
echo "zip: $OUT  ($(du -h "$OUT" | cut -f1))"
echo "sandbox copy: $ROOT/download/oxide-${SLUG}.zip  ($(du -h "$ROOT/download/oxide-${SLUG}.zip" | cut -f1))"
echo "files in zip: $(wc -l < /tmp/oxide_zip_list.txt)"
echo "--- must be present ---"
for must in Cargo.toml Cargo.lock .gitignore .github/workflows/ci.yml crates/adapters/src/lib.rs conformance/suite.ts; do
  grep -qxF "$must" /tmp/oxide_zip_list.txt && echo "  present: $must" || echo "  MISSING: $must"
done
echo "--- src present (a sample of crate src dirs) ---"
grep -E '^crates/[a-z]+/src/' /tmp/oxide_zip_list.txt | head -3
echo "--- must be ABSENT (no escape) ---"
unzip -l "$OUT" | grep -E '^\s.*(\.git/|target/|node_modules/)' | head -3 && echo "  ESCAPE FOUND (bad)" || echo "  none (good)"
echo "--- ship.toml + pr-body present ---"
grep -qxF 'ship.toml' /tmp/oxide_zip_list.txt && echo "  present: ship.toml"
grep -qxF '.github/pr-body.md' /tmp/oxide_zip_list.txt && echo "  present: .github/pr-body.md"
