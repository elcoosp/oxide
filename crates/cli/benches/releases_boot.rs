//! §V2-F10 step 1 + §V2-F11: the releases-boot slice's cost bench — the
//! `--releases` boot leg's two pieces over a synthetic deployed store:
//! the pointer resolution (the operator's every-boot cost: one small
//! read through the deploy engine's own parser) and the dist-view
//! materialization (the symlink farm over the release prefix + the
//! immutable store-root trees; the per-file price a deploy-size release
//! scales from).
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! releases_boot` (the bench-util opt-in keeps plain `cargo test` off
//! criterion).

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_adapters::{DeployStore, DirStore};
use oxide_fw_cli::serve::releases::{materialize_release_dist, resolve_release_boot};
use std::hint::black_box;

/// A synthetic deployed store: 64 immutable chunk files under `assets/`
/// (the shape a real store's shared tree takes), a release prefix with
/// the non-hashed dist files (build-id, prerendered HTML, manifests) and
/// the `current` pointer written LAST (the deploy's atomicity contract).
fn seed_store(root: &std::path::Path, id: &str) {
    let rel = root.join("releases").join(id);
    for i in 0..64 {
        let dir = root.join("assets").join(format!("chunk{i:02}"));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("m.js"), b"export const v = 1;\n").expect("write asset");
    }
    std::fs::create_dir_all(rel.join("prerendered")).expect("mkdir release");
    std::fs::write(rel.join("build-id"), format!("{id}\n")).expect("write build-id");
    std::fs::write(rel.join("prerendered/index.html"), "<html>release</html>").expect("write html");
    std::fs::write(rel.join("client-manifest.json"), b"{}").expect("write manifest");
    DirStore::new(root).put_pointer(id).expect("pointer");
}

fn bench_releases_boot(c: &mut Criterion) {
    let mut group = c.benchmark_group("releases_boot");
    let id = "0123456789abcdef";

    // The every-boot cost: pointer resolution + prefix verification
    // against an already-materialized store (the view is reused).
    group.bench_function("resolve_reused_view", |b| {
        let tmp = tempfile::tempdir().expect("tmp");
        seed_store(tmp.path(), id);
        materialize_release_dist(tmp.path(), id).expect("materialize");
        b.iter(|| resolve_release_boot(black_box(tmp.path())).expect("boot"));
    });

    // The cold boot: the materialization itself — the symlink farm over
    // 64 immutable assets + the release prefix, from scratch each
    // iteration (the view directory is re-created so the walk pays for
    // real entries).
    group.bench_function("materialize_64_assets_cold", |b| {
        let tmp = tempfile::tempdir().expect("tmp");
        seed_store(tmp.path(), id);
        b.iter(|| {
            let view = tmp.path().join(".oxide/releases-boot").join(id);
            let _ = std::fs::remove_dir_all(&view);
            let dist =
                materialize_release_dist(black_box(tmp.path()), black_box(id)).expect("boot");
            assert!(dist.join("prerendered/index.html").is_file());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_releases_boot);
criterion_main!(benches);
