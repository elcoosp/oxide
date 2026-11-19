//! V2-F42: the boot flag's window resolver bench — the
//! `--cache-hot-key-top` override slot and the §20 parse core the boot
//! resolves ONCE at the `set_cache_plane` wire (the `trace_file_path`
//! posture: a boot-time read, not a per-scrape env poll). The arms cover
//! the resolver's three shapes — the default read (no flag, no env), the
//! env spelling's parse core (falsy, operator, the ledger-bound clamp)
//! and the flag's override read — so the resolve cost a boot pays (and a
//! windowed consumer re-pays per read) is measured on this machine, not
//! assumed.
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! hot_key_top` (the bench-util opt-in keeps plain `cargo test` off
//! criterion).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use oxide_fw_cli::observability::{
    cache_hot_key_top, cache_hot_key_top_from, set_cache_hot_key_top_override,
};

/// The parse core over the spellings a boot actually resolves: the unset
/// spelling (the default answer), the falsy spelling (the default again),
/// the operator's value (the parse rides verbatim) and the clamp (the
/// ledger's own bound).
fn bench_cache_hot_key_top_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_hot_key_top_parse");
    group.bench_function("unset", |b| {
        b.iter(|| black_box(cache_hot_key_top_from(black_box(None))));
    });
    group.bench_function("falsy_off", |b| {
        b.iter(|| black_box(cache_hot_key_top_from(black_box(Some("off")))));
    });
    group.bench_function("operator_50", |b| {
        b.iter(|| black_box(cache_hot_key_top_from(black_box(Some("50")))));
    });
    group.bench_function("clamp_cap", |b| {
        b.iter(|| black_box(cache_hot_key_top_from(black_box(Some("999999")))));
    });
    group.finish();
}

/// The override read: the flag's slot wins ahead of the env read (the
/// `trace_log_enabled` shape). The flag-set arm measures the slot hit;
/// the flag-absent arm measures the miss plus the env read the F41 shape
/// carries.
fn bench_cache_hot_key_top_override(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_hot_key_top_override");
    set_cache_hot_key_top_override(Some(50));
    group.bench_function("flag_set", |b| {
        b.iter(|| black_box(cache_hot_key_top()));
    });
    set_cache_hot_key_top_override(None);
    group.bench_function("flag_absent", |b| {
        b.iter(|| black_box(cache_hot_key_top()));
    });
    group.finish();
}

/// The V2-F46 coverage arm's marginal cost over a FULL-capacity ledger:
/// the boot's composition (`cache_plane_scrape` — the exporter's
/// families, then the labeled window, then — when the operator opted in —
/// the F44 coverage exposition) measured at both arms. The `off` arm is
/// the byte-identical default shape; the `on` arm adds the
/// `render_coverage_prometheus` pass (one lock pass over the ledger, ~9
/// fixed rows — the occupancy/bound gauges and the unattributed score's
/// counter a scrape diffs across windows).
fn bench_cache_plane_coverage_scrape(c: &mut Criterion) {
    use oxide_fw_cache::{
        CacheEvent, CacheObserver, CacheOp, HotKeyObserver, MetricsCacheObserver,
    };

    // The boot's ledger shape: the F40 capacity, driven to FULL
    // occupancy (first-sight admission over the rotation pool) so the
    // coverage gauges render the bound's real posture, not an empty
    // window.
    const BENCH_LEDGER_CAPACITY: usize = 2048;
    let keys: Vec<String> = (0..BENCH_LEDGER_CAPACITY)
        .map(|i| format!("route:/{i:04}"))
        .collect();
    let exporter = MetricsCacheObserver::new();
    let hot = HotKeyObserver::bounded(BENCH_LEDGER_CAPACITY);
    for key in &keys {
        let event = CacheEvent {
            key,
            op: CacheOp::Set { bytes: 512 },
        };
        hot.on_event(&event);
    }

    let mut group = c.benchmark_group("cache_plane_coverage_scrape");
    group.bench_function("off_default_shape", |b| {
        b.iter(|| {
            black_box(black_box(&exporter).render_prometheus());
            black_box(black_box(&hot).render_prometheus(black_box(10)));
        })
    });
    group.bench_function("on_coverage_arm", |b| {
        b.iter(|| {
            black_box(black_box(&exporter).render_prometheus());
            black_box(black_box(&hot).render_prometheus(black_box(10)));
            black_box(black_box(&hot).render_coverage_prometheus());
        })
    });
    group.finish();
}

/// The V2-F47 health arm's marginal cost over a populated tier: the
/// boot's composition (`cache_plane_scrape` — the exporter's families,
/// then the labeled window, then — when the operator opted in — the F32
/// posture exposition) measured at both arms. The `off` arm is the
/// byte-identical default shape; the `on` arm adds the posture pull
/// (`TieredCache::health` — one sampling pass over the tiers) plus the
/// `render_prometheus` render (nine fixed gauge families, ~1.2 KB of
/// `# HELP`/`# TYPE`/row text a scraper polls).
fn bench_cache_plane_health_scrape(c: &mut Criterion) {
    use oxide_fw_cache::{Entry, MetricsCacheObserver, Policy, TieredCache};

    // The boot's tier shape: a real TieredCache driven to resident
    // occupancy (write-through sets) so the posture pull reads a live
    // memory tier, not the zeroed default — the honest cost is the one
    // a populated stack pays.
    const BENCH_TIER_BYTES: u64 = 16 * 1024 * 1024;
    const BENCH_TIER_ENTRIES: usize = 256;
    let rt = tokio::runtime::Runtime::new().expect("the bench's tokio runtime");
    let tier = rt.block_on(async {
        let tier = TieredCache::new(BENCH_TIER_BYTES);
        let entry = Entry {
            body: vec![0u8; 512],
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            tags: vec!["bench".into()],
            policy: Policy {
                ttl: std::time::Duration::from_secs(60),
                swr: None,
                stale_if_error: None,
                soft_tags: true,
            },
            stored_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the wall clock runs")
                .as_millis() as u64,
            etag: "\"bench\"".into(),
        };
        for i in 0..BENCH_TIER_ENTRIES {
            tier.set(&format!("route:/{i:04}"), entry.clone())
                .await
                .expect("the tier takes the write");
        }
        tier
    });
    let exporter = MetricsCacheObserver::new();

    let mut group = c.benchmark_group("cache_plane_health_scrape");
    group.bench_function("off_default_shape", |b| {
        b.iter(|| {
            black_box(black_box(&exporter).render_prometheus());
        })
    });
    group.bench_function("on_health_arm", |b| {
        b.iter(|| {
            black_box(black_box(&exporter).render_prometheus());
            black_box(black_box(&tier).health().render_prometheus());
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_cache_hot_key_top_parse,
    bench_cache_hot_key_top_override,
    bench_cache_plane_coverage_scrape,
    bench_cache_plane_health_scrape
);
criterion_main!(benches);
