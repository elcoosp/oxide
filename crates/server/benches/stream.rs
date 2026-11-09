//! Criterion benches for the V2-F07 resumable-streams substrate
//! (`bench-util` gated like the V2-A02/V2-A05/V2-B09/V2-F05 benches —
//! plain `cargo test` never pays the criterion build).
//!
//! The wiring-relevant questions:
//! - `append`: the per-chunk tee cost every streamed response pays on the
//!   hot path (lock, push, atomic add, watch send).
//! - `read_from`: the reattach backlog copy at the offsets a real client
//!   presents (0 = full replay, mid-stream, the exact end).
//! - `store_get`: the reattach lookup (map hit + TTL check) per reattach.
//! - `stream_id`: the id mint per resumable response.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_server::stream_store::{StreamStore, read_from, stream_id};

/// A store with generous limits (bench shapes do not want eviction or
/// overflow interference) and a buffer holding `n` 1 KB chunks.
fn seeded(
    n: usize,
) -> (
    Arc<StreamStore>,
    Arc<oxide_fw_server::stream_store::StreamBuf>,
) {
    let store = Arc::new(StreamStore::new(
        Duration::from_secs(3600),
        1 << 30,
        1 << 40,
    ));
    let buf = store.begin("bench".into());
    let chunk = vec![7u8; 1024];
    for _ in 0..n {
        store.append(&buf, Bytes::from(chunk.clone()));
    }
    (store, buf)
}

fn bench_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("append");
    // The realistic tee shapes: small SSE-ish chunks and 8 KB body chunks.
    for size in [256usize, 8192] {
        let (store, buf) = seeded(0);
        let chunk = Bytes::from(vec![7u8; size]);
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &(store, buf, chunk),
            |b, (s, buf, chunk)| {
                b.iter(|| {
                    s.append(buf, chunk.clone());
                });
            },
        );
    }
    group.finish();
}

fn bench_read_from(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_from");
    let (_store, buf) = seeded(1024); // 1 MiB buffered
    let total = buf.len_now();
    for (name, offset) in [("full", 0u64), ("mid", total / 2), ("tail", total)] {
        group.bench_function(name, |b| {
            b.iter(|| black_box(read_from(black_box(&buf), black_box(offset))));
        });
    }
    group.finish();
}

fn bench_store_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_get");
    let (store, _buf) = seeded(0);
    group.bench_function("hit", |b| {
        b.iter(|| black_box(store.get(black_box("bench"))));
    });
    group.bench_function("miss", |b| {
        b.iter(|| black_box(store.get(black_box("absent"))));
    });
    group.finish();
}

fn bench_stream_id(c: &mut Criterion) {
    let mut group = c.benchmark_group("stream_id");
    group.bench_function("mint", |b| {
        b.iter(|| black_box(stream_id(black_box("req-abc123"), black_box(7))));
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_append,
    bench_read_from,
    bench_store_get,
    bench_stream_id
);
criterion_main!(benches);
