//! TDD pins for the §V2-F01 island boot planner — the pure compiler-side
//! emission the build's `__boot__` entry and the tooling's
//! `.oxide/islands.json` come from (`oxide_fw_compiler::islands`).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - A single-framework plan emits the LEGACY boot entry shape —
//!   byte-preserving contracts (the static adapter import, the
//!   `modules` map, `installBootRuntime()`,
//!   `window.__oxideRegisterAdapter(clientAdapter, ...)`, the §8.4
//!   loading map, the manifest-fetch tail). Existing single-framework
//!   apps and the conformance suite must not see a byte of drift.
//! - A mixed plan emits the registry shape: one lazy adapter import
//!   per framework present (deduplicated), an `islands` map carrying
//!   `{ fw, load }` per entry with the absolute import path, and
//!   `installBootRuntime({ adapters, islands })` — the §8.4 loading
//!   map, the page adapter's static import and the `bootWithRouter`
//!   tail are unchanged (the router contract keeps its sequencing).
//! - `islands_json()` renders the tooling payload (`{ Name: { chunk,
//!   framework } }`, sorted, compact) the `oxide why`/F17 overlays read.
//! - The PWA registration line rides both shapes (`SPEC-21`).

use std::collections::BTreeMap;
use std::path::Path;

use oxide_fw_compiler::islands::{ResolvedIsland, plan_island_boot};

fn islands(specs: &[(&str, &str, &str)]) -> BTreeMap<String, ResolvedIsland> {
    specs
        .iter()
        .map(|(name, src, fw)| {
            (
                (*name).to_string(),
                ResolvedIsland {
                    src: (*src).to_string(),
                    framework: (*fw).to_string(),
                },
            )
        })
        .collect()
}

// ------------------------------------------------------ single-framework

#[test]
fn single_framework_plan_is_mixed_false() {
    let plan = plan_island_boot(
        "react",
        &islands(&[("Counter", "src/islands/counter.tsx", "react")]),
    );
    assert!(!plan.mixed);
}

#[test]
fn single_framework_boot_entry_keeps_the_legacy_shape() {
    let plan = plan_island_boot(
        "react",
        &islands(&[("Counter", "src/islands/counter.tsx", "react")]),
    );
    let out = plan.boot_entry_source(Path::new("/proj"), &BTreeMap::new(), false);
    assert!(
        out.contains("import { clientAdapter } from '@oxide-fw/react';"),
        "{out}"
    );
    assert!(out.contains(
        "import { bootWithRouter, installBootRuntime } from '@oxide-fw/core/client/boot';"
    ));
    // The static island import + modules map (the legacy hydration path).
    assert!(out.contains("import __island_Counter from \"/proj/src/islands/counter.tsx\";"));
    assert!(out.contains("const modules = {"));
    assert!(out.contains("  \"Counter\": __island_Counter,"));
    assert!(out.contains("installBootRuntime();"));
    assert!(out.contains("window.__oxideRegisterAdapter(clientAdapter"));
    // The §8.4 tail: manifest fetch + bootWithRouter + vitals.
    assert!(out.contains("bootWithRouter(clientAdapter, { loading: __loading_map, manifest })"));
    assert!(out.contains("reportWebVitals();"));
    // No registry in the legacy shape.
    assert!(!out.contains("const adapters"));
    assert!(!out.contains("installBootRuntime({"));
}

#[test]
fn the_pwa_registration_line_rides_both_shapes() {
    let single = plan_island_boot("react", &islands(&[("Counter", "src/i.tsx", "react")]));
    let mixed = plan_island_boot(
        "react",
        &islands(&[
            ("Counter", "src/i.tsx", "react"),
            ("Price", "src/p.vue", "vue"),
        ]),
    );
    for out in [
        single.boot_entry_source(Path::new("/p"), &BTreeMap::new(), true),
        mixed.boot_entry_source(Path::new("/p"), &BTreeMap::new(), true),
    ] {
        assert!(
            out.contains("__oxidePwa.src = '/pwa-register.js';"),
            "pwa registration missing"
        );
    }
}

// ---------------------------------------------------------- mixed plan

#[test]
fn mixed_plan_emits_the_registry_shape() {
    let plan = plan_island_boot(
        "react",
        &islands(&[
            ("Counter", "src/islands/counter.tsx", "react"),
            ("Price", "src/islands/Price.vue", "vue"),
            ("Ticker", "src/islands/ticker.tsx", "solid"),
        ]),
    );
    assert!(plan.mixed);
    let out = plan.boot_entry_source(Path::new("/proj"), &BTreeMap::new(), false);
    // One lazy adapter per framework (the page's own included), sorted.
    assert!(out.contains("const adapters = {"), "{out}");
    assert!(out.contains("  react: () => import('@oxide-fw/react'),"));
    assert!(out.contains("  solid: () => import('@oxide-fw/solid'),"));
    assert!(out.contains("  vue: () => import('@oxide-fw/vue'),"));
    // The islands map carries fw + load per entry (absolute paths).
    assert!(out.contains("const islands = {"));
    assert!(out.contains(
        "  \"Counter\": { fw: 'react', load: () => import(\"/proj/src/islands/counter.tsx\") },"
    ));
    assert!(out.contains(
        "  \"Price\": { fw: 'vue', load: () => import(\"/proj/src/islands/Price.vue\") },"
    ));
    assert!(out.contains("installBootRuntime({ adapters, islands });"));
    // The router contract survives: the page adapter stays static and
    // the bootWithRouter tail keeps its sequencing.
    assert!(out.contains("import { clientAdapter } from '@oxide-fw/react';"));
    assert!(out.contains("bootWithRouter(clientAdapter, { loading: __loading_map, manifest })"));
    // The legacy modules map is gone (nothing static-imports an island).
    assert!(!out.contains("const modules = {"));
    assert!(!out.contains("window.__oxideRegisterAdapter"));
}

#[test]
fn mixed_plan_deduplicates_the_adapter_map() {
    let plan = plan_island_boot(
        "react",
        &islands(&[
            ("A", "src/a.tsx", "react"),
            ("B", "src/b.tsx", "react"),
            ("C", "src/c.vue", "vue"),
        ]),
    );
    let out = plan.boot_entry_source(Path::new("/p"), &BTreeMap::new(), false);
    assert_eq!(out.matches("() => import('@oxide-fw/react')").count(), 1);
    assert_eq!(out.matches("() => import('@oxide-fw/vue')").count(), 1);
}

#[test]
fn the_loading_map_contract_survives_the_mixed_shape() {
    // §8.4: bracket tokens preserved, root key `/`, longest key wins.
    let plan = plan_island_boot("react", &islands(&[("A", "src/a.tsx", "vue")]));
    let mut loading = BTreeMap::new();
    loading.insert(
        "routes/blog/[slug]/loading".to_string(),
        std::path::PathBuf::from("src/routes/blog/[slug]/loading.tsx"),
    );
    loading.insert(
        "routes/loading".to_string(),
        std::path::PathBuf::from("src/routes/loading.tsx"),
    );
    let out = plan.boot_entry_source(Path::new("/proj"), &loading, false);
    assert!(out.contains("\"/blog/[slug]\""), "{out}");
    assert!(out.contains("\"/\""));
    assert!(out.contains("bootWithRouter(clientAdapter, { loading: __loading_map"));
}

// ------------------------------------------------------------ islands.json

#[test]
fn islands_json_is_the_sorted_tooling_payload() {
    let plan = plan_island_boot(
        "react",
        &islands(&[
            ("Counter", "src/islands/counter.tsx", "react"),
            ("Price", "src/islands/Price.vue", "vue"),
        ]),
    );
    let json = plan.islands_json();
    assert_eq!(
        json,
        r#"{"Counter":{"chunk":"src/islands/counter.tsx","framework":"react"},"Price":{"chunk":"src/islands/Price.vue","framework":"vue"}}"#
    );
}

// ------------------------------------------------------------ vendor groups
// §V2-F01 item 2: the rolldown `advancedChunks` per-framework vendor split.
// The groups come from the plan's adapter set; a SINGLE-framework plan stays
// empty so existing builds keep their byte-identical chunk shape (the
// F01 contract: existing apps and the conformance suite see no drift), and a
// MIXED plan emits one group per framework — `react` additionally splits
// `react-dom` (the spec names both as their own vendor chunks) — so each
// vendor loads once, on the first island of that framework.

#[test]
fn a_mixed_plan_emits_the_per_framework_vendor_groups() {
    let plan = plan_island_boot(
        "react",
        &islands(&[
            ("Counter", "src/islands/counter.tsx", "react"),
            ("Price", "src/islands/Price.vue", "vue"),
        ]),
    );
    let groups = plan.vendor_groups();
    let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["vendor-react", "vendor-react-dom", "vendor-vue"]
    );
    // The tests are JS regexes matching the module id inside node_modules —
    // both slash layouts (the pnpm store's inner `node_modules/react/` and a
    // plain npm install) and never a bare prefix like `react-dom` matching
    // the `react` group.
    for g in &groups {
        assert!(g.test.contains("node_modules"), "{}", g.test);
        assert!(
            g.test.contains("[\\\\/]"),
            "the test must match both slashes: {}",
            g.test
        );
    }
    let react = &groups[0];
    assert_eq!(react.test, r"[\\/]node_modules[\\/]react[\\/]");
    assert_eq!(groups[1].test, r"[\\/]node_modules[\\/]react-dom[\\/]");
    // The vue group covers the `@vue/*` internals the runtime pulls in.
    assert!(groups[2].test.contains("vue"));
}

#[test]
fn a_single_framework_plan_emits_no_vendor_groups() {
    let plan = plan_island_boot("react", &islands(&[("A", "src/a.tsx", "react")]));
    assert_eq!(plan.vendor_groups().len(), 0);
}

#[test]
fn the_solid_and_svelte_groups_carry_their_own_tests() {
    let plan = plan_island_boot(
        "solid",
        &islands(&[("A", "src/a.tsx", "solid"), ("B", "src/b.svelte", "svelte")]),
    );
    let groups = plan.vendor_groups();
    let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, vec!["vendor-solid-js", "vendor-svelte"]);
}

#[test]
fn an_unknown_framework_contributes_no_group() {
    // The inference's empty-string case (a bare pragma line) must not
    // produce a nonsense vendor chunk.
    let plan = plan_island_boot(
        "react",
        &islands(&[("A", "src/a.tsx", "react"), ("B", "src/b.tsx", "")]),
    );
    let groups = plan.vendor_groups();
    let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, vec!["vendor-react", "vendor-react-dom"]);
}

// ------------------------------------------------- legacy byte identity
// §V2-F01 build-path wiring: the CLI's `generate_island_boot_entry`
// delegates to the planner, so the planner's SINGLE-framework emission
// must be BYTE-identical to the legacy generator's bytes (the F01
// contract: existing single-framework builds see not one byte of drift).
// The fixtures are the legacy generator's exact emission for the same
// inputs (boot_entry.rs at the wiring revision, mechanically transcribed
// by scripts/legacy_boot_fixture.py).

/// The legacy generator's bytes: one island, no loading map, no PWA.
/// One literal per line in a `concat!` — a `\` string continuation would
/// strip the next line's indentation, which is part of the bytes.
const LEGACY_BYTES_BARE: &str = concat!(
    "// Generated by oxide-fw-cli -- island boot entry. Do not edit.\n",
    "// Static imports resolve before any statement runs, so `boot()`\n",
    "// can see both the adapter and the boot runtime's registration hook.\n",
    "import { clientAdapter } from '@oxide-fw/react';\n",
    "import { bootWithRouter, installBootRuntime } from '@oxide-fw/core/client/boot';\n",
    "import __island_Counter from \"/proj/src/islands/counter.tsx\";\n",
    "\n",
    "const modules = {\n",
    "  \"Counter\": __island_Counter,\n",
    "};\n",
    "\n",
    "\n",
    "const __loading_map = {};\n",
    "\n",
    "installBootRuntime();\n",
    "window.__oxideRegisterAdapter(clientAdapter, async (chunk) => {\n",
    "  return modules[chunk] ?? (await import(chunk));\n",
    "});\n",
    "fetch('/client/client.manifest.json')\n",
    "  .then((r) => (r.ok ? r.json() : undefined))\n",
    "  .then((manifest) => bootWithRouter(clientAdapter, { loading: __loading_map, manifest }))\n",
    "  .catch(() => bootWithRouter(clientAdapter, { loading: __loading_map }));\n",
    "import { reportWebVitals } from '@oxide-fw/core/client';\n",
    "reportWebVitals();\n",
);

/// The legacy generator's bytes with the §8.4 loading map and the §16.6
/// PWA registration (every emission branch exercised).
const LEGACY_BYTES_FULL: &str = concat!(
    "// Generated by oxide-fw-cli -- island boot entry. Do not edit.\n",
    "// Static imports resolve before any statement runs, so `boot()`\n",
    "// can see both the adapter and the boot runtime's registration hook.\n",
    "import { clientAdapter } from '@oxide-fw/react';\n",
    "import { bootWithRouter, installBootRuntime } from '@oxide-fw/core/client/boot';\n",
    "import __island_Counter from \"/proj/src/islands/counter.tsx\";\n",
    "\n",
    "const modules = {\n",
    "  \"Counter\": __island_Counter,\n",
    "};\n",
    "\n",
    "import __loading_routes_blog__slug__loading from \"/proj/src/routes/blog/[slug]/loading.tsx\";\n",
    "\n",
    "const __loading_map = {\n",
    "  \"/blog/[slug]\": __loading_routes_blog__slug__loading,\n",
    "};\n",
    "\n",
    "installBootRuntime();\n",
    "window.__oxideRegisterAdapter(clientAdapter, async (chunk) => {\n",
    "  return modules[chunk] ?? (await import(chunk));\n",
    "});\n",
    "fetch('/client/client.manifest.json')\n",
    "  .then((r) => (r.ok ? r.json() : undefined))\n",
    "  .then((manifest) => bootWithRouter(clientAdapter, { loading: __loading_map, manifest }))\n",
    "  .catch(() => bootWithRouter(clientAdapter, { loading: __loading_map }));\n",
    "import { reportWebVitals } from '@oxide-fw/core/client';\n",
    "reportWebVitals();\n",
    "if (typeof navigator !== 'undefined' && 'serviceWorker' in navigator && !globalThis.__oxidePwaRegistered) {\n",
    "  globalThis.__oxidePwaRegistered = true;\n",
    "  var __oxidePwa = document.createElement('script');\n",
    "  __oxidePwa.src = '/pwa-register.js';\n",
    "  __oxidePwa.async = true;\n",
    "  document.head.appendChild(__oxidePwa);\n",
    "}\n",
);

#[test]
fn single_framework_boot_entry_is_byte_identical_to_the_legacy_emission() {
    let plan = plan_island_boot(
        "react",
        &islands(&[("Counter", "src/islands/counter.tsx", "react")]),
    );
    assert_eq!(
        plan.boot_entry_source(Path::new("/proj"), &BTreeMap::new(), false),
        LEGACY_BYTES_BARE
    );
}

#[test]
fn the_legacy_emission_with_loading_and_pwa_is_byte_identical() {
    let plan = plan_island_boot(
        "react",
        &islands(&[("Counter", "src/islands/counter.tsx", "react")]),
    );
    let mut loading = BTreeMap::new();
    loading.insert(
        "routes/blog/[slug]/loading".to_string(),
        std::path::PathBuf::from("src/routes/blog/[slug]/loading.tsx"),
    );
    assert_eq!(
        plan.boot_entry_source(Path::new("/proj"), &loading, true),
        LEGACY_BYTES_FULL
    );
}

#[test]
fn the_single_framework_shape_runs_install_boot_runtime_after_the_loading_map() {
    // The legacy order (§8.4 sequencing): the loading map resolves before
    // `installBootRuntime()` runs; the mixed registry keeps its own order.
    let plan = plan_island_boot(
        "react",
        &islands(&[("Counter", "src/islands/counter.tsx", "react")]),
    );
    let out = plan.boot_entry_source(Path::new("/proj"), &BTreeMap::new(), false);
    let loading_map = out
        .find("const __loading_map")
        .expect("loading map present");
    let install = out.find("installBootRuntime();").expect("install call");
    assert!(loading_map < install, "{out}");
}
