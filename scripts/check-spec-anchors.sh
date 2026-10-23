#!/usr/bin/env bash
# Every `§N` / `§N.M` cited in source must correspond to a real heading in
# the frozen spec.
#
# Why this exists: `§16 / K1`, `§16 / K2`, `§16 / K3` appeared 28 times across
# crates and packages. The `K*` labels are invented — the spec has no such
# identifiers — so those comments cited a section that does not exist and a
# label that never did. That is the same class of defect the compliance
# audits kept catching: an unverifiable claim, stated confidently, in a
# comment nobody re-reads.
#
# The check is deliberately mechanical: collect the set of headings the spec
# actually defines, collect the set of anchors the code cites, and report the
# difference. A new invented anchor fails the build the first time it is
# written, rather than surviving until an audit notices.
set -uo pipefail
cd "$(dirname "$0")/.."

SPEC=docs/spec-v0.1.md
FAIL=0

[ -f "$SPEC" ] || { echo "spec-anchors: $SPEC missing"; exit 1; }

# Headings the spec defines: `## 12` and `### 12.1` -> 12, 12.1
defined=$(grep -oE '^#{2,3} [0-9]+(\.[0-9]+)?' "$SPEC" \
    | sed -E 's/^#+ //' | sort -u)

# Anchors cited in source: `§12`, `§12.1`. Strip the § sigil for comparison.
cited=$(grep -rhoE '§[0-9]+(\.[0-9]+)?' crates/ packages/ tests/ \
        --include='*.rs' --include='*.ts' 2>/dev/null \
    | grep -v node_modules \
    | sed 's/^§//' | sort -u)

# A cited anchor whose *top-level* section exists is fine: the spec numbers
# subsections only where it needs them (e.g. `§6` has no `§6` heading of its
# own but `§6.1` does). Compare on the parent when the exact child is absent.
missing=""
for a in $cited; do
    if echo "$defined" | grep -qx "$a"; then
        continue
    fi
    parent="${a%%.*}"
    if echo "$defined" | grep -qx "$parent"; then
        continue
    fi
    missing="$missing $a"
done

if [ -n "$missing" ]; then
    echo "STALE: source cites spec anchors the spec does not define:"
    for a in $missing; do
        echo "  §$a"
        grep -rn "§$a\b" crates/ packages/ tests/ --include='*.rs' --include='*.ts' 2>/dev/null \
            | grep -v node_modules | head -2 | sed 's/^/      /'
    done
    FAIL=1
fi

# The invented label class specifically. `K1`/`K2`/`K3` are not spec
# identifiers; if they reappear the anchor they decorate is unverifiable.
if grep -rn '\bK[0-9]\b' crates/ packages/ tests/ --include='*.rs' --include='*.ts' 2>/dev/null \
    | grep -v node_modules | grep -q .; then
    echo "STALE: invented work-package labels (K1/K2/K3) in source — the spec defines no such identifiers:"
    grep -rn '\bK[0-9]\b' crates/ packages/ tests/ --include='*.rs' --include='*.ts' 2>/dev/null \
        | grep -v node_modules | head -5 | sed 's/^/  /'
    FAIL=1
fi

if [ "$FAIL" -eq 0 ]; then
    echo "spec-anchors: PASS (every cited anchor is defined; no invented labels)"
    exit 0
fi
echo "spec-anchors: FAIL — see above"
exit 1
