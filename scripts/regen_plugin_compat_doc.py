#!/usr/bin/env python3
"""Regenerate docs/plugin-compat.md to match docs_gen::PluginCompatPage::render().

The CLI binary (which `oxide docs generate` runs) does not link in this sandbox
(ENOSPC on the final `cc` link — the dep tree + binary exceed the 10 GB disk).
This script reproduces the generator's deterministic output byte-for-byte by
reading the prose consts + the verified-section strings straight out of
`crates/cli/src/docs_gen.rs`, the kind×hook table out of the existing
`docs/plugin-compat.md` (which is `compat_markdown_table()` byte-identical —
pinned by `compat_tdd`), and the matrix out of `conformance/plugins/results.json`.
The CI `docs-check` job (`cargo run -p oxide-fw-cli -- docs --root . generate --check`)
is the final arbiter; this script is the sandbox faithful-copy.

Run:  python3 scripts/regen_plugin_compat_doc.py
"""

from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS_GEN = ROOT / "crates/cli/src/docs_gen.rs"
DOC = ROOT / "docs/plugin-compat.md"
RESULTS = ROOT / "conformance/plugins/results.json"


def unescape_rust_string(raw: str) -> str:
    """Unescape a Rust `&str` literal body (between the opening `"` and the
    closing `"` that ends the literal). Handles `\\"`, `\\\\`, `\\n` and the
    `\\` + newline line continuation (which drops the newline AND the leading
    whitespace of the next line)."""
    out: list[str] = []
    i = 0
    n = len(raw)
    while i < n:
        ch = raw[i]
        if ch == "\\" and i + 1 < n:
            nxt = raw[i + 1]
            if nxt == "\n":
                # line continuation: skip \, newline, and leading whitespace
                i += 2
                while i < n and raw[i] in " \t":
                    i += 1
                continue
            if nxt == '"':
                out.append('"')
                i += 2
                continue
            if nxt == "\\":
                out.append("\\")
                i += 2
                continue
            if nxt == "n":
                out.append("\n")
                i += 2
                continue
            if nxt == "t":
                out.append("\t")
                i += 2
                continue
            # unknown escape — drop the backslash, keep the char
            out.append(nxt)
            i += 2
            continue
        out.append(ch)
        i += 1
    return "".join(out)


def extract_const(name: str) -> str:
    """Extract `const NAME: &str = "...";` value (the body, unescaped)."""
    src = DOCS_GEN.read_text(encoding="utf-8")
    # Find the opening quote after the const decl.
    m = re.search(r"const " + re.escape(name) + r": &str = \"", src)
    if not m:
        raise SystemExit(f"const {name} not found in docs_gen.rs")
    start = m.end()
    # Scan for the closing unescaped `"` (followed by `;`).
    i = start
    while i < len(src):
        ch = src[i]
        if ch == "\\":
            i += 2  # skip the escaped char
            continue
        if ch == '"':
            # closing quote
            body = src[start:i]
            return unescape_rust_string(body)
        i += 1
    raise SystemExit(f"const {name} not terminated in docs_gen.rs")


def extract_push_str_after_position(start: int) -> tuple[str, int]:
    """Find the next `push_str(` after `start` in docs_gen.rs and return its
    string-literal body (unescaped) plus the position just after the literal."""
    src = DOCS_GEN.read_text(encoding="utf-8")
    ps = src.find("push_str(", start)
    if ps < 0:
        raise SystemExit("no push_str after the table push_str")
    q = src.find('"', ps)
    i = q + 1
    while i < len(src):
        if src[i] == "\\":
            i += 2
            continue
        if src[i] == '"':
            return unescape_rust_string(src[q + 1 : i]), i + 1
        i += 1
    raise SystemExit("push_str string not terminated")


def main() -> None:
    pre = extract_const("PLUGIN_COMPAT_PRE")
    post = extract_const("PLUGIN_COMPAT_POST")

    # The kind×hook table: compat_markdown_table() output, already embedded
    # byte-identical in the existing docs/plugin-compat.md (pinned by
    # compat_tdd::plugin_compat_doc_embeds_the_generated_table).
    old = DOC.read_text(encoding="utf-8")
    table_match = re.search(r"## Hooks\n\n(\| Hook \|.*?\| `cacheHandler` \| [^\n]*\n)", old, re.DOTALL)
    if not table_match:
        raise SystemExit("could not extract the kind×hook table from the existing doc")
    table = table_match.group(1)

    results = json.loads(RESULTS.read_text(encoding="utf-8"))
    matrix = results["matrix"]

    src = DOCS_GEN.read_text(encoding="utf-8")
    table_push = src.find("out.push_str(&compat_markdown_table())")
    if table_push < 0:
        raise SystemExit("compat_markdown_table() push_str not found")
    verified_heading, pos = extract_push_str_after_position(table_push)
    verified_intro, _ = extract_push_str_after_position(pos)

    page = pre + table + verified_heading + verified_intro + matrix + "\n\n" + post
    DOC.write_text(page, encoding="utf-8")
    print(f"wrote {DOC} ({len(page)} bytes)")
    assert compat_table_fragment() in page, "the kind×hook table is not embedded"
    assert "## Verified third-party plugins (§V2-E05)" in page
    print("ok: kind×hook table embedded + verified-plugins section present")


def compat_table_fragment() -> str:
    # the first row — a cheap presence check (the full table check is the
    # compat_tdd pin's job, run via `cargo test -p oxide-fw-plugin`).
    return "| Hook | § | `js` | `wasm` | `native` | Notes |"


if __name__ == "__main__":
    main()
