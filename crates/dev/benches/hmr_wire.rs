//! Criterion bench for the §13.3 message-schema v1 wire (the B09 pattern:
//! plain `cargo test` never pays the criterion build — run behind
//! `--features bench-util`).
//!
//! The wiring-relevant question is the per-watcher-tick cost of the frame
//! encode/decode the `cmd_dev` fan-out and the `/_oxide/hmr` endpoint run
//! on every publish, across the batch shapes a real dev session produces:
//!
//! - `encode/batch_{1,16,64}`: `Update` frames with N entries (the §3.2
//!   coalescing tail; a scripted edit burst lands at the high end) —
//!   `to_json` includes the envelope insert.
//! - `decode/batch_{1,16,64}`: the same frames through `from_json` — the
//!   v1 gate reads the version before the body deserializes.
//! - `encode_full_reload` / `encode_ox_data`: the two non-entry frames
//!   (the control: no per-entry cost).
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-dev --features bench-util
//! ```

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dev::{HmrMessage, UpdateEntry, UpdateKind};

/// One v1 entry shaped like the fan-out's: a file path, a dev URL, a
/// publish stamp and a 16-hex content hash.
fn entry(i: usize) -> UpdateEntry {
    UpdateEntry {
        kind: UpdateKind::Js,
        path: format!("/abs/src/comp/mod_{i}.tsx"),
        accepted_path: format!("/src/comp/mod_{i}.tsx"),
        timestamp: 1_760_000_000_000,
        hash: "0123456789abcdef".to_string(),
    }
}

fn update_frame(n: usize) -> HmrMessage {
    HmrMessage::Update {
        updates: (0..n).map(entry).collect(),
        preserve: oxide_fw_dev::Preservation::default(),
    }
}

fn bench_wire(c: &mut Criterion) {
    let mut group = c.benchmark_group("wire_encode");
    for n in [1usize, 16, 64] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let frame = update_frame(n);
            b.iter(|| frame.to_json());
        });
    }
    group.finish();

    let mut group = c.benchmark_group("wire_decode");
    for n in [1usize, 16, 64] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let json = update_frame(n).to_json();
            b.iter(|| HmrMessage::from_json(&json));
        });
    }
    group.finish();

    c.bench_function("encode_full_reload", |b| {
        let frame = HmrMessage::FullReload {
            reason: "src/routes/layout.tsx has no HMR boundary".to_string(),
        };
        b.iter(|| frame.to_json());
    });

    c.bench_function("encode_ox_data", |b| {
        let frame = HmrMessage::OxData {
            route: "/slow".to_string(),
            data: "T0RGMQ==".to_string(),
        };
        b.iter(|| frame.to_json());
    });
}

criterion_group!(benches, bench_wire);
criterion_main!(benches);
