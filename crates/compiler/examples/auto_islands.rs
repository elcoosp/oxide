//! The §V2-F02 auto-islands inference walkthrough: a React page, an
//! interactive counter island, a Vue SFC with a template `@event` and a
//! pure badge — the scanner classifies each file, the fixpoint promotes
//! the importer of an interactive component, and the hydration decision
//! shows which candidates hydrate (explicit registration always wins,
//! pure classification is the only skip).
//!
//! ```text
//! cargo run -p oxide-fw-compiler --example auto_islands
//! ```

use std::collections::BTreeMap;

use oxide_fw_compiler::framework::Framework;
use oxide_fw_compiler::interactivity::{IslandManifest, hydrates};

fn main() {
    // The client tree: root-relative path → (framework, source).
    let files: BTreeMap<String, (Framework, String)> = [
        (
            "src/islands/counter.tsx".to_string(),
            (
                Framework::React,
                "import { useState } from 'react';\nexport default function Counter() {\n  const [n, setN] = useState(0);\n  return <button onClick={() => setN(n + 1)}>{n}</button>;\n}\n".to_string(),
            ),
        ),
        (
            "src/islands/gallery.tsx".to_string(),
            (
                Framework::React,
                "import Counter from './counter';\nexport default function Gallery() {\n  return <section><Counter /><p>more</p></section>;\n}\n".to_string(),
            ),
        ),
        (
            "src/islands/price.vue".to_string(),
            (
                Framework::Vue,
                "<script setup>\nconst label = 'from €9';\n</script>\n<template>\n  <button @click=\"buy\">{{ label }}</button>\n</template>\n".to_string(),
            ),
        ),
        (
            "src/islands/badge.svelte".to_string(),
            (
                Framework::Svelte,
                "<p>ships in 24h</p>\n".to_string(),
            ),
        ),
        (
            "src/islands/broken.tsx".to_string(),
            (
                Framework::React,
                "export default function Broken( {{{ <not js\n".to_string(),
            ),
        ),
    ]
    .into_iter()
    .collect();

    let manifest = IslandManifest::build(&files);
    println!("{}", manifest.to_json());
    println!();
    println!("interactive (the auto hydration set):");
    for file in manifest.interactive_files() {
        println!("  {file}");
    }

    // The hydration decision per candidate: the counter was ALSO
    // explicitly registered, so it hydrates twice over; the pure badge is
    // the only skip; the broken file hydrates (the conservative arm).
    println!();
    for (file, explicit) in [
        ("src/islands/counter.tsx", true),
        ("src/islands/gallery.tsx", false),
        ("src/islands/price.vue", false),
        ("src/islands/badge.svelte", false),
        ("src/islands/broken.tsx", false),
    ] {
        let verdict = hydrates(explicit, manifest.get(file));
        println!("hydrates({file}, explicit={explicit}) = {verdict}");
    }

    // The side-car JSON round trip the build's next phase reads.
    let back = IslandManifest::from_json(&manifest.to_json()).expect("round trip");
    println!();
    println!("side-car round trip identical: {}", back == manifest);
}
