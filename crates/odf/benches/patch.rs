//! Criterion benches for the F06 patch substrate (`bench-util` gated like
//! the V2-B09/V2-F05 benches — plain `cargo test` never pays the criterion
//! build).
//!
//! Three groups model the live-loader rerun path (`server::live`
//! `delta_payload`): decode both fulls, `diff`, `encode_patch`, and the
//! size/op policy; plus the client-side `apply`. The 2000-row append is the
//! spec's acceptance shape (§V2-F06: "a 2,000-row list with one appended row
//! sends a patch under 5% of the full payload").

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_odf::Value;
use oxide_fw_odf::patch::{apply, base_id, diff, encode_patch};

fn rows(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|k| {
                Value::Object(
                    [
                        ("i".to_string(), Value::Number(k as f64)),
                        ("name".to_string(), Value::String(format!("row-{k}"))),
                        (
                            "body".to_string(),
                            Value::String(format!(
                                "The body text of row {k} — long enough to dominate the payload."
                            )),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect(),
    )
}

/// The full bytes of a `rows` list plus a one-row appended variant.
fn fulls(n: usize) -> (Vec<u8>, Vec<u8>, Value, Value) {
    let old = rows(n);
    let mut new = rows(n);
    if let Value::Array(r) = &mut new {
        r.push(Value::Object(
            [
                (
                    "i".to_string(),
                    Value::Number(f64::from(u32::try_from(n).unwrap_or(u32::MAX))),
                ),
                ("name".to_string(), Value::String(format!("row-{n}"))),
                (
                    "body".to_string(),
                    Value::String("the appended row".to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        ));
    }
    let old_bytes = oxide_fw_odf::encode(&old).expect("old encodes");
    let new_bytes = oxide_fw_odf::encode(&new).expect("new encodes");
    (old_bytes, new_bytes, old, new)
}

fn bench_diff_encode(c: &mut Criterion) {
    let mut g = c.benchmark_group("patch_diff_encode");
    for n in [256, 2000] {
        let (old_b, _new_b, old, new) = fulls(n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let ops = diff(black_box(&old), black_box(&new));
                let id = base_id(black_box(&old_b));
                encode_patch(&id, black_box(&ops)).expect("patch encodes")
            })
        });
    }
    g.finish();
}

fn bench_apply(c: &mut Criterion) {
    let mut g = c.benchmark_group("patch_apply");
    for n in [256, 2000] {
        let (_old_b, _new_b, old, new) = fulls(n);
        let ops = diff(&old, &new);
        assert_eq!(ops.len(), 1, "an append is one Splice");
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| apply(black_box(&old), black_box(&ops)).expect("applies"))
        });
    }
    g.finish();
}

fn bench_delta_policy(c: &mut Criterion) {
    // The full server-side policy: decode both fulls (the rerun closure
    // hands back bytes), diff, encode, and compare sizes — everything
    // `delta_payload` does per rerun.
    let mut g = c.benchmark_group("patch_delta_policy");
    for n in [256, 2000] {
        let (old_b, new_b, _old, _new) = fulls(n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let old = oxide_fw_odf::decode(black_box(&old_b)).expect("decodes");
                let new = oxide_fw_odf::decode(black_box(&new_b)).expect("decodes");
                let ops = diff(&old, &new);
                if ops.len() <= oxide_fw_odf::patch::PATCH_MAX_OPS {
                    let patch = encode_patch(&base_id(&old_b), &ops).expect("encodes");
                    if patch.len() * 2 < new_b.len() {
                        patch
                    } else {
                        new_b.clone()
                    }
                } else {
                    new_b.clone()
                }
            })
        });
    }
    g.finish();
}

criterion_group!(benches, bench_diff_encode, bench_apply, bench_delta_policy);
criterion_main!(benches);
