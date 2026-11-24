//! §V2-F10 step 4: the rollback slice's cost bench — the emit-tail
//! retention ([`oxide_fw_cli::serve::generations::retain_dist_generation`])
//! and the swap ([`oxide_fw_cli::serve::generations::repoint_dist_to_generation`])
//! over a synthetic dist tree.
//!
//! The retention runs at the end of every build, so its cost is build-path
//! overhead: the copy tree's per-file price over 64 files (the fallback
//! posture — a real copy per the recorded delta) is what a deploy-size
//! dist scales from. The swap is the operator's rollback latency.
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! generation_rollback` (the bench-util opt-in keeps plain `cargo test`
//! off criterion).

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cli::serve::generations::{
    repoint_dist_to_generation, resolve_generation, retain_dist_generation,
};
use std::hint::black_box;

/// A synthetic dist: 64 chunk files under `client/assets/` (the shape a
/// real dist's client half takes), stamped through the A04 seam so every
/// generation carries a build id.
fn stamp_dist(root: &std::path::Path, body: &[u8]) -> String {
    let dist = root.join("dist");
    for i in 0..64 {
        let dir = dist.join("client/assets").join(format!("chunk{i:02}"));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("m.js"), body).expect("write");
    }
    oxide_fw_cache::stamp_build_id(&dist).expect("stamp")
}

fn bench_generation_rollback(c: &mut Criterion) {
    let mut group = c.benchmark_group("generation_rollback");

    // The emit-tail cost: retaining a 64-file dist into the generations
    // store, from scratch each iteration (the file set is re-created so
    // the copy walk pays for real bytes).
    group.bench_function("retain_64_files", |b| {
        b.iter_with_large_drop(|| {
            let tmp = tempfile::tempdir().expect("tmp");
            let id = stamp_dist(tmp.path(), b"export default 1;\n");
            let retained = retain_dist_generation(tmp.path(), black_box(&id), 3).expect("retain");
            assert!(retained.is_dir());
        });
    });

    // The rollback latency: resolve `previous` + the dist swap, both
    // against an already-retained pair (the operator's path).
    group.bench_function("resolve_and_repoint", |b| {
        let tmp = tempfile::tempdir().expect("tmp");
        let v1 = stamp_dist(tmp.path(), b"export default 1;\n");
        retain_dist_generation(tmp.path(), &v1, 3).expect("retain v1");
        let v2 = stamp_dist(tmp.path(), b"export default 2;\n");
        retain_dist_generation(tmp.path(), &v2, 3).expect("retain v2");
        b.iter(|| {
            let gen_tree =
                resolve_generation(tmp.path(), black_box("previous"), Some(black_box(&v2)))
                    .expect("resolve");
            repoint_dist_to_generation(tmp.path(), &gen_tree).expect("repoint");
            // The alternation: after the swap the other generation is
            // `previous`, so every iteration swaps a fresh pair.
            let back = resolve_generation(tmp.path(), "previous", None).expect("resolve back");
            repoint_dist_to_generation(tmp.path(), &back).expect("repoint back");
        });
    });

    group.finish();
}

criterion_group!(benches, bench_generation_rollback);
criterion_main!(benches);
