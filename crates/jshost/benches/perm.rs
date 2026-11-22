//! Criterion benchmarks for the V2-F19 per-route capability permission
//! surface (`crates/jshost`).
//!
//! Cases: `host_allowed` (the wildcard predicate the net boundary calls
//! per fetch — exact-host hit, `*.` wildcard hit, spoof-rejection miss,
//! deny-all empty slice), `url_host` (the URL host extraction the fetch
//! op runs before the predicate), and the full fetch-path check
//! (`url_host` + `is_net_allowed` — the cost the `op_oxide_fetch`
//! boundary pays per fetch).
//!
//! V8-free — the predicate and the extractor are pure Rust in `perm.rs`,
//! so the bench runs without the `v8` feature (the sandbox reality). The
//! `PermGuard` seed/clear cycle (the RAII cost the worker loop pays once
//! per call) is the same shape as the `trace` bench's guard cycle and
//! rides that precedent.
//!
//! Run: `cargo bench -p oxide-fw-jshost --features bench-util`

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_jshost::perm::{PermissionSet, host_allowed, url_host};
use std::hint::black_box;

/// A grant with a small net allow-list (the typical app shape: one or
/// two hosts plus a wildcard).
fn typical_grant() -> PermissionSet {
    PermissionSet {
        net: Some(vec!["api.stripe.com".to_string(), "*.internal".to_string()]),
        env: None,
        fs: None,
    }
}

fn bench_host_allowed(c: &mut Criterion) {
    let exact = vec!["api.stripe.com".to_string()];
    let wildcard = vec!["*.internal".to_string()];
    let both = vec!["api.stripe.com".to_string(), "*.internal".to_string()];
    let empty: Vec<String> = Vec::new();

    let mut g = c.benchmark_group("perm_host_allowed");
    g.bench_function("exact_host_hit", |b| {
        b.iter(|| black_box(host_allowed(black_box(&exact), black_box("api.stripe.com"))));
    });
    g.bench_function("wildcard_hit", |b| {
        b.iter(|| {
            black_box(host_allowed(
                black_box(&wildcard),
                black_box("api.internal"),
            ))
        });
    });
    g.bench_function("wildcard_miss_no_dot", |b| {
        b.iter(|| black_box(host_allowed(black_box(&wildcard), black_box("notinternal"))));
    });
    g.bench_function("two_patterns_scan", |b| {
        b.iter(|| black_box(host_allowed(black_box(&both), black_box("api.internal"))));
    });
    g.bench_function("deny_all_empty", |b| {
        b.iter(|| black_box(host_allowed(black_box(&empty), black_box("api.stripe.com"))));
    });
    g.finish();
}

fn bench_url_host(c: &mut Criterion) {
    c.bench_function("perm_url_host_https", |b| {
        b.iter(|| black_box(url_host(black_box("https://api.stripe.com/v1/charges"))));
    });
    c.bench_function("perm_url_host_relative", |b| {
        b.iter(|| black_box(url_host(black_box("/api/charges"))));
    });
}

fn bench_fetch_path_check(c: &mut Criterion) {
    // The full fetch-path check: `url_host` + `is_net_allowed`. This is
    // the cost `op_oxide_fetch` pays per fetch (before the SSRF guard
    // and the handler).
    let grant = typical_grant();
    c.bench_function("perm_fetch_path_allowed", |b| {
        b.iter(|| {
            let host = url_host(black_box("https://api.stripe.com/v1/charges"));
            black_box(
                host.as_deref()
                    .is_some_and(|h| grant.is_net_allowed(black_box(h))),
            )
        });
    });
    c.bench_function("perm_fetch_path_denied", |b| {
        b.iter(|| {
            let host = url_host(black_box("https://evil.example/exfil"));
            black_box(
                host.as_deref()
                    .is_some_and(|h| grant.is_net_allowed(black_box(h))),
            )
        });
    });
    c.bench_function("perm_fetch_path_relative", |b| {
        b.iter(|| {
            let host = url_host(black_box("/api/charges"));
            black_box(
                host.as_deref()
                    .is_some_and(|h| grant.is_net_allowed(black_box(h))),
            )
        });
    });
}

criterion_group!(
    perm_benches,
    bench_host_allowed,
    bench_url_host,
    bench_fetch_path_check,
);
criterion_main!(perm_benches);
