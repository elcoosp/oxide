//! Criterion benches for the §V2-A04 build-id substrate
//! (`build_id_of_dist`, `bench-util` gated like the V2-B09/F04 benches —
//! plain `cargo test` never pays the criterion build).
//!
//! The wiring-relevant question is the end-of-`phase_emit` cost of
//! recomputing the canonical build id over the emitted tree: the walk plus
//! one blake3 pass per covered file, at three tree sizes (10 / 50 / 200
//! files, ~2 KiB each — the shape of a small app's `dist/client` +
//! `dist/server`).
//!
//! Run: `cargo bench -p oxide-fw-cache --features bench-util --bench build_id`

use std::fs;
use std::hint::black_box;
use std::path::PathBuf;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::build_id_of_dist;
use tempfile::TempDir;

/// A fixture tree: `files` covered files of ~2 KiB spread over
/// `dist/client/**` and `dist/server/**`, plus the route manifest.
fn fixture_tree(files: usize) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("tmp");
    let root = tmp.path().to_path_buf();
    let mut payload = Vec::with_capacity(2048);
    for i in 0..2048 {
        payload.push((i % 251) as u8);
    }
    for i in 0..files {
        let half = i % 2 == 0;
        let dir = if half {
            root.join("dist/client/chunks")
        } else {
            root.join("dist/server")
        };
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join(format!("f{i}.js")), &payload).expect("write");
    }
    fs::create_dir_all(root.join(".oxide")).expect("mkdir");
    fs::write(root.join(".oxide/routes.manifest"), b"routes").expect("write");
    (tmp, root.join("dist"))
}

fn bench_build_id_of_dist(c: &mut Criterion) {
    let mut group = c.benchmark_group("build_id");
    for files in [10usize, 50, 200] {
        let (_tmp, dist) = fixture_tree(files);
        group.bench_with_input(BenchmarkId::from_parameter(files), &dist, |b, dist| {
            b.iter(|| black_box(build_id_of_dist(black_box(dist)).expect("id")));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_build_id_of_dist);
criterion_main!(benches);
