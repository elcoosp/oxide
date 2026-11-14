//! Criterion bench for the §V2-F15 capture substrate — the per-request
//! cost of the recording the sampler opts a request into.
//!
//! The shapes: the secret-trio redaction digest (one SHA-256 per secret
//! header — the per-request count is the header set's size, so the
//! per-op cost is what the wiring pays), the fetch-op capture (the base64
//! encode + cap walk over a body), a loader-call capture (the input +
//! output JSON serialization the decorator does) and the record finish
//! (the full `RequestRecord` assembly with a populated loader/fetch
//! history — the per-request one-shot).

use std::hint::black_box;
use std::sync::{Arc, Mutex};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::record::{Recording, RecordingLoaderRunner, Redactor};
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner};

/// The bench body size: a representative SSR page fragment.
const BODY: &[u8] = &[0x3C; 4096];

/// The runner under the decorator: answers every load with data, so the
/// bench measures the decoration (the input + output JSON serialization),
/// not the runner.
struct NopRunner;

#[async_trait::async_trait]
impl LoaderRunner for NopRunner {
    fn name(&self) -> &'static str {
        "nop"
    }
    async fn run(&self, input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
        Ok(LoaderOutput::ok(serde_json::json!({ "ran": input.route })))
    }
}

fn bench_record(c: &mut Criterion) {
    let redactor = Redactor::new(false);

    let mut group = c.benchmark_group("record");
    group.bench_function("redact_secret_header", |b| {
        b.iter(|| {
            black_box(redactor.redact("set-cookie", "sid=abcdef0123456789; Path=/"));
        });
    });
    group.bench_function("redact_plain_header", |b| {
        b.iter(|| {
            black_box(redactor.redact("content-type", "text/html"));
        });
    });
    group.bench_function(BenchmarkId::new("record_fetch_body", BODY.len()), |b| {
        let mut rec = Recording::new(redactor.clone(), "GET", "/p");
        let headers = vec![("content-type".to_string(), "text/html".to_string())];
        b.iter(|| {
            rec.record_fetch(
                "GET",
                "https://up.example/data",
                black_box(BODY),
                200,
                &headers,
                black_box(BODY),
            );
            let n = rec.fetches().len();
            rec = Recording::new(redactor.clone(), "GET", "/p");
            black_box(n);
        });
    });
    group.bench_function("finish_populated_record", |b| {
        b.iter(|| {
            let mut rec = Recording::new(redactor.clone(), "GET", "https://app.test/p");
            rec.set_route("routes/p/[slug]", &[("slug".to_string(), "s".to_string())]);
            rec.set_locals(serde_json::json!({ "locale": "en" }));
            for i in 0..4 {
                rec.record_loader(
                    &LoaderRunInput::new(format!("/m{i}.ts"), format!("routes/m{i}"), "/p"),
                    &LoaderOutput::ok(serde_json::json!({ "i": i })),
                );
            }
            for i in 0..8 {
                rec.record_fetch(
                    "GET",
                    &format!("https://up.example/{i}"),
                    BODY,
                    200,
                    &[],
                    BODY,
                );
            }
            black_box(rec.finish("bench-req", "aaaaaaaaaaaaaaaa", 200, &[], BODY));
        });
    });
    group.finish();

    // The decorator: the per-load overhead the dispatch closure pays for
    // wrapping its runner (the JSON serialization of the input + output).
    let rec = Arc::new(Mutex::new(Recording::new(Redactor::new(false), "GET", "/")));
    let runner: Arc<dyn LoaderRunner> = Arc::new(RecordingLoaderRunner::new(
        Arc::new(NopRunner),
        Arc::clone(&rec),
    ));
    let mut group = c.benchmark_group("record_runner");
    group.bench_function("decorated_load", |b| {
        b.iter(|| {
            let input = LoaderRunInput::new("/m.ts", "routes/m", "/");
            let out = futures::executor::block_on(runner.run(input));
            black_box(out.expect("the nop runner never fails"));
        });
    });
    group.finish();
}

criterion_group!(benches, bench_record);
criterion_main!(benches);
