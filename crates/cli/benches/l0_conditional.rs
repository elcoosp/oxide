//! V2-F55: the L0 conditional seam's decision-cost bench — the
//! `CachedResponse::serve_conditional` call the lookup stage answers
//! every conditional request with (the canonical `serve_entry`
//! composition: the RFC 9110 §13 comparison and the §15.4.5 validator
//! set over the stored entry). The arms cover the three shapes a re-check
//! can arrive in — the echoed `If-None-Match` (the 304 arm), the
//! `If-Modified-Since`-only re-check (the F55 upgrade) and the
//! non-matching validator that serves the entry in full — so the cost a
//! hit pays (and the hit path's parity with the pre-F55 hand-rolled
//! match) is measured on this machine, not assumed.
//!
//! The entry rides the store path's own builder (`L0Cache::make_entry` —
//! the sentineled body, the generated Last-Modified, the RFC-shaped weak
//! etag) wrapped in the hit envelope, so the measured cost is the
//! decision, not the store.
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! l0_conditional` (the bench-util opt-in keeps plain `cargo test` off
//! criterion).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use oxide_fw_cli::{CachedResponse, L0Cache, NONCE_SENTINEL};
use std::sync::Arc;
use std::time::Duration;

fn stored_hit() -> CachedResponse {
    let tier: Arc<dyn oxide_fw_cache::CacheHandler> =
        Arc::new(oxide_fw_cache::TieredCache::new(16 * 1024 * 1024));
    let l0 = L0Cache::new(tier, "bench-build", Duration::from_secs(300));
    let body = format!("<html>{NONCE_SENTINEL}</html>").into_bytes();
    let entry = Arc::new(l0.make_entry(
        200,
        vec![("content-type".to_string(), "text/html".to_string())],
        body,
        vec![],
    ));
    CachedResponse {
        entry,
        stale: false,
    }
}

fn bench_l0_conditional(c: &mut Criterion) {
    let hit = stored_hit();
    let etag = hit.etag().to_string();
    let lm = hit
        .entry
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("last-modified"))
        .map(|(_, v)| v.clone())
        .expect("make_entry generated a Last-Modified");

    let mut group = c.benchmark_group("l0_conditional");
    // The echoed If-None-Match: the 304 arm, the validator set composed.
    group.bench_function("inm_echo_not_modified", |b| {
        b.iter(|| {
            black_box(hit.serve_conditional(
                black_box(&[("if-none-match".to_string(), etag.clone())]),
                black_box(0),
            ))
        });
    });
    // The IMS-only re-check: the F55 upgrade's arm.
    group.bench_function("ims_only_not_modified", |b| {
        b.iter(|| {
            black_box(hit.serve_conditional(
                black_box(&[("if-modified-since".to_string(), lm.clone())]),
                black_box(0),
            ))
        });
    });
    // The non-matching validator: the Full arm (stored headers + the
    // generated gaps composed beside the stored representation).
    group.bench_function("inm_mismatch_full", |b| {
        b.iter(|| {
            black_box(hit.serve_conditional(
                black_box(&[("if-none-match".to_string(), "\"nope\"".to_string())]),
                black_box(0),
            ))
        });
    });
    group.finish();
}

criterion_group!(benches, bench_l0_conditional);
criterion_main!(benches);
