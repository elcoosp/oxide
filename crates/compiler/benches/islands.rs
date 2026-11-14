//! Criterion bench for the §V2-F01 island boot planner — the cost of
//! planning + emitting the `__boot__` entry and the tooling payload
//! for a single-framework registry and a mixed one (the spec's
//! mixed-islands shape: one lazy adapter per framework present).

use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_compiler::islands::{ResolvedIsland, plan_island_boot};

fn registry(n: usize, mixed: bool) -> std::collections::BTreeMap<String, ResolvedIsland> {
    let frameworks = if mixed {
        ["vue", "solid", "svelte", "react"]
    } else {
        ["react"]
    };
    (0..n)
        .map(|i| {
            let fw = frameworks[i % frameworks.len()];
            (
                format!("Island{i}"),
                ResolvedIsland {
                    src: format!("src/islands/island{i}.tsx"),
                    framework: fw.to_string(),
                },
            )
        })
        .collect()
}

fn bench_islands(c: &mut Criterion) {
    c.bench_function("island_boot_plan_single_8", |b| {
        b.iter(|| {
            let plan = plan_island_boot("react", black_box(&registry(8, false)));
            black_box(plan.boot_entry_source(Path::new("/proj"), &Default::default(), false))
        })
    });
    c.bench_function("island_boot_plan_mixed_32", |b| {
        b.iter(|| {
            let plan = plan_island_boot("react", black_box(&registry(32, true)));
            black_box((
                plan.boot_entry_source(Path::new("/proj"), &Default::default(), false),
                plan.islands_json(),
            ))
        })
    });
}

criterion_group!(benches, bench_islands);
criterion_main!(benches);
