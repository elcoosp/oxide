#!/usr/bin/env bash
# Prints the oxide-* crates in dependency order (leaf first).
#
# Derived from `cargo metadata` — not hand-maintained. Every crate
# appears after every crate it depends on. Used by PUBLISHING.md and by
# CI's release-prep checks so the doc and the graph cannot drift.
#
# Usage:
#   ./scripts/publish-order.sh          # one crate per line
#   ./scripts/publish-order.sh --dry-run # prefixed with `cargo publish --dry-run -p`
#   ./scripts/publish-order.sh --publish # prefixed with `cargo publish -p`
set -euo pipefail

mode=""
case "${1:-}" in
  --dry-run) mode="cargo publish --dry-run -p" ;;
  --publish) mode="cargo publish -p" ;;
  "") mode="" ;;
  *) echo "unknown flag: $1" >&2; exit 2 ;;
esac

python3 <<'PY'
import json, subprocess, sys

meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--no-deps"]
))
pkgs = {p["name"]: p for p in meta["packages"] if p["name"].startswith("oxide-")}

deps = {name: set() for name in pkgs}
for name, p in pkgs.items():
    for d in p.get("dependencies", []):
        dn = d.get("name", "")
        if dn in pkgs and dn != name:
            deps[name].add(dn)

emitted = set()
order = []
while len(order) < len(pkgs):
    ready = sorted(n for n in pkgs if n not in emitted and deps[n] <= emitted)
    if not ready:
        print(f"error: dependency cycle among {sorted(set(pkgs) - emitted)}", file=sys.stderr)
        sys.exit(1)
    for n in ready:
        order.append(n)
        emitted.add(n)

mode = sys.argv[1] if len(sys.argv) > 1 else ""
for n in order:
    if mode:
        print(f"{mode} {n}")
    else:
        print(n)
PY
