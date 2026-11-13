//! Criterion bench for the V2-E02 typed-search-params substrate — the
//! per-build cost typegen pays around the search dimension.
//!
//! The shapes: the `searchSchema` declaration scan over one server module
//! (the per-route loop the build pays once per route with a server
//! module), and a synthetic 1,000-route scan sweep (the build-time cost
//! at scale — the scan is a line walk, so the sweep is linear in routes).
//! The emission itself is a file write (measured by the existing
//! deterministic pins, not by a bench).
//!
//! Run: `cargo bench -p oxide-fw-router --features bench-util`

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_router::declares_search_schema;

/// One server module with the declaration (the WP's example shape).
const DECLARING: &str = "import * as s from '@oxide/schema';\n\
export const searchSchema = s.object({ slug: s.string(), tag: s.optional(s.string()) });\n\
export const load = () => ({ data: 'blog' });\n";

/// One server module without the declaration (the negative case — the
/// scan must reject it, which is the common build's per-route work).
const PLAIN: &str = "export const load = () => ({ data: 'blog' });\n";

fn bench_typegen_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("typegen_search");
    group.throughput(criterion::Throughput::Bytes(DECLARING.len() as u64));

    // The positive scan: one declaring server module.
    group.bench_function("scan_declaring", |b| {
        b.iter(|| {
            assert!(declares_search_schema(DECLARING));
        });
    });

    // The negative scan: one plain server module (the majority case).
    group.bench_function("scan_plain", |b| {
        b.iter(|| {
            assert!(!declares_search_schema(PLAIN));
        });
    });

    // The build-scale sweep: 1,000 server modules (every tenth declaring),
    // scanned the way the build's per-route loop walks them.
    let sources: Vec<String> = (0..1000)
        .map(|i| {
            if i % 10 == 0 {
                DECLARING.to_string()
            } else {
                PLAIN.to_string()
            }
        })
        .collect();
    group.bench_function(BenchmarkId::new("scan_sweep", 1000), |b| {
        b.iter(|| {
            let declared = sources.iter().filter(|s| declares_search_schema(s)).count();
            assert_eq!(declared, 100, "every tenth module declares");
        });
    });

    group.finish();
}

criterion_group!(benches, bench_typegen_search);
criterion_main!(benches);
