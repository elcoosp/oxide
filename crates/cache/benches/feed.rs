//! Criterion bench for the cache plane's bounded event feed (`feed.rs`,
//! §20): the costs the feed adds to the two sides of the seam.
//!
//! 1. The producer side — what `on_event` costs the cache's call path
//!    when the queue has room (the push + lock) versus when it is
//!    saturated (the drop + the sticky-warn check; the warn itself is a
//!    one-time state flip after the first drop, so the saturated steady
//!    state is a counter add, not a render).
//! 2. The consumer side — `try_pop` and the batch `drain` over a full
//!    queue, the loop a host's task runs to move rows off-process.
//! 3. The gate's two arms (F35) — the admit path's delta over the
//!    ungated push (the predicate call before the push) and the reject
//!    path's floor (the predicate call, no owned row, no lock).
//!
//! Run: `cargo bench -p oxide-fw-cache --features bench-util --bench feed`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{CacheEvent, CacheObserver, CacheOp, EventFeed, KindGate};

fn bench_feed_producer(c: &mut Criterion) {
    let mut g = c.benchmark_group("cache_event_feed");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    // Room in the queue: the push path a warm feed pays.
    let roomy = Arc::new(EventFeed::bounded(1024));
    g.bench_function("on-event/roomy", |b| {
        b.iter(|| {
            let observer: Arc<dyn oxide_fw_cache::CacheObserver> = roomy.clone();
            observer.on_event(&CacheEvent {
                key: black_box("route:/a"),
                op: CacheOp::Set { bytes: 512 },
            });
        });
    });

    // Saturated: the drop path (queue pinned full; every arrival drops).
    let saturated = Arc::new(EventFeed::bounded(1));
    saturated.on_event(&CacheEvent {
        key: "route:/seed",
        op: CacheOp::Set { bytes: 512 },
    });
    g.bench_function("on-event/saturated", |b| {
        b.iter(|| {
            let observer: Arc<dyn oxide_fw_cache::CacheObserver> = saturated.clone();
            observer.on_event(&CacheEvent {
                key: black_box("route:/a"),
                op: CacheOp::Set { bytes: 512 },
            });
        });
    });

    g.finish();
}

fn bench_feed_consumer(c: &mut Criterion) {
    let mut g = c.benchmark_group("cache_event_feed");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    // One row at a time: the processing loop a streaming consumer runs.
    let feed = Arc::new(EventFeed::bounded(256));
    g.bench_function("try-pop/full-queue", |b| {
        b.iter_batched(
            || {
                while feed.try_pop().is_some() {}
                for i in 0u32..256 {
                    feed.on_event(&CacheEvent {
                        key: "route:/a",
                        op: CacheOp::Set {
                            bytes: u64::from(i),
                        },
                    });
                }
            },
            |()| {
                while let Some(row) = feed.try_pop() {
                    black_box(row);
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // The batch arm: one drain call empties the full queue.
    g.bench_function("drain/256-rows", |b| {
        b.iter_batched(
            || {
                while feed.try_pop().is_some() {}
                for i in 0u32..256 {
                    feed.on_event(&CacheEvent {
                        key: "route:/a",
                        op: CacheOp::Set {
                            bytes: u64::from(i),
                        },
                    });
                }
            },
            |()| {
                let rows = feed.drain();
                black_box(rows.len());
            },
            criterion::BatchSize::SmallInput,
        );
    });

    g.finish();
}

fn bench_feed_gate(c: &mut Criterion) {
    let mut g = c.benchmark_group("cache_event_feed_gate");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    let event = CacheEvent {
        key: "route:/a",
        op: CacheOp::Set { bytes: 512 },
    };
    let rejected = CacheEvent {
        key: "route:/a",
        op: CacheOp::Miss,
    };

    // The admit arm: the gate's predicate call before the ungated push —
    // the delta over `on-event/roomy` is the narrowing's price.
    let gated = EventFeed::gated(1024, Arc::new(KindGate::only(&["set"])));
    let admitted_obs: Arc<dyn CacheObserver> = gated.clone();
    g.bench_function("on-event/gated/admitted", |b| {
        b.iter(|| {
            admitted_obs.on_event(black_box(&event));
            while gated.try_pop().is_some() {}
        });
    });

    // The reject arm: the predicate call, no owned row, no lock — the
    // floor a narrow watcher pays for the hot rows it declines.
    let narrow = EventFeed::gated(1024, Arc::new(KindGate::only(&["invalidate"])));
    let narrow_obs: Arc<dyn CacheObserver> = narrow.clone();
    g.bench_function("on-event/gated/rejected", |b| {
        b.iter(|| narrow_obs.on_event(black_box(&rejected)));
    });

    g.finish();
}

criterion_group!(
    bench_feed,
    bench_feed_producer,
    bench_feed_consumer,
    bench_feed_gate
);
criterion_main!(bench_feed);
