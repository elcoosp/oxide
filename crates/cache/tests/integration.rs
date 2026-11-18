//! The cache crate's cross-module integration pins (the placeholder
//! `mod placeholder {}` stub, filled): one flow through the key, tier
//! and single-flight modules — the shapes the server's route step rides.
//!
//! The crate's unit suites pin each module's pieces
//! (`cache_tdd.rs`/`build_id_tdd.rs`); these pins hold the MODULES
//! TOGETHER: a `CacheKey`'s canonical digest is the storage key the
//! `TieredCache` serves and the single-flight map coalesces, the
//! freshness window decays exactly as `Entry`'s policy promises, and a
//! soft tag invalidation flips a stored row stale across the tiers.

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{CacheKey, Entry, FlightSlot, Hit, Layer, Policy, SingleFlight, TieredCache};

/// One stored entry (the L0 row shape: HTML body, tags, an SWR window),
/// stamped at the real wall clock (the freshness math runs on unix ms).
fn row(tags: &[&str], ttl: Duration, swr: Option<Duration>) -> Entry {
    let stored_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the wall clock")
        .as_millis() as u64;
    Entry {
        body: b"<html>ok</html>".to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
        policy: Policy {
            ttl,
            swr,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at,
        etag: "\"v1\"".into(),
    }
}

/// The decay arm's row: the same shape with `stored_at` BACKDATED by
/// `back_ms` — the window's math runs on the stored stamp, so aging the
/// stamp is the deterministic decay (a wall-clock sleep here raced its
/// own 10 ms ttl under a loaded scheduler and flaked the pin).
fn row_backdated(tags: &[&str], ttl: Duration, swr: Option<Duration>, back_ms: u64) -> Entry {
    let mut entry = row(tags, ttl, swr);
    entry.stored_at = entry.stored_at.saturating_sub(back_ms);
    entry
}

/// A tiered store with a disk tier under it (both tiers cross the pins).
fn tiered() -> Arc<TieredCache> {
    let root = Box::leak(Box::new(tempfile::tempdir().expect("temp disk tier")));
    Arc::new(
        TieredCache::new(1 << 20)
            .with_disk(root.path().to_path_buf())
            .expect("the disk tier opens"),
    )
}

/// The canonical digest is the storage key: one `CacheKey` → one
/// `TieredCache` row, and the freshness window decays exactly as the
/// policy promises (fresh inside the TTL, stale inside the SWR, a miss
/// once no SWR window remains).
#[tokio::test]
async fn key_digest_names_the_row_and_the_freshness_window_decays() {
    let cache = tiered();
    let key = CacheKey {
        layer: Layer::L0,
        route: "/index".into(),
        params: String::new(),
        vary: String::new(),
        build_id: "0123456789abcdef".into(),
    };
    let digest = key.canonical();
    assert_eq!(digest.len(), 64, "the digest is blake3 hex");

    // The SWR row: fresh inside the TTL, stale inside the SWR window.
    // The decay runs on BACKDATED stamps, not sleeps — the fresh row
    // stores now (fresh for 60 s), the aged row's stamp sits 100 ms in
    // the past with a 50 ms ttl (stale), the no-SWR row the same age
    // (expired) — deterministic under any scheduler.
    let swr_row = row(
        &["route:/index"],
        Duration::from_secs(60),
        Some(Duration::from_secs(60)),
    );
    cache.set(&digest, swr_row).await.expect("the row stores");
    let hit = cache.get(&digest).await.expect("the get succeeds");
    assert!(
        matches!(hit, Some(Hit::Fresh(_))),
        "a just-stored row answers fresh"
    );

    let aged = row_backdated(
        &["route:/index"],
        Duration::from_millis(50),
        Some(Duration::from_secs(60)),
        100,
    );
    let digest_stale = CacheKey {
        route: "/index-aged".into(),
        ..key.clone()
    };
    cache
        .set(&digest_stale.canonical(), aged)
        .await
        .expect("the row stores");
    let hit = cache
        .get(&digest_stale.canonical())
        .await
        .expect("the get succeeds");
    assert!(
        matches!(hit, Some(Hit::Stale(_))),
        "past the TTL, inside the SWR window, the row answers stale"
    );

    // The no-SWR row under a third key: past the TTL there is no stale
    // window — a miss.
    let no_swr = CacheKey {
        layer: Layer::L1,
        route: "/data".into(),
        ..key.clone()
    };
    let digest_b = no_swr.canonical();
    assert_ne!(digest, digest_b, "a component change changes the digest");
    cache
        .set(
            &digest_b,
            row_backdated(&[], Duration::from_millis(50), None, 100),
        )
        .await
        .expect("the row stores");
    let hit = cache.get(&digest_b).await.expect("the get succeeds");
    assert!(hit.is_none(), "no SWR window: the expired row is a miss");
}

/// One rebuild serves every waiter: the canonical key acquires a
/// single-flight slot (one owner), a concurrent acquire follows, the
/// owner publishes the computed row, the follower receives it, the
/// in-flight map cleans itself, and the next acquire starts a fresh
/// flight.
#[tokio::test(flavor = "multi_thread")]
async fn single_flight_coalesces_a_rebuild_over_the_tiered_store() {
    use oxide_fw_cache::FlightOwnerGuard;

    let cache = tiered();
    let key = CacheKey {
        layer: Layer::L0,
        route: "/submit".into(),
        params: String::new(),
        vary: String::new(),
        build_id: "0123456789abcdef".into(),
    };
    let digest = key.canonical();
    let flights: Arc<dashmap::DashMap<String, Arc<tokio::sync::broadcast::Sender<Arc<Entry>>>>> =
        Arc::new(dashmap::DashMap::new());
    let single = SingleFlight::new();

    // The first acquirer owns the flight.
    let first = single.acquire(&flights, &digest).await;
    let owner_tx = match &first {
        FlightSlot::Owner(tx) => tx.clone(),
        _ => panic!("the first acquirer owns the flight"),
    };
    // A concurrent acquirer follows (subscribed to the owner's channel).
    let second = single.acquire(&flights, &digest).await;
    let mut rx = match second {
        FlightSlot::Follower(rx) => rx,
        _ => panic!("the second acquirer follows"),
    };

    // The owner computes (stores the row), then publishes through the
    // RAII guard — followers receive it, the map cleans itself. The
    // follower count is the F33 emitter's join number; this pin reaches
    // exactly one follower.
    let computed = Arc::new(row(&["route:/submit"], Duration::from_secs(60), None));
    cache
        .set(&digest, (*computed).clone())
        .await
        .expect("the computed row stores");
    let reached =
        FlightOwnerGuard::new(flights.clone(), digest.clone(), owner_tx).publish(computed.clone());
    assert_eq!(reached, 1, "the one subscribed follower was reached");
    let received = rx.recv().await.expect("the follower receives the row");
    assert_eq!(received.body, computed.body);
    assert!(
        flights.get(&digest).is_none(),
        "the publish removed the in-flight entry"
    );

    // The map is clean, so the next acquire starts a fresh flight.
    let third = single.acquire(&flights, &digest).await;
    assert!(
        matches!(third, FlightSlot::Owner(_)),
        "after the flight ended, the next acquirer owns again"
    );
}

/// A soft tag invalidation flips a stored row stale across the tiers
/// (the body stays servable, the freshness window collapses); a hard
/// invalidation removes the row outright.
#[tokio::test]
async fn soft_tag_invalidation_flips_the_row_stale_across_tiers() {
    let cache = tiered();
    let key = CacheKey {
        layer: Layer::L0,
        route: "/blog/a".into(),
        params: String::new(),
        vary: String::new(),
        build_id: "0123456789abcdef".into(),
    };
    let digest = key.canonical();
    let entry = row(
        &["route:/blog/a", "path:/blog/a"],
        Duration::from_secs(3600),
        Some(Duration::from_secs(3600)),
    );
    cache.set(&digest, entry).await.expect("the row stores");
    let hit = cache.get(&digest).await.expect("the get succeeds");
    assert!(matches!(hit, Some(Hit::Fresh(_))));

    // Soft: the row turns stale immediately (serve stale, regenerate).
    let invalidated = cache
        .invalidate_tag("route:/blog/a", true)
        .await
        .expect("the soft invalidation succeeds");
    assert_eq!(invalidated, 1, "one row carried the tag");
    let hit = cache.get(&digest).await.expect("the get succeeds");
    assert!(
        matches!(hit, Some(Hit::Stale(_))),
        "the soft invalidation collapsed the freshness window"
    );

    // Hard: the row leaves every tier.
    let invalidated = cache
        .invalidate_tag("route:/blog/a", false)
        .await
        .expect("the hard invalidation succeeds");
    assert_eq!(invalidated, 1);
    let hit = cache.get(&digest).await.expect("the get succeeds");
    assert!(hit.is_none(), "the hard invalidation removed the row");
}
