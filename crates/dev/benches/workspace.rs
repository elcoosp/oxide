//! Criterion bench for the V2-E12 workspace-links substrate (the B09
//! pattern: plain `cargo test` never pays the criterion build — run behind
//! `--features bench-util`).
//!
//! The wiring-relevant question is the cost of the two call sites:
//!
//! - `workspace_links/discovery_{8,16}`: the discovery pass `oxide dev`
//!   boots once — the workspace-manifest walk, the glob expansion over the
//!   fixture's package tree, the app-manifest dependency read and the
//!   `oxc_resolver` work per linked dependency (the fixture links ONE
//!   package, so the group isolates discovery from resolution fan-out).
//! - `workspace_links/real_path`: the package-edge bridge the importers-
//!   graph harvest runs per resolved node — the canonicalize that maps a
//!   `node_modules`-arrived path onto the real tree (the hit path; the
//!   miss path is a caller-side `None` on the same probe).
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-dev --features bench-util -- workspace_links
//! ```

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dev::workspace::{ManifestWorkspaceLinks, WorkspaceLinks, real_path};
use std::path::PathBuf;

/// A fixture workspace with `n` candidate packages under `packages/` and
/// one app (`apps/web`) that links exactly one of them. The linked
/// package's `node_modules` entry is a real directory (the copied layout
/// the resolver serves on every OS), so the bench compiles and runs
/// without symlink privileges.
fn fixture_workspace(n: usize) -> (tempfile::TempDir, PathBuf) {
    let ws = tempfile::tempdir().expect("tempdir");
    let root = ws.path().to_path_buf();
    let app = root.join("apps/web");
    std::fs::create_dir_all(&app).expect("app");
    std::fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\n",
    )
    .expect("manifest");
    std::fs::write(
        app.join("package.json"),
        r#"{"name":"web","dependencies":{"@acme/pkg0":"workspace:*"}}"#,
    )
    .expect("app manifest");
    for i in 0..n {
        let pkg = root.join(format!("packages/pkg{i}"));
        std::fs::create_dir_all(pkg.join("src")).expect("pkg src");
        std::fs::write(
            pkg.join("package.json"),
            format!(r#"{{"name":"@acme/pkg{i}","main":"./src/index.ts"}}"#),
        )
        .expect("pkg manifest");
        std::fs::write(pkg.join("src/index.ts"), "export {};\n").expect("entry");
    }
    let linked = root.join("packages/pkg0");
    let nm = app.join("node_modules/@acme/pkg0");
    std::fs::create_dir_all(&nm).expect("node_modules");
    std::fs::write(
        nm.join("package.json"),
        format!(
            r#"{{"name":"@acme/pkg0","main":"{}"}}"#,
            linked.join("src/index.ts").display()
        ),
    )
    .expect("linked manifest");
    (ws, app)
}

fn bench_workspace_links(c: &mut Criterion) {
    let mut group = c.benchmark_group("workspace_links");
    for n in [8usize, 16] {
        let (ws, app) = fixture_workspace(n);
        group.bench_with_input(BenchmarkId::new("discovery", n), &app, |b, app| {
            b.iter(|| ManifestWorkspaceLinks.links(app).expect("discovery"))
        });
        drop(ws);
    }
    let (ws, _app) = fixture_workspace(1);
    let pkg_root = ws.path().join("packages/pkg0");
    group.bench_function("real_path", |b| {
        b.iter(|| real_path(&pkg_root).expect("canonicalize"))
    });
    group.finish();
}

criterion_group!(benches, bench_workspace_links);
criterion_main!(benches);
