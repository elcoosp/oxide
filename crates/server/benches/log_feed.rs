//! Criterion benches for the V2-F24 dev-plane log feed (the server
//! half), `bench-util` gated like every other criterion bench — plain
//! `cargo test` never pays the criterion build.
//!
//! The wiring-relevant questions: the per-event record cost the capture
//! layer pays on an attached sink (the steady-state eviction at
//! capacity — every admitted event pays it), the capture layer's own
//! per-event cost under a live subscriber (the field visitor walk), and
//! the per-query render cost the dev feed's poll pays at the attached
//! window size.
//!
//! Run with: `cargo bench -p oxide-fw-server --features bench-util --bench log_feed`

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_server::{LogEvent, LogSink, LogsEndpoint, RingLogSink};
use tracing_subscriber::layer::SubscriberExt as _;

/// One pre-built row (the fixture shape the capture layer records — an
/// info event with a field, the pipeline's common shape).
fn row(i: usize) -> LogEvent {
    LogEvent {
        ts_ms: 1_700_000_000_000 + u64::try_from(i).unwrap_or(0),
        level: "info".into(),
        target: "oxide_fw_server::trace_feed".into(),
        message: "trace feed served".into(),
        fields: vec![("rows".to_string(), "3".to_string())],
    }
}

fn bench_ring_record(c: &mut Criterion) {
    // The steady-state record cost: a FULL ring evicts on every push —
    // the shape every admitted event pays on an attached sink. The wide
    // field list is the export arm's shape (ten fields, the F22 row's
    // width); the bare row is the message-only shape.
    let mut group = c.benchmark_group("log_sink_ring");
    for n in [64usize, 256] {
        let sink = Arc::new(RingLogSink::new(n)) as Arc<dyn LogSink>;
        for i in 0..n {
            sink.record(row(i));
        }
        let bare = LogEvent {
            fields: Vec::new(),
            ..row(0)
        };
        group.bench_with_input(BenchmarkId::from_parameter(n), &sink, |b, s| {
            b.iter(|| {
                s.record(black_box(row(0)));
                s.record(black_box(bare.clone()));
            });
        });
    }
    group.finish();
}

fn bench_capture_layer(c: &mut Criterion) {
    // The capture arm's per-event cost under a live subscriber: the
    // event walk (fields visited, the row built, the ring record) —
    // the shape every admitted event pays while the layer is installed.
    let sink = Arc::new(RingLogSink::new(256)) as Arc<dyn LogSink>;
    let layer = oxide_fw_server::CaptureLayer::new(sink.clone());
    for i in 0..256 {
        sink.record(row(i));
    }
    let mut group = c.benchmark_group("log_feed_capture");
    group.bench_function("event/one-field-info", |b| {
        b.iter(|| {
            // A scoped subscriber so the bench event itself is captured
            // through the layer (the dev boot's install shape).
            tracing::subscriber::with_default(
                tracing_subscriber::registry().with(layer.clone()),
                || {
                    tracing::info!(target: "oxide_fw_server::bench", rows = 3, "bench event");
                },
            );
            black_box(sink.snapshot().len());
        });
    });
    group.finish();
}

fn bench_snapshot(c: &mut Criterion) {
    // The per-query read cost the dev feed's poll pays: the full window
    // cloned newest-first (the endpoint renders every row on a GET).
    let mut group = c.benchmark_group("log_feed_snapshot");
    for n in [64usize, 256] {
        let sink = Arc::new(RingLogSink::new(n)) as Arc<dyn LogSink>;
        for i in 0..n {
            sink.record(row(i));
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &sink, |b, s| {
            b.iter(|| {
                black_box(s.snapshot().len());
            });
        });
    }
    group.finish();
}

fn bench_serve(c: &mut Criterion) {
    // The endpoint's total request → response function at the full
    // window: the snapshot + the JSON render (the wire cost a `GET
    // /__oxide/logs` pays).
    let sink = Arc::new(RingLogSink::new(256)) as Arc<dyn LogSink>;
    for i in 0..256 {
        sink.record(row(i));
    }
    let ep = LogsEndpoint::new(sink);
    let mut group = c.benchmark_group("log_feed_serve");
    group.bench_function("get/full-window", |b| {
        b.iter(|| {
            let resp = ep.serve(black_box("GET"));
            black_box(resp.status);
        });
    });
    group.bench_function("refused/non-get", |b| {
        b.iter(|| {
            let resp = ep.serve(black_box("POST"));
            black_box(resp.status);
        });
    });
    group.bench_function("filtered/level-and-limit", |b| {
        b.iter(|| {
            let resp = ep.serve_query(black_box("GET"), Some("level=warn,error&limit=50"));
            black_box(resp.status);
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_ring_record,
    bench_capture_layer,
    bench_snapshot,
    bench_serve
);
criterion_main!(benches);
