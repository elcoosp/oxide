//! Criterion benches for the V2-E08 open-in-editor endpoint (the server
//! half), `bench-util` gated like every other criterion bench — plain
//! `cargo test` never pays the criterion build.
//!
//! The wiring-relevant questions:
//! - `open_resolve`: the env chain + split the endpoint runs per request
//!   (the resolver is deliberately per-request — a dev session can export
//!   `OXIDE_EDITOR` late).
//! - `open_serve`: the full request → response against a no-op opener at
//!   the three shapes the overlay sends: the valid file (the
//!   canonicalize + starts_with confinement is the cost the happy path
//!   pays), the `../` escape (the refusal cost the attackers pay) and the
//!   missing file (the canonicalize-failure branch).
//!
//! Run with: `cargo bench -p oxide-fw-server --features bench-util --bench open_editor`

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_server::{EditorCommand, EditorOpener, OpenEditorEndpoint, resolve_editor};

/// A no-op launcher (the launch itself is one `spawn` — the pin shapes the
/// pure decision cost, not the process cost).
struct NoopOpener;

impl EditorOpener for NoopOpener {
    fn open(&self, _cmd: &EditorCommand) -> Result<(), oxide_fw_diagnostics::OxideError> {
        Ok(())
    }
}

/// A project fixture with nested dirs (the canonicalize target the
/// confinement walks).
fn fixture(depth: usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("oxide-open-bench-{depth}-{}", std::process::id()));
    let mut path = dir.clone();
    for i in 0..depth {
        path.push(format!("d{i}"));
    }
    std::fs::create_dir_all(&path).expect("mkdir");
    std::fs::write(path.join("a.rs"), "export const a = 1;\n").expect("write");
    dir
}

fn bench_resolve(c: &mut Criterion) {
    let getenv = |k: &str| (k == "EDITOR").then(|| "code -g".to_string());
    c.benchmark_group("open_resolve")
        .bench_function("chain/editor", |b| {
            b.iter(|| {
                resolve_editor(black_box(&getenv)).expect("resolves");
            });
        });
}

fn bench_serve(c: &mut Criterion) {
    let root = fixture(4);
    let ep = OpenEditorEndpoint::new(root.clone(), Arc::new(NoopOpener) as Arc<dyn EditorOpener>);
    let mut group = c.benchmark_group("open_serve");
    group.bench_function("ok/file+line", |b| {
        b.iter(|| {
            let resp = ep.serve(black_box("GET"), black_box("file=d0/d1/d2/d3/a.rs&line=12"));
            black_box(resp.status);
        });
    });
    group.bench_function("refused/dotdot-escape", |b| {
        b.iter(|| {
            let resp = ep.serve(black_box("GET"), black_box("file=../../etc/passwd"));
            black_box(resp.status);
        });
    });
    group.bench_function("refused/missing-file", |b| {
        b.iter(|| {
            let resp = ep.serve(black_box("GET"), black_box("file=d0/d1/d2/ghost.rs"));
            black_box(resp.status);
        });
    });
}

criterion_group!(benches, bench_resolve, bench_serve);
criterion_main!(benches);
