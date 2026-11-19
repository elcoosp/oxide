//! Criterion bench for the cache plane's hot-key attribution
//! (`hot_keys.rs`, §20): what the ledger's bounded per-key shape costs
//! on the two sides that matter.
//!
//! 1. The per-event attribution delta — `on_event` for a RESIDENT key
//!    (one lock, one lookup, one saturating add) versus a NEW key under
//!    the cap (admission: one insert) versus a NEW key PAST the cap
//!    (the refusal: the unattributed score, no map mutation). The
//!    resident arm is the hot L0 hit's whole attribution cost.
//! 2. The reads — `top(10)` over a 256-key window (the dashboard's
//!    sample call: one lock, the sort, the owned rows) — plus the F44
//!    typed snapshot arms (`snapshot(window)` over the full-capacity
//!    ledger at the default and cap windows, the coverage families'
//!    render) and the F45 hand-over arms (`take` over the full-capacity
//!    ledger — the destructive read rides `iter_batched` — and the
//!    empty-window floor).
//!
//! The key pool is built ONCE in setup (never inside a measured loop —
//! the measured op is the observer's, not a `format!`), and the new-key
//! arms rotate over it so every iteration is a distinct borrow handed
//! to the seam (the zero-alloc promise: the event carries the `&str` it
//! already holds).
//!
//! Run: `cargo bench -p oxide-fw-cache --features bench-util --bench hot_keys`

use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{CacheEvent, CacheObserver, CacheOp, HotKeyObserver};

/// The rotation pool: `n` distinct route keys, built once per bench.
fn key_pool(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("route:/{i:04}")).collect()
}

fn bench_attribution_delta(c: &mut Criterion) {
    let mut g = c.benchmark_group("cache_hot_key_attribution");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));
    let keys = key_pool(1024);

    // Resident key: one lock, one lookup, one saturating add — the hot
    // L0 hit's whole attribution cost.
    let resident = HotKeyObserver::bounded(1024);
    let event = CacheEvent {
        key: "route:/a",
        op: CacheOp::Set { bytes: 512 },
    };
    resident.on_event(&event);
    g.bench_function("on-event/resident-key", |b| {
        b.iter(|| resident.on_event(black_box(&event)));
    });

    // New key under the cap: the admission arm (one insert), rotating
    // over the pool so every iteration hands the ledger a fresh borrow.
    let admitting = HotKeyObserver::bounded(usize::MAX);
    let cursor = AtomicUsize::new(0);
    g.bench_function("on-event/new-key-admits", |b| {
        b.iter(|| {
            let i = cursor.fetch_add(1, Ordering::Relaxed) & 1023;
            let event = CacheEvent {
                key: keys[i].as_str(),
                op: CacheOp::Set { bytes: 512 },
            };
            admitting.on_event(black_box(&event));
        });
    });

    // New key past the cap: the refusal arm (the unattributed score,
    // no map mutation) — a tiny bound with the same stream.
    let refusing = HotKeyObserver::bounded(1);
    refusing.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Set { bytes: 512 },
    });
    let rotating = AtomicUsize::new(0);
    g.bench_function("on-event/cap-full-refuses", |b| {
        b.iter(|| {
            let i = rotating.fetch_add(1, Ordering::Relaxed) & 1023;
            let event = CacheEvent {
                key: keys[i].as_str(),
                op: CacheOp::Set { bytes: 512 },
            };
            refusing.on_event(black_box(&event));
        });
    });

    g.finish();
}

fn bench_top_window(c: &mut Criterion) {
    let mut g = c.benchmark_group("cache_hot_key_attribution");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));
    let keys = key_pool(256);

    // A 256-key window: the dashboard's sample call — one lock, the
    // deterministic sort, the owned rows.
    let hot = HotKeyObserver::bounded(1024);
    for key in &keys {
        hot.on_event(&CacheEvent {
            key,
            op: CacheOp::Set { bytes: 8 },
        });
    }
    g.bench_function("top/256-resident", |b| {
        b.iter(|| hot.top(black_box(10)));
    });

    g.finish();
}

fn bench_gate_delta(c: &mut Criterion) {
    // The F39 gate's delta (the F35 gate group's shape): the predicate
    // runs before the ledger lock, so the admitted arm pays the consult
    // plus the resident update and the rejected arm pays the consult
    // alone — the reject floor a narrow dashboard's gate rides.
    let mut g = c.benchmark_group("cache_hot_key_gate");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));
    let keys = key_pool(1024);

    // Admitted behind the default-as-a-type gate: the consult plus the
    // resident update (the delta over the ungated resident arm above).
    let admitting =
        HotKeyObserver::bounded(1024).with_gate(std::sync::Arc::new(oxide_fw_cache::AdmitAllKeys));
    let event = CacheEvent {
        key: "route:/a",
        op: CacheOp::Set { bytes: 512 },
    };
    admitting.on_event(&event);
    g.bench_function("on-event/gated/admitted", |b| {
        b.iter(|| admitting.on_event(black_box(&event)));
    });

    // Rejected behind the kind gate: every row refused before the lock —
    // the predicate consult is the whole cost, no map touch.
    let refusing = HotKeyObserver::bounded(1024).with_gate(std::sync::Arc::new(
        oxide_fw_cache::HotKeyKindGate::only(&["invalidate"]),
    ));
    let rotating = AtomicUsize::new(0);
    g.bench_function("on-event/gated/rejected", |b| {
        b.iter(|| {
            let i = rotating.fetch_add(1, Ordering::Relaxed) & 1023;
            let event = CacheEvent {
                key: keys[i].as_str(),
                op: CacheOp::Set { bytes: 512 },
            };
            refusing.on_event(black_box(&event));
        });
    });

    g.finish();
}

fn bench_operator_window(c: &mut Criterion) {
    // The F41 operator-tunable top window (§20): the boot resolves the
    // `OXIDE_CACHE_PLANE_HOT_KEY_TOP` spelling once and every scrape
    // renders that window — the arms measure what a widened window puts
    // on one scrape over a FULL-CAPACITY ledger (2048 keys, the boot's
    // own bound): the read (`top`) and the labeled render the §20
    // endpoint composes, at the F40 default (10), an operator's widened
    // window (50) and the honest cap (the ledger's bound — rows past the
    // occupancy do not exist, the clamp keeps one scrape bounded).
    let mut g = c.benchmark_group("cache_hot_key_top_window");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));
    let keys = key_pool(2048);
    let hot = HotKeyObserver::bounded(2048);
    for key in &keys {
        hot.on_event(&CacheEvent {
            key,
            op: CacheOp::Set { bytes: 8 },
        });
    }

    g.bench_function("top/10-default", |b| {
        b.iter(|| hot.top(black_box(10)));
    });
    g.bench_function("top/50-operator", |b| {
        b.iter(|| hot.top(black_box(50)));
    });
    g.bench_function("top/2048-cap", |b| {
        b.iter(|| hot.top(black_box(2048)));
    });
    g.bench_function("render/10-default", |b| {
        b.iter(|| hot.render_prometheus(black_box(10)));
    });
    g.bench_function("render/2048-cap", |b| {
        b.iter(|| hot.render_prometheus(black_box(2048)));
    });

    g.finish();
}

fn bench_typed_snapshot(c: &mut Criterion) {
    // The F44 typed read (§20): the whole ledger answers as ONE value
    // under ONE lock pass — the arms measure what the one-pass shape
    // costs over the read it replaces (`top`), and the coverage
    // families' render, over the same FULL-capacity 2048-key ledger the
    // operator-window group rides.
    let mut g = c.benchmark_group("cache_hot_key_snapshot");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));
    let keys = key_pool(2048);
    let hot = HotKeyObserver::bounded(2048);
    for key in &keys {
        hot.on_event(&CacheEvent {
            key,
            op: CacheOp::Set { bytes: 8 },
        });
    }

    g.bench_function("snapshot/10-default", |b| {
        b.iter(|| hot.snapshot(black_box(10)));
    });
    g.bench_function("snapshot/2048-cap", |b| {
        b.iter(|| hot.snapshot(black_box(2048)));
    });
    g.bench_function("render-coverage/2048-occupied", |b| {
        b.iter(|| hot.render_coverage_prometheus());
    });

    g.finish();
}

fn bench_window_take(c: &mut Criterion) {
    // The F45 windowed hand-over (§20): the read-and-reset pair the
    // per-interval reporter hand-rolled, composed atomically. The arms
    // measure the full-capacity hand-over (every tracked row cloned
    // into the value, the map cleared under the same lock pass) and the
    // fresh-window floor a reporter pays between busy windows.
    //
    // The full-capacity arm rides `iter_batched`: the take is
    // DESTRUCTIVE, so the setup refills the ledger (unmeasured) and the
    // routine hands the whole 2048-key window over — a plain `b.iter`
    // would drain on the first call and measure an empty hand-over
    // forever after.
    let mut g = c.benchmark_group("cache_hot_key_take");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));
    let keys = key_pool(2048);
    let hot = HotKeyObserver::bounded(2048);
    g.bench_function("take/full-capacity", |b| {
        b.iter_batched(
            || {
                for key in &keys {
                    hot.on_event(&CacheEvent {
                        key,
                        op: CacheOp::Set { bytes: 8 },
                    });
                }
            },
            |_| hot.take(),
            criterion::BatchSize::SmallInput,
        );
    });

    let empty = HotKeyObserver::bounded(2048);
    g.bench_function("take/empty", |b| {
        b.iter(|| empty.take());
    });

    g.finish();
}

criterion_group!(
    cache_hot_key_attribution,
    bench_attribution_delta,
    bench_top_window
);
criterion_group!(cache_hot_key_gate, bench_gate_delta);
criterion_group!(cache_hot_key_top_window, bench_operator_window);
criterion_group!(cache_hot_key_snapshot, bench_typed_snapshot);
criterion_group!(cache_hot_key_take, bench_window_take);
criterion_main!(
    cache_hot_key_attribution,
    cache_hot_key_gate,
    cache_hot_key_top_window,
    cache_hot_key_snapshot,
    cache_hot_key_take
);
