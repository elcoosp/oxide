//! Criterion bench for the generated capability-matrix docs table (E11's
//! `doc_table_matches_matrix` seam) — the deterministic markdown render the
//! crate README embeds and a future docs generator would call.
//!
//! The shape: the full emitter (12 targets × 18 capabilities — a fresh
//! `String` per call, the docs path's worst case), measured so a docs-side
//! consumer (per-PR or per-page render) knows its cost.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-adapters --features bench-util -- doc_table
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_adapters::capability_markdown_table;

fn bench_doc_table(c: &mut Criterion) {
    let mut group = c.benchmark_group("doc_table");

    // The full render: the exact bytes the README embeds (the E11 pin
    // holds them together, so this measures the pinned shape).
    group.bench_function("capability_markdown_table", |b| {
        b.iter(capability_markdown_table);
    });

    group.finish();
}

criterion_group!(benches, bench_doc_table);
criterion_main!(benches);
