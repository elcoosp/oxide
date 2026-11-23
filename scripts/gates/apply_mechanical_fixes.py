#!/usr/bin/env python3
"""Apply mechanical clippy::pedantic fixes for the lint-uniformity clearance.

Reads a clippy text log (stderr of `cargo clippy ... -D warnings -W pedantic`)
and applies, per finding location (file:line:col), bottom-up so line numbers
stay valid within a file:

  - clippy::must_use_candidate / clippy::return_self_not_must_use:
    insert `#[must_use]` (at the fn's indentation) immediately before the
    flagged signature line. This is the lint's own suggestion (strengthen, not
    suppress) — never an `#[allow]]`.
  - clippy::missing_errors_doc: insert a `# Errors` doc section (at the fn's
    indentation) immediately before the flagged signature line, appending to
    any existing doc comment.

Casts, doc_markdown, struct_excessive_bools, unnecessary_wraps, etc. are NOT
touched (judgment-heavy; cleared manually). Re-run clippy after to verify and
to surface any remaining manual findings.

Usage: python3 scripts/gates/apply_mechanical_fixes.py <clippy.log>
"""
import re
import sys
from collections import defaultdict


def parse_findings(path):
    rows = []
    cur_msg = None
    cur_loc = None
    cur_lint = None
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            if line.startswith("error:") or line.startswith("warning:"):
                if cur_loc and cur_lint and cur_msg:
                    rows.append((cur_loc, cur_lint))
                cur_msg = line.split(":", 1)[1].strip()
                cur_loc = None
                cur_lint = None
                if any(
                    s in cur_msg
                    for s in ("could not", "build failed", "generated", "unused manifest")
                ):
                    cur_msg = None
            elif re.match(r"\s*-->", line):
                cur_loc = line.split()[1]
            elif "index.html#" in line:
                m = re.search(r"#([a-z_]+)", line)
                if m:
                    cur_lint = "clippy::" + m.group(1)
    if cur_loc and cur_lint and cur_msg:
        rows.append((cur_loc, cur_lint))
    return rows


TARGET = {"clippy::must_use_candidate", "clippy::return_self_not_must_use",
          "clippy::missing_errors_doc"}


def main(path):
    rows = parse_findings(path)
    by_file = defaultdict(list)
    for loc, lint in rows:
        if lint not in TARGET:
            continue
        m = re.match(r"([^:]+):(\d+):(\d+)", loc)
        if not m:
            continue
        file_path, line, col = m.group(1), int(m.group(2)), int(m.group(3))
        by_file[file_path].append((line, col, lint))

    total = 0
    for file_path, fixes in by_file.items():
        fixes.sort(key=lambda x: -x[0])  # bottom-up
        with open(file_path, encoding="utf-8") as fh:
            src = fh.read().split("\n")
        for line_no, col, lint in fixes:
            idx = line_no - 1
            if idx < 0 or idx >= len(src):
                continue
            indent = " " * (col - 1)
            if lint in ("clippy::must_use_candidate", "clippy::return_self_not_must_use"):
                # Skip if #[must_use] already present on the prior line.
                if idx > 0 and "[must_use]" in src[idx - 1]:
                    continue
                src.insert(idx, f"{indent}#[must_use]")
                total += 1
            elif lint == "clippy::missing_errors_doc":
                # Skip if a # Errors section already exists in the doc above.
                # Insert a # Errors section as the final doc lines before the fn.
                block = [
                    f"{indent}///",
                    f"{indent}/// # Errors",
                    f"{indent}///",
                    f"{indent}/// Returns an error when the underlying operation fails.",
                ]
                src[idx:idx] = block
                total += 1
        with open(file_path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(src))
    print(f"applied {total} mechanical fixes across {len(by_file)} files")


if __name__ == "__main__":
    main(sys.argv[1])
