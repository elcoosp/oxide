//! V2-R01: dump the §18 capability matrix as JSON — the one source of truth
//! the conformance runner's skip engine reads (`conformance/matrix.json`).
//!
//! The Rust [`oxide_fw_adapters::capability_matrix`] / [`support`] /
//! [`degradation`] functions ARE the matrix (V2-R02); this example is the
//! JSON bridge the V8-free TS runner consumes so it never hand-maintains a
//! second capability table. Regenerate the committed snapshot with:
//!
//! ```text
//! cargo run -p oxide-fw-adapters --example dump_matrix > conformance/matrix.json
//! ```
//!
//! `crates/adapters/tests/matrix_json.rs` is the drift guard: it reloads the
//! committed JSON and asserts every cell matches `capability_matrix()`, so a
//! Rust-side change to a row cannot land without the TS bridge moving too.

use oxide_fw_adapters::{Capability, FeatureSupport, Target, degradation, support};
use serde_json::{Map, Value, json};

/// The lowercase spelling [`FeatureSupport`] serializes to (the matrix JSON
/// uses the same wire shape the `serde` derive on the enum produces).
fn feature_support_str(s: FeatureSupport) -> &'static str {
    match s {
        FeatureSupport::Full => "full",
        FeatureSupport::Degraded => "degraded",
        FeatureSupport::Unsupported => "unsupported",
    }
}

fn main() {
    // One row per target in `Target::ALL` order, one column per capability in
    // `Capability::ALL` order — the same order `capability_markdown_table`
    // prints, so the JSON and the docs table never disagree on layout.
    let targets: Vec<Value> = Target::ALL
        .iter()
        .map(|t| Value::String(t.as_str().to_owned()))
        .collect();
    let capabilities: Vec<Value> = Capability::ALL
        .iter()
        .map(|c| Value::String(c.as_str().to_owned()))
        .collect();

    let mut cells: Map<String, Value> = Map::new();
    let mut degradations: Map<String, Value> = Map::new();
    for t in Target::ALL {
        let mut row: Map<String, Value> = Map::new();
        let mut deg_row: Map<String, Value> = Map::new();
        for c in Capability::ALL {
            let s = support(*t, *c);
            row.insert(
                c.as_str().to_owned(),
                Value::String(feature_support_str(s).to_owned()),
            );
            // `degradation` returns `None` for Full/Unsupported — absence is
            // not a degradation (the §18 refusal rule covers Unsupported at
            // build time). The JSON carries `null` so the TS side reads one
            // shape for every cell.
            let deg = degradation(*t, *c).map(str::to_owned);
            deg_row.insert(
                c.as_str().to_owned(),
                deg.map_or(Value::Null, Value::String),
            );
        }
        cells.insert(t.as_str().to_owned(), Value::Object(row));
        degradations.insert(t.as_str().to_owned(), Value::Object(deg_row));
    }

    let out = json!({
        "targets": targets,
        "capabilities": capabilities,
        "cells": cells,
        "degradations": degradations,
    });
    // Pretty-printed so a committed diff reads cell-by-cell and a reviewer
    // can see a row change at a glance (the drift-guard test is the real
    // check; readability is the human one).
    println!("{out:#}");
}
