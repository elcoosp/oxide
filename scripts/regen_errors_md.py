#!/usr/bin/env python3
"""Regenerate docs/reference/errors.md to match docs_gen::ErrorsPage::render().

The CLI binary does not link in this sandbox (ENOSPC on the final `cc` link).
This script reproduces the ErrorsPage generator's deterministic output
byte-for-byte: the BANNER, the header prose, the `## Codes` table (every
non-`spec_only` entry, sorted by code) and the `## Spec-only codes` table.
The source is `docs/errors.toml` (parsed with stdlib `tomllib`); `cell()` is
the generator's pipe-escape. The CI `docs-check` job is the final arbiter.

Run:  python3 scripts/regen_errors_md.py
"""

from __future__ import annotations

import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ERRORS_TOML = ROOT / "docs/errors.toml"
ERRORS_MD = ROOT / "docs/reference/errors.md"

BANNER = (
    "<!-- GENERATED FILE — `oxide docs generate` (spec v0.2 §V2-E11). DO NOT EDIT.\n"
    "Regenerate: cargo run -p oxide-fw-cli -- docs generate\n"
    "Drift gate: crates/cli/tests/docs_tdd.rs -->\n\n"
)

HEADER = (
    "Every stable diagnostic code the tool can emit (Appendix V2.K). Generated "
    "from `docs/errors.toml` — the registry file, not the call sites; "
    "`crates/cli/tests/errors_registry_tdd.rs` is the gate that fails when a "
    "production call site raises a code this page does not carry.\n"
)

TABLE_HEAD = "| code | message | fix |\n| --- | --- | --- |\n"


def cell(text: str) -> str:
    """The generator's `cell()`: escape pipes for a markdown table cell."""
    return text.replace("|", "\\|")


def main() -> None:
    with ERRORS_TOML.open("rb") as f:
        registry = tomllib.load(f)

    entries = []
    for code, fields in registry.items():
        if not isinstance(fields, dict):
            continue
        entries.append(
            {
                "code": code,
                "message": fields.get("message", ""),
                "fix": fields.get("fix"),
                "spec_only": bool(fields.get("spec_only", False)),
            }
        )
    entries.sort(key=lambda e: e["code"])

    out = BANNER
    out += "# Error code registry\n\n"
    out += HEADER + "\n"
    out += "## Codes\n\n"
    out += TABLE_HEAD
    for e in entries:
        if e["spec_only"]:
            continue
        fix = e["fix"] if e["fix"] is not None else "—"
        out += f"| `{e['code']}` | {cell(e['message'])} | {cell(fix)} |\n"

    spec = [e for e in entries if e["spec_only"]]
    if spec:
        out += "\n## Spec-only codes\n\n"
        out += (
            "Defined by the spec for work packages that have not landed; no "
            "call site raises them yet.\n"
        )
        out += "\n" + TABLE_HEAD
        for e in spec:
            fix = e["fix"] if e["fix"] is not None else "—"
            out += f"| `{e['code']}` | {cell(e['message'])} | {cell(fix)} |\n"

    ERRORS_MD.write_text(out, encoding="utf-8")
    print(f"wrote {ERRORS_MD} ({len(out)} bytes, {len(entries)} codes)")
    # sanity: the five new codes landed in the Codes table.
    for code in ("CFG-CHECK", "DOC-004", "RFN-LOAD", "SKW-POOL", "SKW-SNAPSHOT"):
        assert f"| `{code}` |" in out, f"{code} missing from errors.md"
    print("ok: CFG-CHECK, DOC-004, RFN-LOAD, SKW-POOL, SKW-SNAPSHOT present")


if __name__ == "__main__":
    main()
