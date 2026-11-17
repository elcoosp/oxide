//! Criterion bench for the plugin-compat matrix (the E05 substrate seam) —
//! the adapters `doc_table` pattern: the full matrix fold and the
//! deterministic markdown render the published table embeds, measured so a
//! docs-side consumer (per-PR or per-page render) knows its cost.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-plugin --features bench-util -- compat
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_plugin::{HOOKS, PluginKind, compat, compat_markdown_table};

fn bench_compat_matrix(c: &mut Criterion) {
    let mut group = c.benchmark_group("compat");

    // The full fold: every kind × hook cell (19 hooks × 3 kinds), the
    // compatibility runner's worst case per plugin load.
    group.bench_function("matrix_19x3", |b| {
        b.iter(|| {
            for hook in HOOKS {
                for kind in [PluginKind::Js, PluginKind::Wasm, PluginKind::Native] {
                    let _ = compat(kind, *hook);
                }
            }
        });
    });

    // The full render: the exact bytes docs/plugin-compat.md embeds (the
    // byte-identical pin holds them together, so this measures the pinned
    // shape).
    group.bench_function("markdown_table", |b| b.iter(compat_markdown_table));

    group.finish();
}

criterion_group!(benches, bench_compat_matrix);
criterion_main!(benches);
