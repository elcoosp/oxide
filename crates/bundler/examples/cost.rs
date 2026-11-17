//! The §V2-F17 build-side cost engine walkthrough: the per-source
//! `{ bytes, gzip, chunk }` records the build produces for the landed
//! `/__oxide/cost?src=<path>` endpoint. A fake bundle output (the
//! engine's input shape — the real one comes from `BundleOutput` after
//! any backend) is measured, and the table the CLI adapter feeds the
//! endpoint's `CostSource` is printed — the source keys are the
//! `data-ox-src` stamps' spelling, so the overlay's fetch needs no
//! re-spelling.
//!
//! ```text
//! cargo run -p oxide-fw-bundler --example cost
//! ```

use std::path::{Path, PathBuf};

use oxide_fw_bundler::cost::source_costs;
use oxide_fw_bundler::{BundleOutput, Chunk, ChunkKind, Module};

fn module(resolved: &str, source: &str, is_asset: bool, server_only: bool) -> Module {
    Module {
        specifier: resolved.rsplit('/').next().unwrap_or(resolved).to_string(),
        resolved: PathBuf::from(resolved),
        source: source.to_string(),
        hash: format!("h{resolved}"),
        imports: Vec::new(),
        is_asset,
        server_only,
    }
}

fn main() {
    // A small route graph: the page, two islands, a chunk claiming the
    // page, an asset and a server-only module (both excluded).
    let output = BundleOutput {
        modules: vec![
            module(
                "/proj/src/routes/index.tsx",
                "export default function Page() {\n  return <main>hi</main>;\n}\n",
                false,
                false,
            ),
            module(
                "/proj/src/islands/card.tsx",
                "export function Card() {\n  return <b>card</b>;\n}\n",
                false,
                false,
            ),
            module(
                "/proj/src/islands/counter.tsx",
                "import { useState } from 'react';\nexport function Counter() {\n  const [n, setN] = useState(0);\n  return <button onClick={() => setN(n + 1)}>{n}</button>;\n}\n",
                false,
                false,
            ),
            module(
                "/proj/src/logo.svg",
                "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                true,
                false,
            ),
            module(
                "/proj/src/routes/index.server.ts",
                "export async function load() { return { ok: true }; }\n",
                false,
                true,
            ),
        ],
        chunks: vec![Chunk {
            name: "entry.index".to_string(),
            file: "entry.index-abc123.js".to_string(),
            hash: "h9".to_string(),
            modules: vec![PathBuf::from("/proj/src/routes/index.tsx")],
            imports: Vec::new(),
            css: Vec::new(),
            kind: ChunkKind::Entry,
        }],
        client_manifest: None,
        server_manifest: None,
        diagnostics: Vec::new(),
    };

    let root = Path::new("/proj");
    println!("== the build-side cost table (the CostSource feed) ==");
    println!("{:<28} {:>7} {:>7}  chunk", "source", "bytes", "gzip");
    for cost in source_costs(&output, root) {
        println!(
            "{:<28} {:>7} {:>7}  {}",
            cost.source,
            cost.bytes,
            cost.gzip,
            cost.chunk.as_deref().unwrap_or("-")
        );
    }
    println!(
        "\nthe asset (`logo.svg`) and the server-only module are excluded — the overlay attributes CLIENT sources"
    );
    println!(
        "the source keys are the `data-ox-src` stamps' spelling: `GET /__oxide/cost?src=src/islands/card.tsx` fetches a record the overlay renders directly"
    );
}
