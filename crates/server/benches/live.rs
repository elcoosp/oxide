//! Criterion benches for the V2-F05 live substrate (`bench-util` gated
//! like the V2-A02/V2-A05/V2-A06/V2-B09 benches — plain `cargo test`
//! never pays the criterion build).
//!
//! The wiring-relevant questions:
//! - `affected`: the per-revalidation fan-out cost at a scaled hub — the
//!   revalidate path calls it once per tag hit and enqueues every
//!   returned subscriber.
//! - `subscribe_remove`: connection churn (an `EventSource` reconnect is a
//!   subscribe + a guard drop).
//! - `sse_frame`: the per-frame formatting cost every data/ping frame
//!   pays on the hot stream path.

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_server::live::{Hub, Sub, sse_frame};

/// A subscriber holding `tags` (one exact or one wildcard).
fn sub(tags: &[&str]) -> Arc<Sub> {
    Arc::new(Sub::new(
        "routes/blog/[slug]",
        "/blog/rust",
        Vec::new(),
        tags.iter().map(|t| (*t).to_string()),
        None,
    ))
}

/// A hub with `n` subscribers, each on its own exact tag `post:<i>`, plus
/// `w` wildcard subscribers on `comments:*`.
fn hub(n: usize, w: usize) -> Arc<Hub> {
    let hub = Hub::new(n + w + 1, 10_000);
    for i in 0..n {
        let s = sub(&[&format!("post:{i}")]);
        drop(hub.subscribe(s).expect("fits"));
    }
    for _ in 0..w {
        let s = sub(&["comments:*"]);
        drop(hub.subscribe(s).expect("fits"));
    }
    hub
}

fn bench_affected(c: &mut Criterion) {
    let mut group = c.benchmark_group("affected");
    // The realistic shapes: a 256-connection hub with one wildcard pool,
    // and a scaled 1024-connection hub.
    for (n, w) in [(256usize, 16usize), (1024, 16)] {
        let h = hub(n, w);
        group.bench_with_input(BenchmarkId::from_parameter(n), &(h, w), |b, (h, _w)| {
            b.iter(|| {
                // Exact-tag hit (one subscriber) + wildcard hit (the
                // wildcard pool), the two fan-out shapes.
                black_box(h.affected(black_box("post:7")));
                black_box(h.affected(black_box("comments:42")));
            });
        });
    }
    group.finish();
}

fn bench_subscribe_remove(c: &mut Criterion) {
    let mut group = c.benchmark_group("subscribe_remove");
    let n = 256usize;
    let h = hub(n, 0);
    group.bench_function("256", |b| {
        b.iter(|| {
            // One reconnect: subscribe + guard drop (unsubscribes).
            let guard = h.subscribe(sub(&["churn"])).expect("fits");
            drop(guard);
        });
    });
    group.finish();
}

fn bench_sse_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("sse_frame");
    // Typical frame shapes: the keepalive ping, one ODF snapshot line
    // (~1 KB base64), and a multi-line frame.
    group.bench_function("ping", |b| {
        b.iter(|| black_box(sse_frame("ping", None, "1")));
    });
    let snapshot: String = "aGkh".repeat(340); // ~1 KB payload
    group.bench_function("data_1k", |b| {
        b.iter(|| black_box(sse_frame("data", Some(7), black_box(&snapshot))));
    });
    group.bench_function("data_8_lines", |b| {
        let line = snapshot[..64].to_string();
        let multi = (0..8).map(|_| line.clone()).collect::<Vec<_>>().join("\n");
        b.iter(|| black_box(sse_frame("data", Some(8), black_box(&multi))));
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_affected,
    bench_subscribe_remove,
    bench_sse_frame
);
criterion_main!(benches);
