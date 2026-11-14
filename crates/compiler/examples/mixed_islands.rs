//! The §V2-F01 mixed-framework boot planner walkthrough: a React page
//! with one island per framework — the planner emits the registry
//! boot entry (one lazy adapter per framework) and the
//! `.oxide/islands.json` tooling payload, while a single-framework
//! registry keeps the legacy static-import shape byte-for-byte.
//!
//! ```text
//! cargo run -p oxide-fw-compiler --example mixed_islands
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use oxide_fw_compiler::islands::{ResolvedIsland, plan_island_boot};

fn main() {
    // The spec's example registry: a React page, one island per
    // framework, resolved through `island_framework` at the build.
    let mut islands = BTreeMap::new();
    for (name, src, fw) in [
        ("Counter", "src/islands/counter.tsx", "react"),
        ("Price", "src/islands/Price.vue", "vue"),
        ("Ticker", "src/islands/ticker.tsx", "solid"),
        ("Badge", "src/islands/Badge.svelte", "svelte"),
    ] {
        islands.insert(
            name.to_string(),
            ResolvedIsland {
                src: src.to_string(),
                framework: fw.to_string(),
            },
        );
    }
    let mixed = plan_island_boot("react", &islands);
    println!("mixed = {}\n", mixed.mixed);
    println!(
        "{}",
        mixed.boot_entry_source(
            Path::new("/proj"),
            &std::collections::BTreeMap::default(),
            false
        )
    );
    println!("// .oxide/islands.json:");
    println!("{}", mixed.islands_json());

    // The single-framework plan keeps the legacy shape.
    let mut single = BTreeMap::new();
    single.insert(
        "Counter".to_string(),
        ResolvedIsland {
            src: "src/islands/counter.tsx".into(),
            framework: "react".into(),
        },
    );
    let plan = plan_island_boot("react", &single);
    println!("\nmixed = {}", plan.mixed);
    println!(
        "legacy shape: {}",
        plan.boot_entry_source(
            Path::new("/proj"),
            &std::collections::BTreeMap::default(),
            false
        )
        .contains("const modules = {")
    );
}
