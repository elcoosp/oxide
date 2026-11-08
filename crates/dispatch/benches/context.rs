//! V2-A02: criterion benchmarks for the dispatch substrate (`crates/dispatch`).
//!
//! Two groups:
//! - `url/*`: the prelude's per-request URL helpers (`path_of`,
//!   `normalize_path`, `data_route_of`, `parse_form_urlencoded`,
//!   `action_from_query`) on representative inputs. These set the baseline
//!   the phase-3 closure adoption is compared against.
//! - `ctx_table/*`: `RouteContextTable` build and `get` — the boot-time
//!   dispatch answers `stage_match` will consume through this crate's
//!   dependency surface (V2-B01, integration deferred per
//!   `docs/deviations.md`).
//!
//! Run: `cargo bench -p oxide-fw-dispatch --features bench-util`

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::url::{
    action_from_query, data_route_of, normalize_path, parse_form_urlencoded, path_of,
};
use oxide_fw_router::RouteContextTable;
use oxide_fw_router::testing::synthetic_entries;
use std::hint::black_box;
use std::time::Duration;

/// Page path + encoded data route + fragment: one representative URL that
/// exercises every `?`/`#` branch of the helpers.
const URL: &str = "/blog/a/posts?route=%2Fblog%2Fa&id=42#frag";

/// A §9.3 form body: two plain pairs, one `+`-encoded, one percent-encoded,
/// one bare key (the malformed-pair branch).
const FORM_BODY: &[u8] = b"name=elmo&value=42&tag=caf%C3%A9&flag";

fn bench_url_helpers(c: &mut Criterion) {
    let mut g = c.benchmark_group("url");
    g.sample_size(40);
    g.measurement_time(Duration::from_secs(4));
    g.bench_function("path_of", |b| {
        b.iter(|| path_of(black_box(URL)));
    });
    g.bench_function("normalize_path", |b| {
        b.iter(|| normalize_path(black_box("/blog/a/posts/")));
    });
    g.bench_function("data_route_of", |b| {
        b.iter(|| data_route_of(black_box("GET"), black_box("/__data"), black_box(URL)));
    });
    g.bench_function("action_from_query", |b| {
        b.iter(|| action_from_query(black_box("/comment?/setcookie#frag")));
    });
    g.bench_function("parse_form_urlencoded", |b| {
        b.iter(|| parse_form_urlencoded(black_box(FORM_BODY)));
    });
    g.finish();
}

/// Route id of synthetic entry 7 (`routes/s7/p7/page`): a static page, a
/// guaranteed hit for every n >= 8.
const CTX_HIT_ID: &str = "routes/s7/p7/page";

fn bench_ctx_table(c: &mut Criterion) {
    let mut g = c.benchmark_group("ctx_table");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(4));
    g.bench_function("build/1000", |b| {
        b.iter_batched(
            || synthetic_entries(1000),
            |entries| RouteContextTable::build_with_default_pattern(black_box(&entries)),
            criterion::BatchSize::SmallInput,
        );
    });
    for n in [100, 1000] {
        g.bench_with_input(BenchmarkId::new("get", n), &n, |b, &n| {
            let entries = synthetic_entries(n);
            let table = RouteContextTable::build_with_default_pattern(&entries);
            b.iter(|| {
                // One hit and one miss per iteration: the lookup the
                // `stage_match` integration will pay per request.
                let hit = table.get(black_box(CTX_HIT_ID)).is_some();
                let miss = table.get(black_box("routes/zzz/nothing/page")).is_none();
                hit && miss
            });
        });
    }
    g.finish();
}

criterion_group!(benches, bench_url_helpers, bench_ctx_table);
criterion_main!(benches);
