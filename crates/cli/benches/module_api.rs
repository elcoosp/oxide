//! §V2-E06 bench: the config-edit path (`add_module_to_config` on a
//! fixture config) and the manifest resolution (`resolve_one` →
//! `to_manifest`). A cheap criterion harness — before/after numbers go in
//! the PR body. Runs in test mode (`cargo test -p oxide-fw-cli --features
//! bench-util --benches -- --test` or `cargo bench -p oxide-fw-cli
//! --features bench-util`).
//!
//! The bench is gated behind `bench-util` (the V2-F18 pattern: plain
//! `cargo test` never pays the criterion build).

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use oxide_fw_cli::module_config_edit::add_module_to_config;
use oxide_fw_cli::modules_loader::{resolve_one, to_manifest};
use std::hint::black_box as bb;

/// A fixture config the same shape the `module_add` example edits — a
/// static `defineConfig({ … })` with a framework adapter + a routes dir.
const FIXTURE_CONFIG: &str = "// project config\n\
import { defineConfig } from '@oxide-fw/core'\n\
\n\
export default defineConfig({\n\
  framework: { adapter: 'react' },\n\
  routes: { dir: 'src/routes' },\n\
  // a comment that must survive the AST edit\n\
  plugins: [],\n\
})\n";

/// A fixture module package dir on disk — written to a tempdir so the
/// `resolve_one` + `to_manifest` path has a real `package.json` to read.
fn fixture_module() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("node_modules/@oxide/analytics");
    std::fs::create_dir_all(&pkg).expect("mkdir");
    std::fs::create_dir_all(pkg.join("routes")).expect("mkdir routes");
    std::fs::write(
        pkg.join("package.json"),
        r#"{"name":"@oxide/analytics","version":"1.0.0","main":"./index.ts","oxide":{"module":{"adapters":["react","vue","svelte","solid"],"hooks":["transform"],"configDefaults":{"analytics":{"endpoint":"/_oxide/vitals"}},"middlewares":[{"file":"./server/middleware.ts"}],"serverFns":[{"file":"./server/track.ts"}],"types":[{"name":"analytics","dts":"declare global {}"}]}}}"#,
    )
    .expect("write package.json");
    std::fs::write(pkg.join("index.ts"), "export default {};\n").expect("write index.ts");
    dir
}

fn bench_config_edit(c: &mut Criterion) {
    let mut group = c.benchmark_group("module_config_edit");
    group.bench_function("add_module_to_config", |b| {
        b.iter(|| {
            let edit = add_module_to_config(FIXTURE_CONFIG, "@oxide/analytics", Some("analytics"))
                .expect("config edit");
            black_box(edit.bytes.len());
        });
    });
    group.bench_function("add_module_to_config_idempotent", |b| {
        // The second pass: the import + modules[] are already present.
        let once = add_module_to_config(FIXTURE_CONFIG, "@oxide/analytics", Some("analytics"))
            .expect("first edit");
        b.iter(|| {
            let edit = add_module_to_config(&once.bytes, "@oxide/analytics", Some("analytics"))
                .expect("idempotent edit");
            assert!(!edit.import_added && !edit.module_added);
            black_box(edit.bytes.len());
        });
    });
    group.finish();
}

fn bench_manifest_resolution(c: &mut Criterion) {
    let dir = fixture_module();
    let pkg = dir.path().join("node_modules/@oxide/analytics");
    let mut group = c.benchmark_group("module_manifest");
    group.bench_function("resolve_one", |b| {
        b.iter(|| {
            let m = resolve_one(&pkg, "@oxide/analytics".to_string()).expect("resolve");
            black_box(m.adapters.len());
        });
    });
    group.bench_function("to_manifest", |b| {
        let m = resolve_one(&pkg, "@oxide/analytics".to_string()).expect("resolve");
        b.iter(|| {
            let manifest = to_manifest(bb(&m));
            black_box(manifest.hooks.len());
        });
    });
    group.finish();
}

criterion_group!(benches, bench_config_edit, bench_manifest_resolution);
criterion_main!(benches);
