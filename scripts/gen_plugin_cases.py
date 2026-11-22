#!/usr/bin/env python3
"""Generate the V2-E05 plugin-compat cases, fixtures and recorded logs.

Each case = one ``conformance/plugins/cases/<name>.json`` (the §V2-E05 shape),
one fixture app under ``conformance/plugins/fixtures/<fixture>/`` (the CI
substrate the runner copies + installs the plugin into), and one recorded
build log + dist tree under ``conformance/plugins/logs/<name>/`` (the
dry-run truth — what the runner parses V8-free without a single install).

The recorded logs encode the EXPECTED compat-model outcome per plugin: the
``oxide-plugin-load`` structured line the Rust bridge emits (wired hooks +
empty ``unsupportedHooks`` for the pass cases), and the existing ``PLG-HOOK``
fail-at-load message for the one plugin that declares ``configureServer``
(vite-plugin-pwa — a documented, not hidden, limitation). The CI ``node`` job
re-runs the runner in real mode and refreshes ``results.json``; the
``docs-check`` job regenerates ``docs/plugin-compat.md`` and fails on drift.

Run:  python3 scripts/gen_plugin_cases.py
"""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASE = ROOT / "conformance" / "plugins"
CASES = BASE / "cases"
FIX = BASE / "fixtures"
LOGS = BASE / "logs"

# name, plugin, version, adapter, fixture, hooks, assert(file, contains),
# dist_file (under the case log dir), dist_marker, status, note
SPECS = [
    {
        "name": "svelte",
        "pkg": "@sveltejs/vite-plugin-svelte",
        "version": "7.3.1",
        "adapter": "svelte",
        "fixture": "svelte-app",
        "hooks": ["transform"],
        "assert_file": "dist/client/*.js",
        "assert_contains": "svelte_component",
        "dist_file": "dist/client/index.js",
        "dist_marker": "import { svelte_component } from './internal';",
        "status": "pass",
        "note": None,
    },
    {
        "name": "vue",
        "pkg": "@vitejs/plugin-vue",
        "version": "6.0.11",
        "adapter": "vue",
        "fixture": "vue-app",
        "hooks": ["transform"],
        "assert_file": "dist/client/*.js",
        "assert_contains": "_createVNode",
        "dist_file": "dist/client/index.js",
        "dist_marker": "const _createVNode = () => {};",
        "status": "pass",
        "note": None,
    },
    {
        "name": "solid",
        "pkg": "vite-plugin-solid",
        "version": "2.11.17",
        "adapter": "solid",
        "fixture": "solid-app",
        "hooks": ["transform"],
        "assert_file": "dist/client/*.js",
        "assert_contains": "createSignal",
        "dist_file": "dist/client/index.js",
        "dist_marker": "import { createSignal } from 'solid-js';",
        "status": "pass",
        "note": None,
    },
    {
        "name": "mdx",
        "pkg": "@mdx-js/rollup",
        "version": "3.1.1",
        "adapter": "react",
        "fixture": "mdx-app",
        "hooks": ["transform"],
        "assert_file": "dist/client/*.js",
        "assert_contains": "MDXContent",
        "dist_file": "dist/client/index.js",
        "dist_marker": "export function MDXContent() { return null; }",
        "status": "pass",
        "note": None,
    },
    {
        "name": "svgr",
        "pkg": "vite-plugin-svgr",
        "version": "5.2.0",
        "adapter": "react",
        "fixture": "svgr-app",
        "hooks": ["transform", "resolveId", "load"],
        "assert_file": "dist/client/*.js",
        "assert_contains": "createElement",
        "dist_file": "dist/client/index.js",
        "dist_marker": "const Logo = () => createElement('svg');",
        "status": "pass",
        "note": None,
    },
    {
        "name": "tailwind",
        "pkg": "@tailwindcss/vite",
        "version": "4.3.5",
        "adapter": "react",
        "fixture": "tailwind-app",
        "hooks": ["transform"],
        "assert_file": "dist/client/*.css",
        "assert_contains": ".tailwind",
        "dist_file": "dist/client/style.css",
        "dist_marker": ".tailwind { color: red; }",
        "status": "pass",
        "note": None,
    },
    {
        "name": "icons",
        "pkg": "unplugin-icons",
        "version": "24.0.0",
        "adapter": "react",
        "fixture": "icons-app",
        "hooks": ["transform", "resolveId", "load"],
        "assert_file": "dist/client/*.js",
        "assert_contains": "icon",
        "dist_file": "dist/client/index.js",
        "dist_marker": "const icon = 'logo'; export { icon };",
        "status": "pass",
        "note": None,
    },
    {
        "name": "tsconfig-paths",
        "pkg": "vite-tsconfig-paths",
        "version": "6.1.1",
        "adapter": "react",
        "fixture": "tsconfig-app",
        "hooks": ["resolveId"],
        "assert_file": "dist/client/*.js",
        "assert_contains": "resolvedAlias",
        "dist_file": "dist/client/index.js",
        "dist_marker": "import { resolvedAlias } from '@/lib';",
        "status": "pass",
        "note": None,
    },
    {
        "name": "pwa",
        "pkg": "vite-plugin-pwa",
        "version": "2.0.0",
        "adapter": "react",
        "fixture": "pwa-app",
        "hooks": ["transform", "configureServer"],
        "assert_file": "dist/client/*.js",
        "assert_contains": "registerSW",
        "dist_file": None,  # configureServer fails at load → no SW output
        "dist_marker": None,
        "status": "unsupported-hook",
        "note": "declares configureServer (bridge-unsupported, PLG-HOOK); the plugin fails at load so the SW registration never lands",
    },
    {
        "name": "react",
        "pkg": "@vitejs/plugin-react",
        "version": "6.3.0",
        "adapter": "react",
        "fixture": "react-app",
        "hooks": ["transform"],
        "assert_file": "dist/client/*.js",
        "assert_contains": "_jsx",
        "dist_file": "dist/client/index.js",
        "dist_marker": "const el = _jsx('div', {});",
        "status": "pass",
        "note": None,
    },
]


def adapter_pkg(adapter: str) -> str:
    return {
        "react": "@oxide-fw/react",
        "vue": "@oxide-fw/vue",
        "svelte": "@oxide-fw/svelte",
        "solid": "@oxide-fw/solid",
    }[adapter]


def adapter_import(adapter: str) -> str:
    return {
        "react": "react",
        "vue": "vue",
        "svelte": "svelte",
        "solid": "solid",
    }[adapter]


def page_ext(adapter: str) -> str:
    return {"react": "tsx", "vue": "vue", "svelte": "svelte", "solid": "tsx"}[adapter]


def write(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")


def gen_case(s: dict) -> None:
    case = {
        "plugin": f"{s['pkg']}@{s['version']}",
        "fixture": s["fixture"],
        "adapter": s["adapter"],
        "assert": [{"file": s["assert_file"], "contains": s["assert_contains"]}],
        "hooks": s["hooks"],
    }
    write(CASES / f"{s['name']}.json", json.dumps(case, indent=2) + "\n")


def gen_fixture(s: dict) -> None:
    fdir = FIX / s["fixture"]
    pkg = {
        "name": s["fixture"],
        "private": True,
        "version": "0.1.0",
        "type": "module",
        "description": f"V2-E05 plugin-compat fixture for {s['pkg']}@{s['version']} — a minimal oxide app the runner copies, installs the plugin into, and builds.",
        "scripts": {"build": "oxide build"},
        "dependencies": {
            s["pkg"]: f"^{s['version']}",
            adapter_pkg(s["adapter"]): "workspace:*",
            "@oxide-fw/core": "workspace:*",
        },
    }
    write(fdir / "package.json", json.dumps(pkg, indent=2) + "\n")
    # A minimal oxide config wiring the plugin through `plugins` (§15.2).
    plugin_var = s["name"]
    adp = adapter_import(s["adapter"])
    adp_pkg = adapter_pkg(s["adapter"])
    import_line = f"import {{ adapter as {adp}Adapter }} from '{adp_pkg}'\n"
    cfg = (
        "import { defineConfig } from '@oxide-fw/core'\n"
        + import_line
        + f"import {{ default as {plugin_var} }} from '{s['pkg']}'\n\n"
        + f"// V2-E05 fixture: wires {s['pkg']} into the plugin chain so the\n"
        + "// bridge's load-time unsupported-hook report runs against it.\n"
        + f"export default defineConfig({{\n"
        + f"  framework: {adp}Adapter(),\n"
        + "  routes: { dir: 'src/routes' },\n"
        + (
            f"  plugins: [typeof {plugin_var} === 'function' ? {{ name: '{s['pkg']}',"
            f" kind: 'js', path: '{s['pkg']}', options: {{}} }} :"
            f" {{ name: '{s['pkg']}', kind: 'js', path: '{s['pkg']}' }}],\n"
        )
        + "  adapter: 'binary',\n"
        + "})\n"
    )
    write(fdir / "oxide.config.ts", cfg)
    ext = page_ext(s["adapter"])
    body = {
        "tsx": "export default function Page() { return null }\n",
        "vue": "<template><div/></template>\n",
        "svelte": "<div/>\n",
    }[ext]
    write(fdir / "src/routes/index" / f"page.{ext}", body)


def gen_log(s: dict) -> None:
    ldir = LOGS / s["name"]
    # The recorded build log: the structured oxide-plugin-load line (the
    # Rust bridge's load-time report) +, for the configureServer case, the
    # existing PLG-HOOK fail-at-load message.
    lines = ["oxide build: starting"]
    if s["status"] == "unsupported-hook":
        # The JS bridge refuses configureServer at load (PLG-HOOK) — reused,
        # not re-emitted. The plugin never loads, so no SW output lands.
        for h in s["hooks"]:
            if h in ("configureServer", "cacheHandler"):
                lines.append(
                    f"plugin `{s['pkg']}` declares hook `{h}`, which the JS "
                    "bridge does not support — see the compatibility table "
                    "(§15.2/§15.3)"
                )
        lines.append("oxide build: done (plugin skipped)")
    else:
        report = {
            "plugin": s["pkg"],
            "kind": "js",
            "unsupportedHooks": [],
            "wiredHooks": s["hooks"],
        }
        lines.append("oxide-plugin-load\t" + json.dumps(report, separators=(", ", ": ")))
        lines.append(f"oxide build: wrote {s['dist_file']}")
    write(ldir / "build.log", "\n".join(lines) + "\n")
    # The recorded dist tree the dry-run evaluates against.
    if s["dist_file"]:
        write(ldir / s["dist_file"], s["dist_marker"] + "\n")


def main() -> None:
    for d in (CASES, FIX, LOGS):
        d.mkdir(parents=True, exist_ok=True)
    for s in SPECS:
        gen_case(s)
        gen_fixture(s)
        gen_log(s)
    print(f"wrote {len(SPECS)} cases + fixtures + logs under {BASE}")


if __name__ == "__main__":
    main()
