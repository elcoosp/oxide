//! The §V2-F02 dev-warn walkthrough: the props surface the dev transform
//! pipeline sees, crossed with the island manifest's pure
//! classifications. A pure `Button` receives `onClick` + `onInput` from
//! the page — the warn the dev server logs when it serves the page; the
//! interactive `Counter` receives nothing (no usage), a fixpoint-promoted
//! `Chain` receives an event prop without warning (its classification is
//! already interactive), and an unscanned defining file carries no vouch.
//!
//! ```text
//! cargo run -p oxide-fw-compiler --example props_surface
//! ```

use std::collections::BTreeMap;

use oxide_fw_compiler::framework::Framework;
use oxide_fw_compiler::interactivity::IslandManifest;
use oxide_fw_compiler::props_surface::{pure_on_props_warnings, scan_on_props};

fn main() {
    // The client tree: root-relative path → (framework, source) — the
    // same shape `IslandManifest::build` consumes at dev boot.
    let files: BTreeMap<String, (Framework, String)> = [
        (
            "src/islands/button.tsx".to_string(),
            (
                Framework::React,
                "export default function Button(props: { children?: unknown }) {\n  return <button>{props.children}</button>;\n}\n".to_string(),
            ),
        ),
        (
            "src/islands/counter.tsx".to_string(),
            (
                Framework::React,
                "import { useState } from 'react';\nexport default function Counter() {\n  const [n, setN] = useState(0);\n  return <button onClick={() => setN(n + 1)}>{n}</button>;\n}\n".to_string(),
            ),
        ),
        // Pure on its own hits, interactive through the counter it
        // imports — the fixpoint promotes it, so receiving event props is
        // the expected shape, not a mismatch.
        (
            "src/islands/chain.tsx".to_string(),
            (
                Framework::React,
                "import Counter from './counter';\nexport default function Chain() {\n  return <Counter />;\n}\n".to_string(),
            ),
        ),
        (
            "routes/page.tsx".to_string(),
            (
                Framework::React,
                "import Button from '../src/islands/button';\nimport Counter from '../src/islands/counter';\nimport Chain from '../src/islands/chain';\nexport default function Page() {\n  return (\n    <main>\n      <Button onClick={() => {}} onInput={() => {}}>go</Button>\n      <Chain onSubmit={() => {}} />\n      <Counter />\n    </main>\n  );\n}\n".to_string(),
            ),
        ),
    ]
    .into_iter()
    .collect();

    // The usage surface of the page — what the dev pipeline sees when it
    // serves the module (one parse for the usages and the bindings).
    let page = files.get("routes/page.tsx").unwrap().1.clone();
    println!("== the page's on* props surface ==");
    for usage in scan_on_props(&page) {
        println!(
            "  line {:>2}  <{} …> carries {}",
            usage.line,
            usage.component,
            usage.props.join(", ")
        );
    }

    // The manifest the dev boot scans (the same engine the build
    // side-car comes from — the classifications the hydration decision
    // consumes).
    let manifest = IslandManifest::build(&files);
    println!("\n== the manifest's classifications ==");
    for (file, inter) in manifest.iter() {
        println!(
            "  {:<24} {}",
            file,
            if inter.is_interactive() {
                "interactive"
            } else {
                "pure"
            }
        );
    }

    // The warn list: pure-classified defining files receiving on* props.
    println!("\n== the dev warn (AST-ISL-002) ==");
    let warns = pure_on_props_warnings(&files, &manifest);
    if warns.is_empty() {
        println!("  (none)");
    }
    for w in &warns {
        println!(
            "  warn: the pure-classified component `{}` (`{}`) receives {} at `{}:{}` — the auto-islands scan skips `{}`, so these handlers only run when the importing component hydrates",
            w.component,
            w.defining_file,
            w.props.join(", "),
            w.usage_file,
            w.line,
            w.defining_file
        );
    }
    println!(
        "\nthe counter carries no usage and the promoted chain never warns — {} warn(s) total",
        warns.len()
    );
}
