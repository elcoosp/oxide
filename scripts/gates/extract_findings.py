#!/usr/bin/env python3
"""Extract clippy findings from a clippy text log as `file:line:col\\tlint\\tmsg`.

Usage: python3 scripts/gates/extract_findings.py <clippy.log>

Pairs each `error:`/`warning:` line with its `--> file:line:col` location and
the lint name from the `= note: -W clippy::<lint>` line that follows. Output is
sorted by file then line, deduplicated.
"""
import sys
import re

def main(path):
    with open(path, encoding="utf-8", errors="replace") as fh:
        lines = fh.read().split("\n")
    out = []
    cur_msg = None
    cur_loc = None
    cur_lint = None
    for l in lines:
        if l.startswith("error:") or l.startswith("warning:"):
            # flush previous
            if cur_loc and cur_msg:
                out.append((cur_loc, cur_lint or "?", cur_msg))
            cur_msg = l.split(":", 1)[1].strip()
            cur_loc = None
            cur_lint = None
            if "could not" in cur_msg or "build failed" in cur_msg \
               or "generated" in cur_msg or "unused manifest" in cur_msg:
                cur_msg = None
        elif l.startswith("   -->"):
            cur_loc = l.split()[1]
        elif "= note: `-W clippy::" in l or "= note: `-D clippy::" in l:
            m = re.search(r"clippy::([a-z_]+)", l)
            if m:
                cur_lint = m.group(0)
    if cur_loc and cur_msg:
        out.append((cur_loc, cur_lint or "?", cur_msg))
    # sort by file then line
    def key(item):
        loc = item[0]
        m = re.match(r"([^:]+):(\d+):(\d+)", loc)
        if m:
            return (m.group(1), int(m.group(2)), int(m.group(3)))
        return (loc, 0, 0)
    seen = set()
    for loc, lint, msg in sorted(out, key=key):
        if (loc, lint) in seen:
            continue
        seen.add((loc, lint))
        print(f"{loc}\t{lint}\t{msg}")

if __name__ == "__main__":
    main(sys.argv[1])
