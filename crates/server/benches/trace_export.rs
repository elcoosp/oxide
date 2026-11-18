//! Criterion benches for the trace rows' tracing export (the server
//! half of V2-F22), `bench-util` gated like every other criterion bench
//! — plain `cargo test` never pays the criterion build.
//!
//! The wiring-relevant question: the per-record cost a dev session pays
//! on the export arm once `OXIDE_TRACE_LOG` attaches it — the event emit
//! under a live subscriber (field encode + dispatch, the shape the
//! capture pins render), the no-op branch when no subscriber is
//! installed, and the dev-boot fanout (ring + tracing) record against a
//! full ring.
//!
//! The subscriber is the capture layer the `trace_export_tdd.rs` pins
//! use (fields visited into a bounded Vec — a fixed-format writer is
//! stdout-bound and would measure the terminal, not the seam). The
//! numbers are informational (the benchmarks/report.md
//! deterministic-vs-informational split): the export arm is dev-only
//! and opt-in, so no release profile pays it.
//!
//! Run with: `cargo bench -p oxide-fw-server --features bench-util --bench trace_export`

use std::hint::black_box;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_server::{FanoutTraceSink, RequestTrace, RingTraceSink, TraceSink, TracingTraceSink};
use tracing_subscriber::layer::SubscriberExt as _;

/// One pre-built row (the fixture shape the pipeline records — a traced
/// GET answering 200).
fn row(i: usize) -> RequestTrace {
    RequestTrace::of(
        &format!("req-{i:08x}"),
        "GET",
        "/routes/index/page.tsx",
        None,
        200,
        Duration::from_micros(1250 + u64::try_from(i).unwrap_or(0)),
    )
}

/// The capture layer: fields visited into a bounded Vec (the same shape
/// the pins assert on — a subscriber that does real field work, minus
/// the terminal).
#[derive(Default)]
struct SinkholeLayer {
    events: Mutex<Vec<(String, String)>>,
}

impl<S: tracing::Subscriber> tracing_subscriber::layer::Layer<S> for SinkholeLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut fields: Vec<(String, String)> = Vec::new();
        event.record(&mut SinkholeVisitor(&mut fields));
        let mut events = self.events.lock().expect("sinkhole poisoned");
        events.push((
            event.metadata().target().to_string(),
            fields.len().to_string(),
        ));
        events.truncate(1024);
    }
}

/// The field visitor: every value lands as its rendered string.
struct SinkholeVisitor<'a>(&'a mut Vec<(String, String)>);

impl tracing::field::Visit for SinkholeVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .push((field.name().to_string(), format!("{value:?}")));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.push((field.name().to_string(), value.to_string()));
    }
}

fn bench_export_record(c: &mut Criterion) {
    // The per-record emit cost the export arm pays once attached: one
    // `tracing::info!` event with the row's nine fields. `live-subscriber`
    // runs under the capture layer (fields encoded, dispatch walked) —
    // the shape a real subscriber starts from; `no-subscriber` is the
    // no-op branch (no global default installed).
    let mut group = c.benchmark_group("trace_export_record");
    let sink = TracingTraceSink::new();
    group.bench_function("info/live-subscriber", |b| {
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(SinkholeLayer::default()),
        );
        b.iter(|| {
            sink.record(black_box(row(0)));
        });
    });
    group.bench_function("disabled/no-subscriber", |b| {
        b.iter(|| {
            sink.record(black_box(row(0)));
        });
    });
    group.finish();
}

fn bench_fanout_record(c: &mut Criterion) {
    // The dev-boot attach shape: the pipeline records into
    // FanoutTraceSink(ring, tracing) — the full 256-row ring's eviction
    // push AND the export emit in one record (what every completed
    // request pays with `OXIDE_TRACE_LOG=1` and the default window).
    let mut group = c.benchmark_group("trace_export_fanout");
    for n in [64usize, 256] {
        let ring = Arc::new(RingTraceSink::new(n)) as Arc<dyn TraceSink>;
        for i in 0..n {
            ring.record(row(i));
        }
        let export = Arc::new(TracingTraceSink::new()) as Arc<dyn TraceSink>;
        let fanout = Arc::new(FanoutTraceSink::new(vec![ring, export])) as Arc<dyn TraceSink>;
        group.bench_with_input(BenchmarkId::from_parameter(n), &fanout, |b, f| {
            let _guard = tracing::subscriber::set_default(
                tracing_subscriber::registry().with(SinkholeLayer::default()),
            );
            b.iter(|| {
                f.record(black_box(row(0)));
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_export_record, bench_fanout_record);
criterion_main!(benches);
