//! TDD pins for the V2-F07 sweep task (`spawn_sweep`, spec v0.2 §V2-F07
//! wiring step 3: "A background task checks every 30 s" — the store's
//! housekeeping). The store's own `sweep` predicate is pinned in
//! `stream_tdd.rs`; these pins cover the wiring's cadence task: it calls
//! `sweep` on the cadence (expired buffers are reclaimed without any
//! request having to arrive first) and keeps running across multiple
//! ticks without dropping in-TTL buffers.
//!
//! The store's TTL runs on `std::time::Instant`, which does not advance
//! under tokio's paused clock — the pins shrink every duration instead of
//! pausing time, the same real-sleep shape the substrate pins use.
//!
//! Written FIRST (red): these pins failed on the unresolved
//! `spawn_sweep` symbol before the task existed.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use oxide_fw_server::stream_store::{Reattach, StreamStore, spawn_sweep};

#[tokio::test]
async fn sweep_task_reclaims_expired_buffers_on_its_cadence() {
    // The boot wiring runs the spec's 30 s cadence with the config's ttl;
    // the pin shrinks both by the same factor (1 s ttl, a 50 ms tick).
    let store = Arc::new(StreamStore::new(
        Duration::from_millis(400),
        1 << 20,
        1 << 30,
    ));
    let buf = store.begin("gone-soon".into());
    store.append(&buf, Bytes::from_static(b"payload"));

    let _task = spawn_sweep(Arc::clone(&store), Duration::from_millis(50));

    // Before the buffer's TTL: reattachable.
    assert!(
        matches!(store.get("gone-soon"), Reattach::Ok(_)),
        "a live buffer reattaches"
    );

    // Past the TTL the sweep task must have reclaimed it — without any
    // request forcing the housekeeping. Two ticks past the expiry bound
    // cover the cadence racing the real clock.
    tokio::time::sleep(Duration::from_millis(600)).await;

    assert!(
        matches!(store.get("gone-soon"), Reattach::Gone),
        "the expired buffer is gone"
    );
}

#[tokio::test]
async fn sweep_task_runs_every_tick_and_keeps_live_buffers() {
    let store = Arc::new(StreamStore::new(Duration::from_secs(60), 1 << 20, 1 << 30));
    let buf = store.begin("stays".into());
    store.append(&buf, Bytes::from_static(b"kept"));

    let _task = spawn_sweep(Arc::clone(&store), Duration::from_millis(20));

    // Several cadence ticks pass; the in-TTL buffer survives every sweep.
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            matches!(store.get("stays"), Reattach::Ok(_)),
            "sweeping never drops a buffer inside its ttl"
        );
    }
}
