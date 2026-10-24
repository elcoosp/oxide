#!/usr/bin/env bash
# Bump the project version across every surface.
#
# Usage:
#   ./scripts/bump-version.sh 0.2.0            # write the bump
#   ./scripts/bump-version.sh 0.2.0 --dry-run  # show what would change
#
# Surfaces touched:
#   - Cargo.toml: [workspace.package] version, and the 16
#     [workspace.dependencies] oxide-* entries that pin the internal
#     crates (a path dep with an explicit version must be published at
#     that version, so it has to move in lockstep with the crates)
#   - packages/*/package.json: the "version" field. Publishable and
#     private packages both, so the tree stays self-consistent.
#
# Not touched:
#   - Cargo.lock  (updated by the next cargo build/check)
#   - pnpm-lock.yaml  (workspace: protocol is version-independent)
#   - git  (no commit; the human reviews the diff first)
#
# Refuses to run if:
#   - the current versions are already inconsistent (drift compounds)
#   - the target equals the current version (a no-op bump is a mistake)
#   - the target is not semver
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="${1:-}"
DRY_RUN=false
if [ "${2:-}" = "--dry-run" ]; then
  DRY_RUN=true
fi

if [ -z "$TARGET" ]; then
  echo "usage: $0 <semver> [--dry-run]" >&2
  exit 2
fi
if ! echo "$TARGET" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$'; then
  echo "error: '$TARGET' is not semver (expected X.Y.Z or X.Y.Z-pre)" >&2
  exit 2
fi

# ---- read current version from Cargo.toml [workspace.package] ----
CURRENT=$(awk '/^\[workspace.package\]/{f=1;next} f&&/^version/{gsub(/"/,"");sub(/.*=/,"");gsub(/ /,"");print;exit} f&&/^\[/{exit}' Cargo.toml)
if [ -z "$CURRENT" ]; then
  echo "error: could not read [workspace.package] version from Cargo.toml" >&2
  exit 1
fi
echo "current version: $CURRENT"
echo "target version:  $TARGET"
echo

if [ "$CURRENT" = "$TARGET" ]; then
  echo "error: target == current ('$TARGET'); refusing a no-op bump" >&2
  exit 1
fi

# ---- verify every surface is already at CURRENT (no drift) ----
DRIFT=0

# 1. workspace.dependencies oxide-* lines
echo "checking workspace.dependencies..."
while IFS= read -r line; do
  if ! echo "$line" | grep -q "version = \"$CURRENT\""; then
    echo "  DRIFT: $line"
    DRIFT=1
  fi
done < <(grep -E '^oxide-[a-z-]+ = \{ path' Cargo.toml)

# 2. npm package.json files
echo "checking npm packages..."
for pkg in packages/*/package.json; do
  ver=$(python3 -c "import json;print(json.load(open('$pkg')).get('version',''))")
  if [ -z "$ver" ]; then
    continue  # oxide-website has no version field
  fi
  if [ "$ver" != "$CURRENT" ]; then
    echo "  DRIFT: $pkg ($ver)"
    DRIFT=1
  fi
done

if [ "$DRIFT" = "1" ]; then
  echo
  echo "error: version drift detected. Every surface must be at the same" >&2
  echo "       version before a bump. Fix the drift (or reset to 0.1.0)" >&2
  echo "       and rerun." >&2
  exit 1
fi
echo "  no drift — every surface is at $CURRENT"
echo

# ---- preview ----
echo "will change:"
echo "  Cargo.toml:"
echo "    [workspace.package] version: $CURRENT -> $TARGET"
echo "    N workspace.dependencies oxide-* entries: $CURRENT -> $TARGET"
echo "  packages/*/package.json: $CURRENT -> $TARGET"
echo

if $DRY_RUN; then
  echo "(--dry-run; nothing written)"
  exit 0
fi

# ---- write the bump ----
echo "writing..."

# 1. Cargo.toml: both [workspace.package] and every oxide-* workspace.dependencies line.
python3 - "$CURRENT" "$TARGET" <<'PY'
import re, sys
current, target = sys.argv[1], sys.argv[2]
path = "Cargo.toml"
src = open(path).read()

# workspace.package version (only in that section)
def bump_workspace_package(m):
    body = m.group(0)
    body = re.sub(rf'^version = "{re.escape(current)}"', f'version = "{target}"', body, flags=re.MULTILINE)
    return body

src = re.sub(r'^\[workspace\.package\]\n(?:[^\[].*\n)*', bump_workspace_package, src, flags=re.MULTILINE)

# workspace.dependencies oxide-* entries
src = re.sub(
    rf'^(oxide-[a-z-]+ = \{{ path = "[^"]+", version = )"{re.escape(current)}"',
    rf'\1"{target}"',
    src, flags=re.MULTILINE
)
open(path, "w").write(src)
print("  Cargo.toml updated")
PY

# 2. npm package.json files
for pkg in packages/*/package.json; do
  python3 - "$CURRENT" "$TARGET" "$pkg" <<'PY'
import json, sys
current, target, path = sys.argv[1], sys.argv[2], sys.argv[3]
d = json.load(open(path))
if "version" not in d:
    sys.exit(0)
if d["version"] != current:
    sys.exit(0)  # verified earlier, shouldn't happen
d["version"] = target
open(path, "w").write(json.dumps(d, indent=2) + "\n")
print(f"  {path} updated")
PY
done

# No cargo check here: the bump is text edits, and cargo check is
# minutes of compilation for a verification the human runs once anyway.
# The "next steps" below tells them to.

echo
echo "done. next steps:"
echo "  1. cargo check --workspace          # confirm the tree still builds"
echo "  2. pnpm install --frozen-lockfile   # update the lockfile"
echo "  3. git diff --stat                  # review the file count"
echo "  4. git add Cargo.toml Cargo.lock packages/*/package.json"
echo "  5. git commit -m 'chore(release): bump to v$TARGET'"
echo "  6. git tag -a v$TARGET -m 'v$TARGET'"
echo "  7. git push origin main v$TARGET    # release.yml regenerates CHANGELOG"
