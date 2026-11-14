//! Criterion bench for the §V2-F15 replay-RUNNER seam — the per-replay
//! cost of the one-definition orchestration the `oxide replay` command
//! and the MCP `replay.run` tool drive.
//!
//! The shapes: the full runner path (record-store load → the `RPL-003`
//! stale-build verdict → the injected executor → the `replay_diff`
//! verdict) against an in-memory store, at one fetch op and a deep
//! queue; and the store load alone over the filesystem impl (the
//! command's first step, the temp+rename write's read counterpart).

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::record::{DirRecordStore, RecordSink, Recording, Redactor};
use oxide_fw_dispatch::replay::{ExecutedResponse, ReplayExecutor, ReplayRunner};

const BODY: &[u8] = &[0x3C; 4096];

/// An in-memory store over one record (the runner's read seam behind a
/// map, the same shape the MCP surface would inject).
struct MemoryStore(oxide_fw_dispatch::record::RequestRecord);

impl oxide_fw_dispatch::record::RecordSource for MemoryStore {
    fn load(
        &self,
        _request_id: &str,
    ) -> Result<oxide_fw_dispatch::record::RequestRecord, oxide_fw_diagnostics::OxideError> {
        Ok(self.0.clone())
    }
}

/// The faithful re-execution host: hands back the recorded response
/// (the shape a byte-identical replay produces).
struct FaithfulExecutor(ExecutedResponse);

#[async_trait::async_trait]
impl ReplayExecutor for FaithfulExecutor {
    async fn execute(
        &self,
        _record: &oxide_fw_dispatch::record::RequestRecord,
    ) -> Result<ExecutedResponse, oxide_fw_diagnostics::OxideError> {
        Ok(self.0.clone())
    }
}

/// A record with `ops` fetch ops and a captured response, stored on disk
/// under `root` (the write side warms the fixture the read side benches).
fn store_record(root: &std::path::Path, ops: usize) -> oxide_fw_dispatch::record::RequestRecord {
    let mut rec = Recording::new(Redactor::new(false), "GET", "https://app.test/p");
    for i in 0..ops {
        rec.record_fetch(
            "GET",
            &format!("https://up.example/{i}"),
            b"",
            200,
            &[("content-type".to_string(), "application/json".to_string())],
            BODY,
        );
    }
    let record = rec.finish("bench-req", "aaaaaaaaaaaaaaaa", 200, &[], BODY);
    let store = DirRecordStore::new(root);
    RecordSink::write(&store, &record).expect("the fixture record writes");
    record
}

fn bench_replay_runner(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay_runner");
    let rt = tokio::runtime::Runtime::new().expect("bench tokio runtime");
    let tmp = tempfile::tempdir().expect("the bench temp root");

    for ops in [1, 8] {
        let record = store_record(tmp.path(), ops);
        let runner = ReplayRunner::new(Arc::new(MemoryStore(record.clone())), "aaaaaaaaaaaaaaaa");
        let executor = FaithfulExecutor(ExecutedResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "text/html".to_string())],
            body: BODY.to_vec(),
        });
        group.bench_with_input(BenchmarkId::new("run", ops), &ops, |b, _| {
            b.iter(|| {
                let outcome = rt
                    .block_on(runner.run("bench-req", &executor))
                    .expect("the faithful replay succeeds");
                black_box(outcome.diff.ok);
            });
        });
    }

    // The store load alone (the command's first step): the filesystem
    // read + JSON parse + the version gate.
    let store = DirRecordStore::new(tmp.path());
    group.bench_function("load_fs", |b| {
        b.iter(|| {
            let record = oxide_fw_dispatch::record::RecordSource::load(&store, "bench-req")
                .expect("the fixture record loads");
            black_box(record.request_id);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_replay_runner);
criterion_main!(benches);
