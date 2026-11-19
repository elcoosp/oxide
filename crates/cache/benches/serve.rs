//! Criterion bench for the cache plane's §11.3 serving half (F54): the
//! per-serve cost a host pays for the ONE call (`serve_entry`) — the
//! canonical conditional comparison and the §11.3 header composition.
//!
//! Three shapes: the 304 arm (validators match — the comparison plus the
//! validator-set build), the full arm with a conditional miss (stored
//! headers plus the generated gaps), and the floor with no conditional
//! headers at all (the composition a first visit pays).

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{Entry, Policy, serve_entry};

fn entry() -> Entry {
    Entry {
        body: b"<html>the rendered page</html>".to_vec(),
        status: 200,
        headers: vec![
            ("content-type".into(), "text/html".into()),
            (
                "last-modified".into(),
                "Sun, 06 Nov 1994 08:49:37 GMT".into(),
            ),
        ],
        tags: vec![],
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: Some(Duration::from_secs(30)),
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: 1_795_046_400_000,
        etag: "\"v1\"".into(),
    }
}

fn bench_serve(c: &mut Criterion) {
    let e = entry();
    let mut group = c.benchmark_group("cache_serve_entry");
    group.throughput(criterion::Throughput::Elements(1));

    // The 304 arm: matching If-None-Match — the comparison plus the
    // validator set (the cheap path a re-checking browser rides).
    group.bench_with_input(BenchmarkId::new("not-modified", 304), &e, |b, e| {
        let inm = vec![("if-none-match".to_string(), "\"v1\"".to_string())];
        b.iter(|| {
            let out = serve_entry(black_box(e), black_box(&inm), 1_795_046_400);
            black_box(matches!(
                out,
                oxide_fw_cache::ServeOutcome::NotModified { .. }
            ));
        });
    });

    // The full arm with a conditional miss: stored headers + the
    // generated gaps (etag / cache-control / date) composed per serve.
    group.bench_with_input(BenchmarkId::new("full-miss", 200), &e, |b, e| {
        let inm = vec![("if-none-match".to_string(), "\"v2\"".to_string())];
        b.iter(|| {
            let out = serve_entry(black_box(e), black_box(&inm), 1_795_046_400);
            black_box(matches!(out, oxide_fw_cache::ServeOutcome::Full { .. }));
        });
    });

    // The floor: no conditional headers at all — the composition every
    // first visit pays.
    group.bench_with_input(BenchmarkId::new("full-first-visit", 0), &e, |b, e| {
        b.iter(|| {
            let out = serve_entry(black_box(e), black_box(&[]), 1_795_046_400);
            black_box(matches!(out, oxide_fw_cache::ServeOutcome::Full { .. }));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_serve);
criterion_main!(benches);
