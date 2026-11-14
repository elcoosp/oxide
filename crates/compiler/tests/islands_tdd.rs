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
