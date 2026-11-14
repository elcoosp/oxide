//! Criterion bench for the §V2-F15 playback substrate — the per-replay
//! cost of the seams the worker and `oxide replay` drive.
//!
//! The shapes: the ordered fetch matcher (the take that must compare
//! method, URL and the decoded recorded body — one op and a deep queue),
//! the nonce normalization (the §V2-F15 "--diff normalizing the nonce"
//! walk over a representative page) and the full `replay_diff` verdict
//! over a populated record.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::record::{Recording, Redactor};
use oxide_fw_dispatch::replay::{ReplayMatcher, normalize_nonce, replay_diff};

const BODY: &[u8] = &[0x3C; 4096];

/// A record with `ops` fetch ops and a captured response, the fixture
/// every group replays.
fn populated_record(ops: usize) -> oxide_fw_dispatch::record::RequestRecord {
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
    rec.finish("bench-req", "aaaaaaaaaaaaaaaa", 200, &[], BODY)
}

fn bench_replay(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay");

    // The happy take: the next op matches and is consumed.
    let record = populated_record(1);
    group.bench_function("take_next_match", |b| {
        b.iter_batched(
            || ReplayMatcher::from_record(&record),
            |mut m| {
                let out = m.take("GET", "https://up.example/0", b"");
                black_box(out.expect("the matching take plays"));
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // The mismatch take: the deepest op is expected, the caller calls the
    // first URL — the compare runs to the end of the identity check.
    let deep = populated_record(64);
    group.bench_function(
        BenchmarkId::new("take_mismatch_queue", deep.fetches.len()),
        |b| {
            b.iter_batched(
                || ReplayMatcher::from_record(&deep),
                |mut m| {
                    let out = m.take("GET", "https://up.example/0", b"other");
                    black_box(out.is_err());
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );

    // The nonce normalization walk over a page with several nonce
    // attributes.
    let page = format!(
        "<html><head><script nonce=\"{}\"></script></head><body>\
         <script nonce=\"{}\">1</script><div nonce=\"short\">x</div></body></html>",
        "a".repeat(32),
        "b".repeat(32)
    );
    group.bench_function(BenchmarkId::new("normalize_nonce", page.len()), |b| {
        b.iter(|| {
            black_box(normalize_nonce(black_box(&page)));
        });
    });

    // The full verdict: status + headers + body over the populated record.
    group.bench_function("replay_diff_full", |b| {
        let recorded_body = String::from_utf8_lossy(BODY).to_string();
        b.iter(|| {
            black_box(replay_diff(
                black_box(&record),
                200,
                &[("content-type".to_string(), "text/html".to_string())],
                &recorded_body,
            ));
        });
    });
    group.finish();
}

criterion_group!(benches, bench_replay);
criterion_main!(benches);
