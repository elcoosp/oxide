//! Criterion benchmarks for the §20 trace-parent surface (`crates/jshost`).
//!
//! Cases: the extraction over a `CallInput` (headers-map hit, payload-trace
//! hit, raw-pair fallback, untraced miss — the scan the worker loop pays
//! once per call), the guard cycle (seed + drop — the RAII cost the worker
//! loop pays), and the `fetch` op's append rule (slot hit vs site-set).
//!
//! Run: `cargo bench -p oxide-fw-jshost --features bench-util`

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_jshost::CallInput;
use oxide_fw_jshost::v8_ops::{
    TraceParentGuard, propagate_trace_parent, set_trace_parent, trace_parent_from_call_input,
};
use std::collections::BTreeMap;
use std::hint::black_box;

const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

/// A `CallInput` with `n` uninteresting headers plus the interesting one.
fn input_with(n: usize, traced: bool) -> CallInput {
    let mut headers = BTreeMap::new();
    for i in 0..n {
        headers.insert(format!("x-pad-{i}"), "v".repeat(16));
    }
    if traced {
        headers.insert("traceparent".to_string(), VALID.to_string());
    }
    CallInput {
        payload: serde_json::json!({"route": "/r"}),
        headers,
        has_body: false,
        request_id: "bench".to_string(),
        route: None,
    }
}

fn bench_extraction(c: &mut Criterion) {
    let mut g = c.benchmark_group("trace_parent_extraction");
    for n in [4usize, 32] {
        let traced = input_with(n, true);
        g.bench_with_input(format!("headers_map/{n}_pad"), &traced, |b, i| {
            b.iter(|| trace_parent_from_call_input(black_box(i)))
        });
        let untraced = input_with(n, false);
        g.bench_with_input(format!("untraced/{n}_pad"), &untraced, |b, i| {
            b.iter(|| trace_parent_from_call_input(black_box(i)))
        });
    }
    let payload_traced = CallInput {
        payload: serde_json::json!({"trace": {"traceparent": VALID}}),
        headers: BTreeMap::new(),
        has_body: false,
        request_id: "bench".to_string(),
        route: None,
    };
    g.bench_function("payload_trace", |b| {
        b.iter(|| trace_parent_from_call_input(black_box(&payload_traced)))
    });
    g.finish();
}

fn bench_guard_cycle(c: &mut Criterion) {
    let traced = input_with(8, true);
    let mut g = c.benchmark_group("trace_parent_guard");
    g.bench_function("for_call_seed_and_drop", |b| {
        b.iter(|| {
            let guard = TraceParentGuard::for_call(black_box(&traced));
            drop(guard);
        })
    });
    g.bench_function("untraced_clear", |b| {
        b.iter(|| {
            let guard = TraceParentGuard::for_call(black_box(&CallInput {
                payload: serde_json::json!({}),
                headers: BTreeMap::new(),
                has_body: false,
                request_id: "bench".to_string(),
                route: None,
            }));
            drop(guard);
        })
    });
    g.finish();
}

fn bench_propagate(c: &mut Criterion) {
    let mut g = c.benchmark_group("trace_parent_propagate");
    g.bench_function("slot_hit/4_site_headers", |b| {
        b.iter(|| {
            set_trace_parent(Some(VALID.to_string()));
            let mut headers: Vec<(String, String)> =
                (0..4).map(|i| (format!("x-h-{i}"), "v".into())).collect();
            propagate_trace_parent(black_box(&mut headers));
            set_trace_parent(None);
        })
    });
    g.bench_function("site_set_wins/4_site_headers", |b| {
        b.iter(|| {
            set_trace_parent(Some(VALID.to_string()));
            let mut headers: Vec<(String, String)> =
                (0..4).map(|i| (format!("x-h-{i}"), "v".into())).collect();
            headers.push(("traceparent".to_string(), VALID.to_string()));
            propagate_trace_parent(black_box(&mut headers));
            set_trace_parent(None);
        })
    });
    g.bench_function("no_slot/4_site_headers", |b| {
        b.iter(|| {
            set_trace_parent(None);
            let mut headers: Vec<(String, String)> =
                (0..4).map(|i| (format!("x-h-{i}"), "v".into())).collect();
            propagate_trace_parent(black_box(&mut headers));
        })
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_extraction,
    bench_guard_cycle,
    bench_propagate
);
criterion_main!(benches);
