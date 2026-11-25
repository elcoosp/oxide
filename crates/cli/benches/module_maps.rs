//! §V2-E08 item 1: the module-map store's cost bench — the boot's
//! `ModuleMaps` under the dev-loop surfaces that touch it: the insert a
//! lowered module's map joins through, and the frame mapping the
//! `cmd_dev` publish sites run per HMR error (a generated frame through
//! a 64-module store, against the `NoMaps` floor).
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! module_maps` (the bench-util opt-in keeps plain `cargo test` off
//! criterion).

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cli::mapped_hmr_error;
use oxide_fw_compiler::lower_to_js_with_refresh;
use oxide_fw_dev::frames::ModuleMaps;
use oxide_fw_diagnostics::{Diagnostic, StackFrame};
use std::hint::black_box;
use std::sync::{Arc, Mutex};

/// A 64-module store filled with REAL lowered maps (a page per module —
/// the store's steady-state shape deep into a dev session).
fn seeded_store() -> Arc<Mutex<ModuleMaps>> {
    let mut maps = ModuleMaps::default();
    for i in 0..64 {
        let path = format!("src/routes/page{i}.tsx");
        let src = format!("export default function Page{i}() {{\n  return <main>hi</main>;\n}}\n");
        if let Ok((_, Some(map))) = lower_to_js_with_refresh(&src, "tsx", &path, None) {
            maps.insert(path, map);
        }
    }
    Arc::new(Mutex::new(maps))
}

fn bench_module_maps(c: &mut Criterion) {
    let mut group = c.benchmark_group("module_maps");
    let store = seeded_store();

    // The publish-site cost: one HMR error's frame mapped through the
    // store (the changed module's map answers; the DEV-BUNDLE attach +
    // map arc the three publish sites run).
    group.bench_function("mapped_hmr_error_hit", |b| {
        let changed = std::path::Path::new("src/routes/page17.tsx");
        b.iter(|| {
            let diag = Diagnostic::error("DEV-BUNDLE", "rebuild failed");
            let out = mapped_hmr_error(
                Some(black_box(&store)),
                std::path::Path::new("/proj"),
                black_box(changed),
                diag,
            );
            assert!(out.frames[0].mapped);
        });
    });

    // The miss arm: a frame no map speaks for keeps its generated
    // position (the honest shape) — the lookup-miss cost.
    group.bench_function("mapped_hmr_error_miss", |b| {
        let changed = std::path::Path::new("/proj/src/lib/never-served.ts");
        b.iter(|| {
            let diag = Diagnostic::error("DEV-HMR-001", "no boundary");
            let out = mapped_hmr_error(
                Some(black_box(&store)),
                std::path::Path::new("/proj"),
                black_box(changed),
                diag,
            );
            assert_eq!(out.frames.len(), 1);
            assert!(!out.frames[0].mapped);
        });
    });

    // The floor: no store wired (the pre-adoption posture) — the payload
    // builds unchanged.
    group.bench_function("no_maps_floor", |b| {
        b.iter(|| {
            let diag = Diagnostic::error("DEV-BUNDLE", "boom").with_frames(vec![StackFrame {
                file: "src/routes/page17.tsx".to_string(),
                line: 2,
                column: 1,
                function: None,
                mapped: false,
            }]);
            let out = mapped_hmr_error(
                None,
                std::path::Path::new("/proj"),
                black_box(&std::path::PathBuf::from("src/routes/page17.tsx")),
                diag,
            );
            assert!(!out.frames[0].mapped);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_module_maps);
criterion_main!(benches);
