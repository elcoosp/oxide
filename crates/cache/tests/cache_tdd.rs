//! TDD suite for oxide-cache (spec §11). Written first; implementation brought to green.
//! Exercises only the public crate API. Deterministic: no sleeps > 50 ms (freshness is
//! driven by manipulating `Entry::stored_at` against the wall clock, never by waiting).

use bytes::Bytes;
use oxide_cache::{
    build_id, cache_control, conditional_response, CacheHandler, CacheKey, Entry, FetchMemo,
    FlightSlot, Hit, Layer, Policy, SingleFlight, TieredCache,
};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn entry(body: &[u8], tags: &[&str], policy: Policy) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().map(|s| s.to_string()).collect(),
        policy,
        stored_at: now_ms(),
        etag: "\"v1\"".into(),
    }
}

fn fresh_policy(ttl_ms: u64, swr_ms: u64) -> Policy {
    Policy {
        ttl: Duration::from_millis(ttl_ms),
        swr: Some(Duration::from_millis(swr_ms)),
        stale_if_error: None,
        soft_tags: true,
    }
}

// ---------------------------------------------------------------- CacheKey / build_id

#[test]
fn canonical_key_deterministic_and_component_sensitive() {
    let k = |layer: Layer, route: &str, params: &str, vary: &str, build: &str| CacheKey {
        layer,
        route: route.into(),
        params: params.into(),
        vary: vary.into(),
        build_id: build.into(),
    };
    let a = k(Layer::L0, "/blog/[slug]", "slug=a", "lang=en", "b1").canonical();
    assert_eq!(a.len(), 64, "blake3 hex digest");
    assert_eq!(
        a,
        k(Layer::L0, "/blog/[slug]", "slug=a", "lang=en", "b1").canonical()
    );
    // Layer sensitive.
    assert_ne!(
        a,
        k(Layer::L1, "/blog/[slug]", "slug=a", "lang=en", "b1").canonical()
    );
    // Every component participates.
    assert_ne!(
        a,
        k(Layer::L0, "/blog/[slug]", "slug=b", "lang=en", "b1").canonical()
    );
    assert_ne!(
        a,
        k(Layer::L0, "/blog/[slug]", "slug=a", "lang=de", "b1").canonical()
    );
    assert_ne!(
        a,
        k(Layer::L0, "/blog/[slug]", "slug=a", "lang=en", "b2").canonical()
    );
    // Field boundaries matter (concat without separators would collide a|bc with ab|c).
    assert_ne!(
        k(Layer::L0, "ab", "c", "", "").canonical(),
        k(Layer::L0, "a", "bc", "", "").canonical()
    );
}

#[test]
fn build_id_stable_and_16_hex() {
    let a = build_id(&[b"routes.json", b"manifest"]);
    let b = build_id(&[b"routes.json", b"manifest"]);
    let c = build_id(&[b"routes.json", b"manifest!"]);
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(a.len(), 16);
    // Length-prefixing separates adjacent parts deterministically.
    assert_ne!(build_id(&[b"ab", b"c"]), build_id(&[b"a", b"bc"]));
}

// ---------------------------------------------------------------- MemoryTier LRU

#[tokio::test]
async fn memory_evicts_to_stay_under_cap() {
    // moka's TinyLFU eviction is frequency-based, not strict LRU (spec §11.2:
    // "moka, TinyLFU, weighted by bytes"). This test asserts the invariants the
    // policy does guarantee: the byte cap is respected and the tier stays usable.
    let mem = oxide_cache::MemoryTier::new(400);
    // weight(body=100, 1 header k=12+v=9 => 21) = 100 + 21 + 32 = 153
    let w = entry(&[b'a'; 100], &[], Policy::default());
    assert_eq!(mem.total_bytes(), 0);
    mem.insert("a", w.clone());
    mem.insert("b", w.clone());
    assert_eq!(mem.len(), 2);
    assert_eq!(mem.total_bytes(), 306);
    assert!(mem.get("a").is_some(), "both fit under 400");
    mem.insert("c", w); // 459 > 400 -> moka evicts to fit
    assert!(
        mem.total_bytes() <= 400,
        "cap respected: {}",
        mem.total_bytes()
    );
    assert!(
        (1..=2).contains(&mem.len()),
        "capacity is 2 entries at weight 153 each; got {}",
        mem.len()
    );
    // The tier remains usable after eviction and clear. (moka's TinyLFU admission
    // is frequency-based, so a freshly-inserted cold entry can be evicted by the
    // same housekeeping pass; the durability guarantee is that `clear` + `insert`
    // yields a retrievable entry.)
    mem.clear();
    assert!(mem.is_empty(), "clear empties the tier");
    assert_eq!(mem.total_bytes(), 0, "clear resets byte accounting");
    mem.insert("d", entry(&[b'd'; 50], &[], Policy::default()));
    assert!(mem.get("d").is_some(), "tier usable after clear + insert");
}

#[tokio::test]
async fn memory_byte_accounting_stays_under_cap() {
    // Under TinyLFU no specific entry is guaranteed to survive (the policy is
    // frequency-based, not LRU); the guarantees are the byte cap and non-emptiness
    // after a series of inserts.
    let mem = oxide_cache::MemoryTier::new(300);
    for i in 0..10 {
        let mut e = entry(&[b'x'; 100], &[], Policy::default());
        e.etag = format!("\"e{i}\"");
        mem.insert(&format!("k{i}"), e);
    }
    assert!(mem.total_bytes() <= 300, "under cap: {}", mem.total_bytes());
    assert!(mem.len() >= 1, "at least one entry survives");
}

#[tokio::test]
async fn oversize_entry_is_rejected_not_stored() {
    let mem = oxide_cache::MemoryTier::new(100);
    let big = entry(&[b'z'; 500], &[], Policy::default());
    mem.insert("big", big);
    assert!(
        mem.get("big").is_none(),
        "entry heavier than the cap is refused"
    );
    assert_eq!(mem.total_bytes(), 0);
    // The tier still works afterwards.
    mem.insert("small", entry(&[b's'; 10], &[], Policy::default()));
    assert!(mem.get("small").is_some());
}

// ---------------------------------------------------------------- DiskTier

#[tokio::test]
async fn disk_open_creates_dirs_and_roundtrips_compressed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("nested/cache");
    let disk = oxide_cache::DiskTier::open(root.clone()).unwrap();
    assert!(root.is_dir(), "open() creates the root");
    assert!(root.join("blobs").is_dir(), "open() creates the blob dir");

    let raw = entry(&[b'a'; 8192], &["t"], fresh_policy(60_000, 0));
    let uncompressed_len = serde_json::to_vec(&raw).unwrap().len();
    disk.set("k1", &raw).unwrap();

    // Blob lives under blobs/<first2>/<name>.bin and is zstd-compressed.
    let mut blobs = std::fs::read_dir(root.join("blobs")).unwrap();
    let shard = blobs.next().unwrap().unwrap().path();
    let bin = std::fs::read_dir(&shard)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(bin.extension().unwrap(), "bin");
    let stored_len = std::fs::read(&bin).unwrap().len();
    assert!(
        stored_len < uncompressed_len,
        "zstd must shrink repetitive payload: stored={stored_len} raw={uncompressed_len}"
    );

    let got = disk.get("k1").unwrap().expect("roundtrip");
    assert_eq!(got.body, raw.body);
    assert_eq!(got.etag, raw.etag);
    assert_eq!(got.policy, raw.policy);
    assert_eq!(got.tags, raw.tags);
    assert_eq!(got.stored_at, raw.stored_at);
    assert!(disk.get("missing").unwrap().is_none());
}

#[tokio::test]
async fn disk_tag_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let disk = oxide_cache::DiskTier::open(dir.path().join("c")).unwrap();
    disk.set("k1", &entry(b"1", &["t1", "t2"], Policy::default()))
        .unwrap();
    disk.set("k2", &entry(b"2", &["t1"], Policy::default()))
        .unwrap();
    disk.set("k3", &entry(b"3", &["t3"], Policy::default()))
        .unwrap();

    let mut t1 = disk.tag_keys("t1").unwrap();
    t1.sort();
    assert_eq!(t1, vec!["k1".to_string(), "k2".to_string()]);
    assert_eq!(disk.tag_keys("t2").unwrap(), vec!["k1".to_string()]);
    assert!(disk.tag_keys("nope").unwrap().is_empty());

    // Deleting a key removes it from the tag index too.
    disk.delete("k1").unwrap();
    assert_eq!(disk.tag_keys("t2").unwrap(), Vec::<String>::new());
}

#[tokio::test]
async fn disk_corrupt_blob_is_a_miss_and_self_heals() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("c");
    let disk = oxide_cache::DiskTier::open(root.clone()).unwrap();
    disk.set("k1", &entry(b"payload", &["t"], Policy::default()))
        .unwrap();

    // Corrupt the blob on disk (garbage bytes).
    let mut blobs = std::fs::read_dir(root.join("blobs")).unwrap();
    let shard = blobs.next().unwrap().unwrap().path();
    let bin = std::fs::read_dir(&shard)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&bin, vec![0u8; 64]).unwrap();

    // Corruption never errors and never serves garbage: it is a silent miss + eviction.
    assert!(disk.get("k1").unwrap().is_none());
    assert!(
        disk.get("k1").unwrap().is_none(),
        "entry was evicted from the index"
    );
    assert!(disk.tag_keys("t").unwrap().is_empty(), "tag index cleaned");
}

#[tokio::test]
async fn disk_delete_removes_blob_and_index() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("c");
    let disk = oxide_cache::DiskTier::open(root.clone()).unwrap();
    disk.set("abcd", &entry(b"x", &[], Policy::default()))
        .unwrap();
    assert!(disk.get("abcd").unwrap().is_some());
    disk.delete("abcd").unwrap();
    assert!(disk.get("abcd").unwrap().is_none());
    // Blob file gone.
    let shards: Vec<_> = std::fs::read_dir(root.join("blobs")).unwrap().collect();
    for s in shards {
        let n = std::fs::read_dir(s.unwrap().path()).unwrap().count();
        assert_eq!(n, 0, "blob file removed");
    }
}

// ---------------------------------------------------------------- TieredCache: tiers + freshness

#[tokio::test]
async fn tier_promotion_disk_hit_promotes_to_memory() {
    let dir = tempfile::tempdir().unwrap();
    let cache = TieredCache::new(1 << 20)
        .with_disk(dir.path().join("c"))
        .unwrap();
    let e = entry(b"body", &[], fresh_policy(60_000, 0));

    assert!(cache.get("k").await.unwrap().is_none());
    cache.set("k", e).await.unwrap();
    drop(cache); // release the redb file lock before reopening the same root

    // Simulate a process restart of the memory tier: fresh instance, same disk.
    let cache2 = TieredCache::new(1 << 20)
        .with_disk(dir.path().join("c"))
        .unwrap();
    let hit = cache2.get("k").await.unwrap().expect("disk hit");
    assert!(matches!(hit, Hit::Fresh(_)));
    let s1 = cache2.stats();
    assert_eq!(s1.hits, 1);
    assert_eq!(s1.misses, 0, "this instance has only seen the disk hit");

    let hit2 = cache2.get("k").await.unwrap().expect("promoted hit");
    assert!(matches!(hit2, Hit::Fresh(_)));
    let s2 = cache2.stats();
    assert_eq!(s2.hits, 2);
    assert_eq!(
        s2.misses, 0,
        "both reads of this instance hit (disk, then memory)"
    );
    assert_eq!(
        s2.entries_memory, 1,
        "second get is served from memory (promoted)"
    );
    assert_eq!(s2.bytes_memory, cache2.memory.total_bytes());
    assert!(s2.bytes_memory > 0);
}

#[tokio::test]
async fn fresh_stale_expired_transitions() {
    let cache = TieredCache::new(1 << 20);
    let mut e = entry(b"b", &[], fresh_policy(30, 30)); // ttl 30ms, swr 30ms

    // Fresh: stored now.
    cache.set("k", e.clone()).await.unwrap();
    assert!(matches!(cache.get("k").await.unwrap(), Some(Hit::Fresh(_))));

    // Stale: past ttl, inside swr. Manipulate stored_at directly (Entry has pub fields).
    e.stored_at = now_ms().saturating_sub(40);
    cache.set("k2", e.clone()).await.unwrap();
    let s = cache.stats();
    assert!(matches!(
        cache.get("k2").await.unwrap(),
        Some(Hit::Stale(_))
    ));
    assert_eq!(cache.stats().stale_served, s.stale_served + 1);

    // Expired: past ttl + swr -> miss, lazily deleted.
    e.stored_at = now_ms().saturating_sub(500);
    cache.set("k3", e).await.unwrap();
    assert!(
        cache.get("k3").await.unwrap().is_none(),
        "expired is a miss"
    );
    assert!(
        cache.get("k3").await.unwrap().is_none(),
        "and stays a miss (lazily deleted)"
    );
    assert_eq!(
        cache.memory.len(),
        2,
        "k3 evicted; k (fresh) + k2 (stale-usable) remain"
    );
    let s = cache.stats();
    assert_eq!(s.misses, 2, "both expired reads of k3");
    assert_eq!(s.hits, 2, "fresh + stale count as hits");
    assert_eq!(s.stale_served, 1);

    // Entry::is_fresh helper.
    let mut helper = entry(b"h", &[], fresh_policy(1000, 0));
    assert!(helper.is_fresh(now_ms()));
    helper.stored_at = now_ms().saturating_sub(2000);
    assert!(!helper.is_fresh(now_ms()));
}

#[tokio::test]
async fn no_swr_means_stale_is_a_miss() {
    let cache = TieredCache::new(1 << 20);
    let mut e = entry(
        b"b",
        &[],
        Policy {
            ttl: Duration::from_millis(10),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
    );
    e.stored_at = now_ms().saturating_sub(50);
    cache.set("k", e).await.unwrap();
    assert!(
        cache.get("k").await.unwrap().is_none(),
        "no swr -> past ttl is a plain miss"
    );
}

// ---------------------------------------------------------------- TieredCache: tags / paths / seed

#[tokio::test]
async fn invalidate_tag_soft_vs_hard() {
    let cache = TieredCache::new(1 << 20);
    cache
        .set("a", entry(b"a", &["x"], fresh_policy(60_000, 0)))
        .await
        .unwrap();
    cache
        .set("b", entry(b"b", &["x", "y"], fresh_policy(60_000, 0)))
        .await
        .unwrap();
    cache
        .set("c", entry(b"c", &["y"], fresh_policy(60_000, 0)))
        .await
        .unwrap();

    // SOFT: entries remain but are marked stale (ttl zeroed, swr extended by original ttl).
    let n = cache.invalidate_tag("x", true).await.unwrap();
    assert_eq!(n, 2);
    assert!(matches!(cache.get("a").await.unwrap(), Some(Hit::Stale(_))));
    assert!(matches!(cache.get("b").await.unwrap(), Some(Hit::Stale(_))));
    assert!(
        matches!(cache.get("c").await.unwrap(), Some(Hit::Fresh(_))),
        "untagged stays fresh"
    );

    // HARD: entries are gone.
    let n = cache.invalidate_tag("x", false).await.unwrap();
    assert_eq!(n, 2);
    assert!(cache.get("a").await.unwrap().is_none());
    assert!(cache.get("b").await.unwrap().is_none());
    assert!(matches!(cache.get("c").await.unwrap(), Some(Hit::Fresh(_))));
    // Invalidating again affects nobody.
    assert_eq!(cache.invalidate_tag("x", false).await.unwrap(), 0);
}

#[tokio::test]
async fn soft_invalidation_survives_disk_tier() {
    let dir = tempfile::tempdir().unwrap();
    let cache = TieredCache::new(1 << 20)
        .with_disk(dir.path().join("c"))
        .unwrap();
    cache
        .set("d1", entry(b"1", &["t"], fresh_policy(60_000, 0)))
        .await
        .unwrap();
    cache
        .set("d2", entry(b"2", &["t"], fresh_policy(60_000, 0)))
        .await
        .unwrap();
    // Evict everything from memory to prove the disk path participates.
    cache.memory.clear();
    assert_eq!(cache.memory.len(), 0);

    let n = cache.invalidate_tag("t", true).await.unwrap();
    assert_eq!(n, 2, "soft invalidation reaches disk-only entries");
    assert!(matches!(
        cache.get("d1").await.unwrap(),
        Some(Hit::Stale(_))
    ));
    assert!(matches!(
        cache.get("d2").await.unwrap(),
        Some(Hit::Stale(_))
    ));
}

#[tokio::test]
async fn invalidate_path_via_set_with_path() {
    let cache = TieredCache::new(1 << 20);
    cache
        .set_with_path(
            "p1",
            entry(b"a", &["other"], fresh_policy(60_000, 0)),
            "/blog/a",
        )
        .await
        .unwrap();
    cache
        .set_with_path("p2", entry(b"b", &[], fresh_policy(60_000, 0)), "/blog/a")
        .await
        .unwrap();
    cache
        .set("p3", entry(b"c", &[], fresh_policy(60_000, 0)))
        .await
        .unwrap();

    let n = cache.invalidate_path("/blog/a", false).await.unwrap();
    assert_eq!(n, 2);
    assert!(cache.get("p1").await.unwrap().is_none());
    assert!(cache.get("p2").await.unwrap().is_none());
    assert!(cache.get("p3").await.unwrap().is_some());

    // Soft path invalidation keeps entries stale-but-servable.
    cache
        .set_with_path("p4", entry(b"d", &[], fresh_policy(60_000, 0)), "/blog/b")
        .await
        .unwrap();
    let n = cache.invalidate_path("/blog/b", true).await.unwrap();
    assert_eq!(n, 1);
    assert!(matches!(
        cache.get("p4").await.unwrap(),
        Some(Hit::Stale(_))
    ));
}

#[tokio::test]
async fn tags_are_deduped_across_sets() {
    let cache = TieredCache::new(1 << 20);
    cache
        .set("k", entry(b"1", &["t", "t"], fresh_policy(60_000, 0)))
        .await
        .unwrap();
    cache
        .set("k", entry(b"2", &["t"], fresh_policy(60_000, 0)))
        .await
        .unwrap();
    let keys = cache.tag_keys("t").await.unwrap();
    assert_eq!(keys, vec!["k".to_string()], "tag -> keys stays deduped");
    assert_eq!(
        cache.tags.get("t").unwrap().len(),
        1,
        "in-memory index deduped too"
    );
}

#[tokio::test]
async fn seed_inserts_and_stats_count() {
    let cache = TieredCache::new(1 << 20);
    let entries = vec![
        (
            "s1".to_string(),
            entry(b"a", &["seeded"], fresh_policy(60_000, 0)),
        ),
        (
            "s2".to_string(),
            entry(b"b", &["seeded"], fresh_policy(60_000, 0)),
        ),
    ];
    let n = cache.seed(entries).await.unwrap();
    assert_eq!(n, 2);
    assert!(matches!(
        cache.get("s1").await.unwrap(),
        Some(Hit::Fresh(_))
    ));
    assert!(matches!(
        cache.get("s2").await.unwrap(),
        Some(Hit::Fresh(_))
    ));
    let s = cache.stats();
    assert_eq!(s.hits, 2);
    assert_eq!(s.misses, 0);
    assert_eq!(s.entries_memory, 2);
    assert!(s.bytes_memory > 0);
    // Seeding does not count as traffic.
    assert_eq!(cache.stats().hits, 2);
}

// ---------------------------------------------------------------- Remote handler (L2-ish)

struct FakeRemote {
    store: dashmap::DashMap<String, Entry>,
    tags: dashmap::DashMap<String, Vec<String>>,
    sets: AtomicU64,
    gets: AtomicU64,
}

impl FakeRemote {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            store: dashmap::DashMap::new(),
            tags: dashmap::DashMap::new(),
            sets: AtomicU64::new(0),
            gets: AtomicU64::new(0),
        })
    }
}

#[async_trait::async_trait]
impl CacheHandler for FakeRemote {
    async fn get(&self, key: &str) -> oxide_cache::Result<Option<Entry>> {
        self.gets.fetch_add(1, Ordering::Relaxed);
        Ok(self.store.get(key).map(|e| e.clone()))
    }
    async fn set(&self, key: &str, entry: Entry) -> oxide_cache::Result<()> {
        self.sets.fetch_add(1, Ordering::Relaxed);
        for t in &entry.tags {
            let mut list = self.tags.entry(t.clone()).or_default();
            if !list.iter().any(|k| k == key) {
                list.push(key.to_string());
            }
        }
        self.store.insert(key.to_string(), entry);
        Ok(())
    }
    async fn delete(&self, key: &str) -> oxide_cache::Result<()> {
        self.store.remove(key);
        Ok(())
    }
    async fn tag_keys(&self, tag: &str) -> oxide_cache::Result<Vec<String>> {
        Ok(self.tags.get(tag).map(|l| l.clone()).unwrap_or_default())
    }
    async fn invalidate_tag(&self, tag: &str, soft: bool) -> oxide_cache::Result<usize> {
        let keys = self.tag_keys(tag).await?;
        for k in &keys {
            if soft {
                if let Some(mut e) = self.store.get_mut(k) {
                    let orig = e.policy.clone();
                    e.policy.ttl = Duration::ZERO;
                    e.policy.swr = Some(orig.swr.unwrap_or_default() + orig.ttl);
                }
            } else {
                self.store.remove(k);
            }
        }
        Ok(keys.len())
    }
    async fn publish_invalidation(&self, _tag: &str, _soft: bool) -> oxide_cache::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn remote_read_through_and_write_through() {
    let remote = FakeRemote::new();
    let mut cache = TieredCache::new(1 << 20);
    cache.remote = Some(remote.clone());
    let cache = Arc::new(cache);

    // Write-through: set reaches the remote handler.
    cache
        .set("r1", entry(b"remote", &["rt"], fresh_policy(60_000, 0)))
        .await
        .unwrap();
    assert_eq!(remote.sets.load(Ordering::Relaxed), 1);
    assert!(remote.store.contains_key("r1"));

    // Read-through from a cache with cold memory but the same remote.
    let mut cache2 = TieredCache::new(1 << 20);
    cache2.remote = Some(remote.clone());
    assert!(matches!(
        cache2.get("r1").await.unwrap(),
        Some(Hit::Fresh(_))
    ));
    assert_eq!(remote.gets.load(Ordering::Relaxed), 1);
    assert_eq!(
        cache2.stats().entries_memory,
        1,
        "remote hit promotes to memory"
    );

    // Hard invalidation removes from the remote too.
    let n = cache2.invalidate_tag("rt", false).await.unwrap();
    assert_eq!(n, 1);
    assert!(!remote.store.contains_key("r1"));
    assert!(cache2.get("r1").await.unwrap().is_none());
}

// ---------------------------------------------------------------- Single-flight

#[tokio::test]
async fn single_flight_coalesces_five_concurrent_acquires() {
    use tokio::sync::broadcast;
    let sf = SingleFlight::new();
    let map: Arc<dashmap::DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>>> = Arc::default();
    let barrier = Arc::new(tokio::sync::Barrier::new(5));
    let barrier2 = Arc::new(tokio::sync::Barrier::new(5));

    let mut tasks = Vec::new();
    for _ in 0..5 {
        let (b1, b2, map) = (barrier.clone(), barrier2.clone(), map.clone());
        tasks.push(tokio::spawn(async move {
            b1.wait().await;
            let slot = SingleFlight::new().acquire(&map, "render:/x").await;
            b2.wait().await; // all acquires done before anyone publishes
            slot
        }));
    }
    let mut owners = 0;
    let mut owner_tx = None;
    let mut followers = Vec::new();
    for t in tasks {
        match t.await.unwrap() {
            FlightSlot::Owner(tx) => {
                owners += 1;
                owner_tx = Some(tx);
            }
            FlightSlot::Follower(rx) => followers.push(rx),
        }
    }
    assert_eq!(owners, 1, "exactly one owner");
    assert_eq!(followers.len(), 4);

    // Owner publishes; every follower receives the owner's value.
    let winner = Arc::new(entry(b"winner-body", &[], Policy::default()));
    let n = sf
        .publish_and_cleanup(
            &map,
            "render:/x",
            FlightSlot::Owner(owner_tx.unwrap()),
            Some(winner.clone()),
        )
        .await;
    assert_eq!(n, 4, "all four followers were reached");
    for mut f in followers {
        let got = f.recv().await.expect("follower receives owner value");
        assert_eq!(got.body, b"winner-body");
    }
    assert!(map.is_empty(), "map cleaned up after publish");
}

#[tokio::test]
async fn single_flight_owner_cleanup_allows_next_owner() {
    use tokio::sync::broadcast;
    let sf = SingleFlight::new();
    let map: dashmap::DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>> = dashmap::DashMap::new();

    let first = sf.acquire(&map, "k").await;
    assert!(matches!(first, FlightSlot::Owner(_)));
    // A second acquire while the owner is in flight coalesces.
    assert!(matches!(
        sf.acquire(&map, "k").await,
        FlightSlot::Follower(_)
    ));

    // Owner fails: cleanup without publishing removes the entry.
    match first {
        FlightSlot::Owner(tx) => {
            sf.publish_and_cleanup(&map, "k", FlightSlot::Owner(tx), None)
                .await;
        }
        _ => unreachable!(),
    }
    assert!(map.is_empty());
    // Next acquire is a fresh owner (nobody is stuck as follower).
    assert!(matches!(sf.acquire(&map, "k").await, FlightSlot::Owner(_)));
}

#[tokio::test]
async fn single_flight_publish_without_followers_is_fine() {
    use tokio::sync::broadcast;
    let sf = SingleFlight::new();
    let map: dashmap::DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>> = dashmap::DashMap::new();
    let slot = sf.acquire(&map, "solo").await;
    if let FlightSlot::Owner(tx) = slot {
        let n = sf
            .publish_and_cleanup(
                &map,
                "solo",
                FlightSlot::Owner(tx),
                Some(Arc::new(entry(b"v", &[], Policy::default()))),
            )
            .await;
        assert_eq!(n, 0, "send with no followers fails silently");
    } else {
        panic!("first acquire must own");
    }
    assert!(map.is_empty());
}

// ---------------------------------------------------------------- HTTP semantics (§11.3)

#[test]
fn cache_control_strings() {
    let p = |ttl: u64, swr: Option<u64>, sie: Option<u64>| Policy {
        ttl: Duration::from_secs(ttl),
        swr: swr.map(Duration::from_secs),
        stale_if_error: sie.map(Duration::from_secs),
        soft_tags: true,
    };
    assert_eq!(cache_control(&p(0, None, None)), "no-store");
    assert_eq!(
        cache_control(&p(60, None, None)),
        "public, max-age=60, s-maxage=60"
    );
    assert_eq!(
        cache_control(&p(60, Some(30), None)),
        "public, max-age=60, s-maxage=60, stale-while-revalidate=30"
    );
    assert_eq!(
        cache_control(&p(60, Some(30), Some(86_400))),
        "public, max-age=60, s-maxage=60, stale-while-revalidate=30, stale-if-error=86400"
    );
    assert_eq!(
        cache_control(&p(0, Some(30), None)),
        "no-store",
        "ttl=0 wins over everything"
    );
}

#[test]
fn conditional_response_etag_paths() {
    let e = |etag: &str| Entry {
        body: vec![],
        status: 200,
        headers: vec![],
        tags: vec![],
        policy: fresh_policy(60_000, 0),
        stored_at: now_ms(),
        etag: etag.into(),
    };
    let inm = |v: &str| vec![("if-none-match".to_string(), v.to_string())];

    assert_eq!(
        conditional_response(&inm("\"abc\""), &e("\"abc\"")),
        Some(304),
        "exact match"
    );
    assert_eq!(
        conditional_response(&inm("\"zzz\""), &e("\"abc\"")),
        None,
        "no match"
    );
    assert_eq!(
        conditional_response(&inm("\"a\", \"b\", \"c\""), &e("\"b\"")),
        Some(304),
        "list match"
    );
    assert_eq!(
        conditional_response(&inm("*"), &e("\"abc\"")),
        Some(304),
        "star matches anything"
    );
    assert_eq!(
        conditional_response(&inm("W/\"abc\""), &e("\"abc\"")),
        Some(304),
        "weak validator matches strongly-tagged entry"
    );
    assert_eq!(
        conditional_response(&inm("\"abc\""), &e("W/\"abc\"")),
        Some(304),
        "weak entry etag still matches"
    );
    assert_eq!(
        conditional_response(&inm("\"ABC\""), &e("\"abc\"")),
        None,
        "etag comparison is case-sensitive"
    );
    assert_eq!(
        conditional_response(&[], &e("\"abc\"")),
        None,
        "no conditional headers"
    );
}

#[test]
fn conditional_response_if_modified_since() {
    let lm = now_ms().saturating_sub(60_000); // unix-ms, documented test format
    let e = Entry {
        body: vec![],
        status: 200,
        headers: vec![("Last-Modified".into(), lm.to_string())],
        tags: vec![],
        policy: fresh_policy(60_000, 0),
        stored_at: now_ms(),
        etag: String::new(),
    };
    let ims = |v: &str| vec![("if-modified-since".to_string(), v.to_string())];
    assert_eq!(
        conditional_response(&ims(&lm.to_string()), &e),
        Some(304),
        "not modified since"
    );
    let older = (lm - 120_000).to_string();
    assert_eq!(
        conditional_response(&ims(&older), &e),
        None,
        "entry modified after the IMS date -> full response"
    );
    let newer = (lm + 5_000).to_string(); // 5 s after the entry's Last-Modified
    assert_eq!(
        conditional_response(&ims(&newer), &e),
        Some(304),
        "entry last modified before the IMS date -> not modified -> 304"
    );
    // Entry without Last-Modified can never answer IMS.
    let e2 = Entry {
        headers: vec![],
        ..e.clone()
    };
    assert_eq!(conditional_response(&ims(&lm.to_string()), &e2), None);
    // If-None-Match takes precedence over If-Modified-Since (RFC 9110 §13.1.3).
    let both = vec![
        ("if-none-match".to_string(), "\"zzz\"".to_string()),
        ("if-modified-since".to_string(), lm.to_string()),
    ];
    assert_eq!(conditional_response(&both, &e), None);
}

// ---------------------------------------------------------------- FetchMemo (L2)

#[test]
fn fetch_memo_roundtrip_and_body_sensitivity() {
    let memo = FetchMemo::default();
    let k = oxide_cache::memo_key("GET", "https://api/x", b"");
    assert_eq!(k, oxide_cache::memo_key("GET", "https://api/x", b""));
    assert_ne!(k, oxide_cache::memo_key("GET", "https://api/x", b"body"));
    assert_ne!(k, oxide_cache::memo_key("POST", "https://api/x", b""));

    memo.insert(k.clone(), Bytes::from_static(b"resp1"));
    assert_eq!(
        memo.get_for_request("GET", "https://api/x", b"").unwrap(),
        Bytes::from_static(b"resp1")
    );
    assert!(memo
        .get_for_request("GET", "https://api/x", b"other")
        .is_none());
    assert!(memo.get_for_request("POST", "https://api/x", b"").is_none());
}

// ---------------------------------------------------------------- JSON shape sanity

#[test]
fn entry_and_policy_serialize_with_camel_case_free_stable_shape() {
    // Round-trip through JSON (used by the disk tier + diagnostics surfaces).
    let e = entry(b"x", &["t"], fresh_policy(1000, 500));
    let v = json!(&e);
    let back: Entry = serde_json::from_value(v).unwrap();
    assert_eq!(
        json!(back),
        json!(e),
        "serialization roundtrip preserves the entry"
    );
}
