//! Criterion benches for the V2-F21 dev-plane trace feed (the server
//! half), `bench-util` gated like every other criterion bench — plain
//! `cargo test` never pays the criterion build.
//!
//! The wiring-relevant question: the per-request record cost every
//! completed pipeline request pays on an attached sink (the steady-state
//! eviction at capacity, traced and untraced shapes), and the per-query
//! render cost the dev feed's poll pays at the attached window size.
//!
//! Run with: `cargo bench -p oxide-fw-server --features bench-util --bench trace_feed`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_server::trace_context::TraceContext;
use oxide_fw_server::{RequestTrace, RingTraceSink, TraceSink, TracesEndpoint};

const TRACED_PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// One pre-built row (the fixture shape the pipeline records — a traced
/// GET answering 200).
fn row(i: usize) -> RequestTrace {
    let trace = TraceContext::parse_traceparent(TRACED_PARENT);
    RequestTrace::of(
        &format!("req-{i:08x}"),
        "GET",
        "/routes/index/page.tsx",
        trace.as_ref(),
        200,
        Duration::from_micros(1250 + u64::try_from(i).unwrap_or(0)),
    )
}

fn bench_ring_record(c: &mut Criterion) {
    // The steady-state record cost: a FULL ring evicts on every push —
    // the shape every completed request pays on an attached sink. The
    // traced row maps the context halves; the untraced row is the None
    // shape (the default dev session without a traceparent header).
    let mut group = c.benchmark_group("trace_sink_ring");
    for n in [64usize, 256] {
        let traced = Arc::new(RingTraceSink::new(n)) as Arc<dyn TraceSink>;
        for i in 0..n {
            traced.record(row(i));
        }
        let untraced = Arc::new(RingTraceSink::new(n)) as Arc<dyn TraceSink>;
        for i in 0..n {
            untraced.record(RequestTrace::of(
                &format!("req-{i:08x}"),
                "GET",
                "/routes/index/page.tsx",
                None,
                200,
                Duration::from_micros(1250),
            ));
        }
        group.bench_with_input(
            BenchmarkId::from_parameter(n),
            &(traced, untraced),
            |b, (t, u)| {
                b.iter(|| {
                    t.record(black_box(row(0)));
                    u.record(black_box(RequestTrace::of(
                        "req-new",
                        "GET",
                        "/routes/index/page.tsx",
                        None,
                        200,
                        Duration::from_micros(1250),
                    )));
                });
            },
        );
    }
    group.finish();
}

fn bench_snapshot(c: &mut Criterion) {
    // The per-query read cost the dev feed's poll pays: the full window
    // cloned newest-first (the endpoint renders every row on a GET).
    let mut group = c.benchmark_group("trace_feed_snapshot");
    for n in [64usize, 256] {
        let sink = Arc::new(RingTraceSink::new(n)) as Arc<dyn TraceSink>;
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
    // /__oxide/traces` pays).
    let sink = Arc::new(RingTraceSink::new(256)) as Arc<dyn TraceSink>;
    for i in 0..256 {
        sink.record(row(i));
    }
    let ep = TracesEndpoint::new(sink);
    let mut group = c.benchmark_group("trace_feed_serve");
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
    group.finish();
}

fn bench_serve_query(c: &mut Criterion) {
    // The F23 filtered window: the per-query cost of the filter walk
    // over the full snapshot (parse + match + take) — the shape a
    // consumer's `?status=5xx&limit=50` poll pays against a busy window.
    let sink = Arc::new(RingTraceSink::new(256)) as Arc<dyn TraceSink>;
    for i in 0..256 {
        sink.record(row(i));
    }
    let ep = TracesEndpoint::new(sink);
    let mut group = c.benchmark_group("trace_feed_query");
    group.bench_function("unfiltered/no-query", |b| {
        b.iter(|| {
            let resp = ep.serve_query(black_box("GET"), None);
            black_box(resp.status);
        });
    });
    group.bench_function("filtered/status-and-limit", |b| {
        b.iter(|| {
            let resp = ep.serve_query(black_box("GET"), Some("status=5xx&limit=50"));
            black_box(resp.status);
        });
    });
    group.bench_function("filtered/method-and-path", |b| {
        b.iter(|| {
            let resp = ep.serve_query(black_box("GET"), Some("method=GET&path=/routes"));
            black_box(resp.status);
        });
    });
    group.finish();
}

fn bench_traceresponse_format(c: &mut Criterion) {
    // The §20 response half (V2-F21): the `traceresponse` emission a
    // traced dispatch response pays — the format render plus the
    // request-id derivation, the shape the pipeline runs once per traced
    // response.
    let trace = TraceContext::parse_traceparent(TRACED_PARENT).expect("valid traceparent");
    let mut group = c.benchmark_group("trace_response_format");
    group.bench_function("traceresponse/derived-id", |b| {
        b.iter(|| {
            let id =
                TraceContext::response_span_id(black_box("req-11aa22bb33cc44dd55ee66ff77008899"));
            black_box(id.and_then(|id| trace.to_traceresponse(id)));
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_ring_record,
    bench_snapshot,
    bench_serve,
    bench_serve_query,
    bench_traceresponse_format
);
criterion_main!(benches);
