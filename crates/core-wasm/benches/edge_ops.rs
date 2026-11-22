//! V2-E09 criterion bench over the new pure edge exports (match/rules/patch).
//!
//! A cheap bench over the host fns the wasm32 Worker reuses through the
//! `#[wasm_bindgen]` bindings — `match_route`, `apply_rules`, `odp1_apply`.
//! Run with `cargo bench -p oxide-fw-core-wasm`.

use std::collections::BTreeMap;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_core_wasm::{apply_rules, match_route, odp1_apply};
use oxide_fw_odf::Value;
use oxide_fw_odf::patch::{diff, encode_patch};
use std::hint::black_box;

fn obj(pairs: &[(&str, Value)]) -> Value {
    let mut m = BTreeMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    Value::Object(m)
}

const ROUTES: &str = r#"[
    {"id":"routes/page","pattern":"/","kind":"page"},
    {"id":"routes/blog/page","pattern":"/blog","kind":"page"},
    {"id":"routes/blog/[slug]/page","pattern":"/blog/:slug","kind":"page"},
    {"id":"routes/files/[...rest]/page","pattern":"/files/*rest","kind":"page"}
]"#;

const RULES: &str = r#"[
    {"source":"/old","destination":"/new"},
    {"source":"/blog/:slug","destination":"/posts/:slug"},
    {"source":"/shop/:item","type":"rewrite","destination":"/products/:item"},
    {"source":"/legacy/*","headers":{"x-frame":"deny"}}
]"#;

fn bench_match_route(c: &mut Criterion) {
    c.bench_function("edge_match_route", |b| {
        b.iter(|| {
            match_route(black_box(ROUTES), black_box("/blog/hello")).unwrap();
        });
    });
}

fn bench_apply_rules(c: &mut Criterion) {
    c.bench_function("edge_apply_rules", |b| {
        b.iter(|| {
            apply_rules(black_box(RULES), black_box("/blog/rust"), black_box("en")).unwrap();
        });
    });
}

fn bench_odp1_apply(c: &mut Criterion) {
    // A representative patch: one object Set + one array Splice.
    let base = obj(&[
        ("title", Value::String("v1".to_string())),
        ("tags", Value::Array(vec![Value::String("a".to_string())])),
    ]);
    let new = obj(&[
        ("title", Value::String("v2".to_string())),
        (
            "tags",
            Value::Array(vec![
                Value::String("a".to_string()),
                Value::String("b".to_string()),
            ]),
        ),
    ]);
    let ops = diff(&base, &new);
    let patch = encode_patch("base0", &ops).unwrap();
    let base_json = serde_json::to_string(&base.to_json()).unwrap();
    c.bench_function("edge_odp1_apply", |b| {
        b.iter(|| {
            odp1_apply(black_box(base_json.as_str()), black_box(patch.as_slice())).unwrap();
        });
    });
}

criterion_group!(
    benches,
    bench_match_route,
    bench_apply_rules,
    bench_odp1_apply
);
criterion_main!(benches);
