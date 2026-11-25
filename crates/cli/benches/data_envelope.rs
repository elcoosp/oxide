//! P0-2 bench: the `/__data` response-envelope bound's cost. The bound
//! check rides EVERY envelope (the under-cap groups measure the full
//! build — loaders, ODF encode, the one `usize` compare), and the two
//! refusal groups measure the loud `DSP-117` arms (the over-cap encode +
//! refuse, the encoder's own `ODF-KEY` refusal). The default cap is the
//! request `max_body` twin, so the under-cap groups are the honest
//! steady-state shape. Runs in test mode (`cargo test -p oxide-fw-cli
//! --features bench-util --benches -- --test` or `cargo bench -p
//! oxide-fw-cli --features bench-util`).
//!
//! The bench is gated behind `bench-util` (the V2-F18 pattern: plain
//! `cargo test` never pays the criterion build).

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cli::{RouteDataArgs, route_data_response};
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner, Metrics, OxideRequest};
use std::collections::BTreeMap;
use std::hint::black_box as bb;
use std::sync::Arc;

/// A loader whose page data is one big string — the envelope's payload
/// the groups size.
struct PayloadLoader {
    bytes: usize,
    reserved: bool,
}

#[async_trait::async_trait]
impl LoaderRunner for PayloadLoader {
    fn name(&self) -> &str {
        "payload"
    }
    async fn run(&self, _input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
        let data = if self.reserved {
            serde_json::json!({ "__proto__": { "polluted": true } })
        } else {
            serde_json::json!({ "blob": "x".repeat(self.bytes) })
        };
        Ok(LoaderOutput::ok(data))
    }
}

fn bench_request() -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: "/__data?route=%2Fblog".into(),
        headers: Arc::new(Vec::new()),
        header_index: Default::default(),
        body: bytes::Bytes::new(),
        request_id: "bench-envelope".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn bench_envelope(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let params: BTreeMap<String, String> = BTreeMap::new();
    let locals = serde_json::Value::Null;
    let mw_headers: Vec<(String, String)> = Vec::new();
    let default_cap = oxide_fw_server::Limits::default().max_data_envelope;

    let run = |bytes: usize, cap: usize, reserved: bool| -> oxide_fw_server::OxideResponse {
        let loader: Arc<dyn LoaderRunner> = Arc::new(PayloadLoader { bytes, reserved });
        let metrics = Arc::new(Metrics::new());
        rt.block_on(route_data_response(RouteDataArgs {
            live: false,
            req: &bench_request(),
            loader_runner: Some(&loader),
            module_path: Some(std::path::Path::new("/fixture/blog/page.server.ts")),
            route_id: Some("routes/blog/page"),
            params: &params,
            metrics: &metrics,
            started: std::time::Instant::now(),
            middleware_locals: &locals,
            middleware_headers: &mw_headers,
            layout_module_paths: &[],
            page_needs_parent: false,
            locale: "",
            session_codec: None,
            middleware_ms: 0.0,
            waterfall: None,
            max_envelope: cap,
        }))
    };

    let mut group = c.benchmark_group("data_envelope");
    group.bench_function("under_cap_1k", |b| {
        b.iter(|| {
            let resp = bb(run(1024, default_cap, false));
            bb(resp.status);
        })
    });
    group.bench_function("under_cap_64k", |b| {
        b.iter(|| {
            let resp = bb(run(64 * 1024, default_cap, false));
            bb(resp.status);
        })
    });
    group.bench_function("over_cap_refusal", |b| {
        b.iter(|| {
            let resp = bb(run(64 * 1024, 1024, false));
            bb(resp.status);
        })
    });
    group.bench_function("encode_refusal", |b| {
        b.iter(|| {
            let resp = bb(run(0, default_cap, true));
            bb(resp.status);
        })
    });
    group.finish();
}

criterion_group!(benches, bench_envelope);
criterion_main!(benches);
