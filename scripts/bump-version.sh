#!/usr/bin/env bash
# Bump the project version across every surface.
#
# The crates and the npm packages run on independent release cadences
# (§release-v0.2-prep §2: crates 0.18.0, packages 0.7.0), so each
# cadence is bumped and checked separately:
#
# Usage:
#   ./scripts/bump-version.sh --crates 0.18.0            # crates only
#   ./scripts/bump-version.sh --packages 0.7.0           # packages only
#   ./scripts/bump-version.sh --crates 0.18.0 --packages 0.7.0
#   ./scripts/bump-version.sh 0.2.0                      # legacy: both to one version
#   add --dry-run (any position) to show what would change
#
# Surfaces touched:
#   - crates cadence: Cargo.toml [workspace.package] version, and the
#     [workspace.dependencies] oxide-* entries that pin the internal
#     crates (a path dep with an explicit version must be published at
#     that version, so it has to move in lockstep with the crates)
#   - packages cadence: packages/*/package.json "version" field.
#     Publishable and private packages both, so the tree stays
#     self-consistent.
#
# The two cadences do NOT have to agree with each other — that was the
# old "every surface at the same version" rule, which refused to run
# forever once crates (0.17.x) and packages (0.6.x) split cadences.
# What IS still enforced: within a cadence every surface must already
# be at that cadence's current version (drift compounds), and a target
# must differ from the current version (a no-op bump is a mistake).
#
# Not touched:
#   - Cargo.lock  (updated by the next cargo build/check)
#   - pnpm-lock.yaml  (workspace: protocol is version-independent)
#   - git  (no commit; the human reviews the diff first)
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET_CRATES=""
TARGET_PACKAGES=""
DRY_RUN=false

# ---- parse arguments (--dry-run accepted anywhere) ----
ARGS=()
for arg in "$@"; do
  if [ "$arg" = "--dry-run" ]; then
    DRY_RUN=true
  else
    ARGS+=("$arg")
  fi
done

i=0
while [ $i -lt ${#ARGS[@]} ]; do
  case "${ARGS[$i]}" in
    --crates)
      [ $((i + 1)) -lt ${#ARGS[@]} ] || { echo "error: --crates needs a value" >&2; exit 2; }
      TARGET_CRATES="${ARGS[$((i + 1))]}"
      i=$((i + 2))
      ;;
    --packages)
      [ $((i + 1)) -lt ${#ARGS[@]} ] || { echo "error: --packages needs a value" >&2; exit 2; }
      TARGET_PACKAGES="${ARGS[$((i + 1))]}"
      i=$((i + 2))
      ;;
    --*)
      echo "error: unknown option '${ARGS[$i]}'" >&2
      exit 2
      ;;
    *)
      # Legacy positional form: bump both cadences to one version.
      [ -z "$TARGET_CRATES" ] || { echo "error: more than one positional target" >&2; exit 2; }
      TARGET_CRATES="${ARGS[$i]}"
      TARGET_PACKAGES="${ARGS[$i]}"
      i=$((i + 1))
      ;;
  esac
done

if [ -z "$TARGET_CRATES" ] && [ -z "$TARGET_PACKAGES" ]; then
  echo "usage: $0 [--crates X.Y.Z] [--packages X.Y.Z] [--dry-run]" >&2
  echo "       (a bare positional value bumps both cadences to it)" >&2
  exit 2
fi

SEMVER_RE='^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$'
if [ -n "$TARGET_CRATES" ] && ! echo "$TARGET_CRATES" | grep -qE "$SEMVER_RE"; then
  echo "error: '$TARGET_CRATES' is not semver (expected X.Y.Z or X.Y.Z-pre)" >&2
  exit 2
fi
if [ -n "$TARGET_PACKAGES" ] && ! echo "$TARGET_PACKAGES" | grep -qE "$SEMVER_RE"; then
  echo "error: '$TARGET_PACKAGES' is not semver (expected X.Y.Z or X.Y.Z-pre)" >&2
  exit 2
fi

# ---- read the current version of each cadence ----
CURRENT_CRATES=$(awk '/^\[workspace.package\]/{f=1;next} f&&/^version/{gsub(/"/,"");sub(/.*=/,"");gsub(/ /,"");print;exit} f&&/^\[/{exit}' Cargo.toml)
if [ -z "$CURRENT_CRATES" ]; then
  echo "error: could not read [workspace.package] version from Cargo.toml" >&2
  exit 1
fi
# Reference for the packages cadence: @oxide-fw/core, the package every
# release doc assumes is aligned first.
CURRENT_PACKAGES=$(python3 -c "import json;print(json.load(open('packages/core/package.json'))['version'])")
if [ -z "$CURRENT_PACKAGES" ]; then
  echo "error: could not read the version from packages/core/package.json" >&2
  exit 1
fi

echo "current crates version:   $CURRENT_CRATES  -> target: ${TARGET_CRATES:-<unchanged>}"
echo "current packages version: $CURRENT_PACKAGES  -> target: ${TARGET_PACKAGES:-<unchanged>}"
echo

# ---- per-cadence no-op guards ----
if [ -n "$TARGET_CRATES" ] && [ "$CURRENT_CRATES" = "$TARGET_CRATES" ]; then
  echo "error: crates target == current ('$TARGET_CRATES'); refusing a no-op bump" >&2
  exit 1
fi
if [ -n "$TARGET_PACKAGES" ] && [ "$CURRENT_PACKAGES" = "$TARGET_PACKAGES" ]; then
  echo "error: packages target == current ('$TARGET_PACKAGES'); refusing a no-op bump" >&2
  exit 1
fi

# ---- verify every surface is at ITS cadence's current version ----
# Drift inside a cadence compounds; drift BETWEEN cadences is the
# dual-cadence design, not an error.
DRIFT=0

if [ -n "$TARGET_CRATES" ]; then
  echo "checking crates cadence (workspace.dependencies oxide-* entries)..."
  while IFS= read -r line; do
    # A path-only entry (no `version =`) moves with the workspace
    # automatically — there is nothing pinned to drift. Only an entry
    # that explicitly pins a version must be at CURRENT_CRATES.
    echo "$line" | grep -q 'version = "' || continue
    if ! echo "$line" | grep -q "version = \"$CURRENT_CRATES\""; then
      echo "  DRIFT: $line"
      DRIFT=1
    fi
  done < <(grep -E '^oxide-[a-z-]+ = \{ path' Cargo.toml)
fi

if [ -n "$TARGET_PACKAGES" ]; then
  echo "checking packages cadence (packages/*/package.json)..."
  for pkg in packages/*/package.json; do
    ver=$(python3 -c "import json;print(json.load(open('$pkg')).get('version',''))")
    if [ -z "$ver" ]; then
      continue  # oxide-website has no version field
    fi
    if [ "$ver" != "$CURRENT_PACKAGES" ]; then
      echo "  DRIFT: $pkg ($ver != $CURRENT_PACKAGES)"
      DRIFT=1
    fi
  done
fi

if [ "$DRIFT" = "1" ]; then
  echo
  echo "error: version drift detected inside a cadence. Every surface of" >&2
  echo "       that cadence must be at the cadence's current version" >&2
  echo "       before a bump. Fix the drift and rerun." >&2
  exit 1
fi
echo "  no drift — each cadence is internally consistent"
echo

# ---- preview ----
echo "will change:"
if [ -n "$TARGET_CRATES" ]; then
  echo "  Cargo.toml:"
  echo "    [workspace.package] version: $CURRENT_CRATES -> $TARGET_CRATES"
  echo "    N workspace.dependencies oxide-* entries: $CURRENT_CRATES -> $TARGET_CRATES"
fi
if [ -n "$TARGET_PACKAGES" ]; then
  echo "  packages/*/package.json: $CURRENT_PACKAGES -> $TARGET_PACKAGES"
fi
echo

if $DRY_RUN; then
  echo "(--dry-run; nothing written)"
  exit 0
fi

# ---- write the bump ----
echo "writing..."

if [ -n "$TARGET_CRATES" ]; then
  # Cargo.toml: both [workspace.package] and every oxide-* workspace.dependencies line.
  python3 - "$CURRENT_CRATES" "$TARGET_CRATES" <<'PY'
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
fi

if [ -n "$TARGET_PACKAGES" ]; then
  for pkg in packages/*/package.json; do
    python3 - "$CURRENT_PACKAGES" "$TARGET_PACKAGES" "$pkg" <<'PY'
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
fi

# No cargo check here: the bump is text edits, and cargo check is
# minutes of compilation for a verification the human runs once anyway.
# The "next steps" below tells them to.

echo
echo "done. next steps:"
echo "  1. cargo check --workspace          # confirm the tree still builds"
echo "  2. pnpm install --frozen-lockfile   # update the lockfile"
echo "  3. git diff --stat                  # review the file count"
echo "  4. git add Cargo.toml Cargo.lock packages/*/package.json"
echo "  5. git commit -m 'chore(release): bump to v${TARGET_CRATES:-$TARGET_PACKAGES}'"
echo "  6. git tag -a v${TARGET_CRATES:-$TARGET_PACKAGES} -m 'v${TARGET_CRATES:-$TARGET_PACKAGES}'"
echo "  7. git push origin main v${TARGET_CRATES:-$TARGET_PACKAGES}    # release.yml regenerates CHANGELOG"
