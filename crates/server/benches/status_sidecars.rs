//! Criterion benches for the V2-A07 prerender error-status sidecar
//! (`bench-util` gated like the V2-A02/V2-A05/V2-B09/V2-F05/V2-F07/V2-F12
//! benches — plain `cargo test` never pays the criterion build).
//!
//! The wiring-relevant questions:
//! - `status_sidecars_scan`: the serve-start walk — the ONE-TIME cost that
//!   must stay O(dist size) with no per-request I/O afterwards. Shapes:
//!   an error-free dist (256 pages, no sidecars — the common build) and an
//!   error-heavy dist (256 pages, 64 sidecars).
//! - `status_for`: the per-request lookup cost — one `HashMap` probe on a
//!   miss (a 200 page, the hot path) and a hit (a sidecar'd error page).
//!
//! Run with: `cargo bench -p oxide-fw-server --features bench-util --bench status_sidecars`

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_server::StatusSidecars;

/// A synthetic dist: `routes` prerendered pages under `prerendered/`,
/// every `err_every`-th one carrying a sidecar.
fn fixture_dist(routes: usize, err_every: usize) -> (tempfile::TempDir, Vec<PathBuf>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut pages = Vec::with_capacity(routes);
    for i in 0..routes {
        let rel = if i % 7 == 0 {
            format!("prerendered/locale{i}/page{i}.html")
        } else {
            format!("prerendered/page{i}.html")
        };
        let page = tmp.path().join(&rel);
        if let Some(parent) = page.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(
            &page,
            format!("<!doctype html><html><body>page {i}</body></html>"),
        )
        .expect("write page");
        if i % err_every == 0 {
            let sidecar = oxide_fw_server::sidecar_path_for(&page);
            std::fs::write(&sidecar, "500").expect("write sidecar");
        }
        pages.push(page);
    }
    (tmp, pages)
}

fn bench_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("status_sidecars_scan");
    for (label, err_every) in [("no_sidecars", 0), ("64_of_256", 4)] {
        let (tmp, _pages) = fixture_dist(
            256,
            if err_every == 0 {
                usize::MAX
            } else {
                err_every
            },
        );
        group.bench_function(BenchmarkId::new("dist_256", label), |b| {
            b.iter(|| StatusSidecars::scan(black_box(tmp.path())))
        });
    }
    group.finish();
}

fn bench_status_for(c: &mut Criterion) {
    let (tmp, pages) = fixture_dist(256, 4);
    let sidecars = StatusSidecars::scan(tmp.path());
    let hit = &pages[0]; // i % 4 == 0 → sidecar'd
    let miss = &pages[3]; // i % 4 != 0 → 200
    assert_eq!(sidecars.status_for(hit), Some(500), "fixture sanity: hit");
    assert_eq!(sidecars.status_for(miss), None, "fixture sanity: miss");
    let mut group = c.benchmark_group("status_for");
    group.bench_function("hit_error_page", |b| {
        b.iter(|| sidecars.status_for(black_box(hit)))
    });
    group.bench_function("miss_200_page", |b| {
        b.iter(|| sidecars.status_for(black_box(miss)))
    });
    group.finish();
}

criterion_group!(benches, bench_scan, bench_status_for);
criterion_main!(benches);
