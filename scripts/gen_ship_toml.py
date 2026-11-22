#!/usr/bin/env python3
"""Generate ship.toml for the V2-E05 plugin-compat harness delivery.

One [[commit]] block per dirty file (the task's "one commit per file, no
multi-file commits" rule), ordered so dependencies land first. The dirty set
is the modified-tracked + new-untracked files (conformance/plugins/ is a
single `??` entry in `git status` — expanded here to its individual files);
ship.toml + .github/pr-body.md are gitignored and deliberately excluded
(deliverables, not commits).

Run:  python3 scripts/gen_ship_toml.py
"""

from __future__ import annotations

import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BRANCH = "feat/plugin-compat-harness"
PR_TITLE = "feat: plugin compatibility harness (E05)"
BASE = subprocess.check_output(
    ["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True
).strip()

# The ordered (path, message) list — dependencies first. The bulk
# conformance/plugins/ files are enumerated below in their dependency slots.
NAMED: list[tuple[str, str]] = [
    ("conformance/plugins/schema.mjs",
     "feat(conformance): the V2-E05 case schema + loader"),
    ("conformance/plugins/evaluator.mjs",
     "feat(conformance): the plugin-compat assert evaluator"),
    ("conformance/plugins/report.mjs",
     "feat(conformance): the report parser + matrix renderer"),
    ("scripts/plugin-compat.mjs",
     "feat(scripts): the plugin-compat runner + --dry-run mode"),
    (".gitignore",
     "chore(gitignore): un-ignore the recorded plugin-compat dist trees"),
    ("crates/plugin/src/compat.rs",
     "feat(plugin): the load-time unsupported-hook report model"),
    ("crates/plugin/src/lib.rs",
     "feat(plugin): wire the load-time report into PluginRegistry"),
    ("crates/plugin/src/wasm.rs",
     "feat(plugin): WasmPlugin::load_time_report"),
    ("crates/cli/src/docs_gen.rs",
     "feat(cli): the PluginCompatPage docs generator + base() override"),
    ("crates/cli/src/commands/docs.rs",
     "feat(cli): cmd_docs writes under the docs root + joins base"),
    ("crates/cli/tests/docs_tdd.rs",
     "test(cli): the plugin-compat docs-check pins"),
    ("docs/plugin-compat.md",
     "docs(plugin-compat): regenerate + add the verified-plugins matrix"),
    ("crates/plugin/tests/compat_tdd.rs",
     "test(plugin): the V2-E05 load-time report + LoadTimeReport pins"),
    ("crates/adapters/tests/config_refusal_tdd.rs",
     "fix(adapters): the V2-E09 Cloudflare-Live refusal pin"),
    ("docs/errors.toml",
     "fix(errors): register CFG-CHECK/RFN-LOAD/SKW-POOL/SKW-SNAPSHOT/DOC-004"),
    ("docs/reference/errors.md",
     "docs(errors): regenerate the error-code registry page"),
    ("docs/deviations.md",
     "docs(deviations): record the V2-E05 landing"),
    ("docs/spec-v0.2-draft.md",
     "docs(spec): mark the V2-E05 row + section landed"),
    ("conformance/plugins/bench.mjs",
     "test(conformance): the plugin-compat node bench"),
    ("conformance/plugins/results.json",
     "test(conformance): the recorded plugin-compat results"),
    ("scripts/gen_plugin_cases.py",
     "chore(scripts): the plugin-compat cases/fixtures/logs generator"),
    ("scripts/regen_plugin_compat_doc.py",
     "chore(scripts): the faithful plugin-compat.md regenerator"),
    ("scripts/regen_errors_md.py",
     "chore(scripts): the faithful errors.md regenerator"),
    ("scripts/gen_ship_toml.py",
     "chore(scripts): the ship.toml generator"),
    ("scripts/gates/e05_red.log",
     "test(gates): the V2-E05 red transcript"),
    ("scripts/gates/e05_green.log",
     "test(gates): the V2-E05 green transcript"),
    ("scripts/gates/errors_registry_red.log",
     "test(gates): the errors-registry red transcript"),
    ("scripts/gates/errors_registry_green.log",
     "test(gates): the errors-registry green transcript"),
    ("scripts/gates/config_refusal_red.log",
     "test(gates): the config-refusal Live-degradation red transcript"),
    ("scripts/gates/config_refusal_green.log",
     "test(gates): the config-refusal Live-degradation green transcript"),
]


def expand_bulk() -> list[tuple[str, str]]:
    """Enumerate the per-file cases/fixtures/logs/test entries with messages,
    slotted after their dependency (the schema, then the runner)."""
    out: list[tuple[str, str]] = []
    base = ROOT / "conformance/plugins"
    # cases (after schema + runner): one per plugin
    for p in sorted((base / "cases").glob("*.json")):
        name = p.stem
        out.append((
            str(p.relative_to(ROOT)),
            f"test(conformance): the {name} plugin-compat case",
        ))
    # fixtures (after cases)
    for app in sorted(d for d in (base / "fixtures").iterdir() if d.is_dir()):
        for f in sorted(app.rglob("*")):
            if f.is_file():
                rel = f.relative_to(app)
                out.append((
                    str(f.relative_to(ROOT)),
                    f"test(conformance): the {app.name} fixture {rel}",
                ))
    # logs (after fixtures)
    for case in sorted(d for d in (base / "logs").iterdir() if d.is_dir()):
        for f in sorted(case.rglob("*")):
            if f.is_file():
                rel = f.relative_to(case)
                kind = "build log" if rel.name == "build.log" else "recorded dist"
                out.append((
                    str(f.relative_to(ROOT)),
                    f"test(conformance): the {case.name} {kind}",
                ))
    # node tests (after bench/results)
    for p in sorted((base / "test").glob("*.test.mjs")):
        out.append((
            str(p.relative_to(ROOT)),
            f"test(conformance): the {p.stem} node --test gate",
        ))
    return out


def dirty_set() -> set[str]:
    """The dirty file set: modified-tracked + new-untracked (conformance/plugins/
    expanded). Excludes gitignored (ship.toml, .github/pr-body.md, target/)."""
    out = subprocess.check_output(
        ["git", "-C", str(ROOT), "status", "--porcelain"], text=True
    ).splitlines()
    files: set[str] = set()
    for line in out:
        if not line.strip():
            continue
        path = line[3:].strip().strip('"')
        if path.endswith("/"):
            path = path[:-1]
        p = ROOT / path
        if p.is_dir():
            for f in p.rglob("*"):
                if f.is_file():
                    files.add(str(f.relative_to(ROOT)))
        else:
            files.add(path)
    return files


def main() -> None:
    bulk = expand_bulk()
    # Insert the bulk blocks in their dependency slots: cases/fixtures/logs
    # after the runner (index of scripts/plugin-compat.mjs) + before
    # docs_gen.rs; node tests after bench/results.
    runner_idx = next(
        i for i, (p, _) in enumerate(NAMED) if p == "scripts/plugin-compat.mjs"
    )
    docs_gen_idx = next(
        i for i, (p, _) in enumerate(NAMED) if p == "crates/cli/src/docs_gen.rs"
    )
    bench_idx = next(
        i for i, (p, _) in enumerate(NAMED) if p == "conformance/plugins/bench.mjs"
    )

    # split bulk: cases/fixtures/logs (before docs_gen) vs node tests (after bench)
    cases_fixtures_logs = [
        b for b in bulk
        if b[0].startswith("conformance/plugins/cases/")
        or b[0].startswith("conformance/plugins/fixtures/")
        or b[0].startswith("conformance/plugins/logs/")
    ]
    node_tests = [b for b in bulk if b[0].startswith("conformance/plugins/test/")]

    ordered = (
        NAMED[:docs_gen_idx]
        + cases_fixtures_logs
        + NAMED[docs_gen_idx : bench_idx + 1]
        + node_tests
        + NAMED[bench_idx + 1 :]
    )
    # dedupe (results.json/bench.mjs are in NAMED, not bulk)
    seen: set[str] = set()
    final: list[tuple[str, str]] = []
    for p, m in ordered:
        if p in seen:
            continue
        seen.add(p)
        final.append((p, m))

    # union check against the dirty set
    dirty = dirty_set()
    listed = {p for p, _ in final}
    missing = dirty - listed
    extra = listed - dirty
    if missing:
        raise SystemExit(f"dirty but not in ship.toml ({len(missing)}):\n  "
                         + "\n  ".join(sorted(missing)[:20]))
    if extra:
        raise SystemExit(f"in ship.toml but not dirty ({len(extra)}):\n  "
                         + "\n  ".join(sorted(extra)[:20]))

    lines = [
        "[delivery]",
        f"branch = \"{BRANCH}\"",
        f"pr_title = \"{PR_TITLE}\"",
        'pr_body = ".github/pr-body.md"',
        f"base_commit = \"{BASE}\"",
        "",
    ]
    for path, msg in final:
        lines.append("[[commit]]")
        lines.append(f'message = "{msg}"')
        lines.append(f'files = ["{path}"]')
        lines.append("")
    ship = ROOT / "ship.toml"
    ship.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {ship} ({len(final)} commit blocks; base {BASE[:8]})")


if __name__ == "__main__":
    main()
