//! V2-B05 TDD (red-first): the tests for the 128-bit [`KeyHash`] (D24) and
//! the D25 tag-index contracts — O(1) tag insert, the reverse key-to-tags
//! lookup, and no JSON round-trip on the disk tier.
//!
//! Written BEFORE the implementation (the directive's red step): the
//! `KeyHash` type and the [`TieredCache::tags_of`] read seam do not exist
//! yet, so this module fails to compile — that compile failure IS the red
//! state recorded in `scripts/gates/key_hash_red.log`. The two D25
//! regression pins (O(1) insert, no JSON round-trip) target behavior the
//! B05/B06 halves already landed; they pin it against regression while the
//! red-first tests drive the D24 delta.
//!
//! Spec refs: V2.1.3 perf #6 (`canonical` allocates a hex String) and #7
//! (tag index `Vec` + `iter().any` + JSON round trip); decisions D24/D25.

use crate::key::KeyHash;
use crate::keyhash::key_of;
use crate::tiers::{DiskTier, now_ms};
use crate::{CacheKey, Entry, Layer, Policy, TieredCache};
use redb::ReadableDatabase;
use redb::ReadableTable;
use std::time::{Duration, Instant};

/// The v1 JSON tag table (the pre-B06 shape): a JSON key list per tag.
/// The test re-declares it by name to prove, at the raw-table level, that
/// inserts never write a JSON list (D25).
const V1_JSON_TAGS: redb::TableDefinition<&[u8], &[u8]> =
    redb::TableDefinition::new("oxide-fw-cache-tags");
/// The v2 multimap (the landed B06 shape): set-valued tag → keys.
const V2_TAG_KEYS: redb::MultimapTableDefinition<&str, &str> =
    redb::MultimapTableDefinition::new("tag_keys_v2");

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().map(std::string::ToString::to_string).collect(),
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: now_ms(),
        etag: "\"v1\"".into(),
    }
}

// ----------------------------------------------------------------- D24: KeyHash

#[test]
fn key_hash_is_128_bit_truncated_blake3() {
    // `From<&[u8]>` is blake3 truncated to 128 bits — 16 raw bytes.
    let from_trait = KeyHash::from(b"oxide".as_slice());
    let full = blake3::hash(b"oxide");
    let mut expected = [0u8; 16];
    expected.copy_from_slice(&full.as_bytes()[..16]);
    assert_eq!(from_trait.as_bytes(), &expected);
    assert_eq!(from_trait.as_bytes().len(), 16);

    // Deterministic: two constructions of the same key agree.
    let key = CacheKey {
        layer: Layer::L0,
        route: "/r".into(),
        params: "p".into(),
        vary: "v".into(),
        build_id: "b".into(),
    };
    assert_eq!(key.canonical(), key.canonical());

    // And the cache-key digest is blake3 over the exact NUL-separated
    // byte sequence, truncated to 128 bits.
    let mut h = blake3::Hasher::new();
    for (i, part) in ["L0", "/r", "p", "v", "b"].iter().enumerate() {
        if i > 0 {
            h.update(&[0]);
        }
        h.update(part.as_bytes());
    }
    let mut expected2 = [0u8; 16];
    expected2.copy_from_slice(&h.finalize().as_bytes()[..16]);
    assert_eq!(key.canonical().as_bytes(), &expected2);

    // NUL separators keep the encoding prefix-free: ("ab","c") and
    // ("a","bc") must not collide, and the layer participates.
    assert_ne!(
        CacheKey {
            layer: Layer::L0,
            route: "ab".into(),
            params: "c".into(),
            vary: String::new(),
            build_id: String::new(),
        }
        .canonical(),
        CacheKey {
            layer: Layer::L0,
            route: "a".into(),
            params: "bc".into(),
            vary: String::new(),
            build_id: String::new(),
        }
        .canonical()
    );
    assert_ne!(
        key.canonical(),
        CacheKey {
            layer: Layer::L1,
            ..key.clone()
        }
        .canonical(),
        "the layer participates in the digest"
    );
}

#[test]
fn key_hash_display_is_hex() {
    let k = KeyHash::of_parts("L0", "routes/blog/[slug]/page", "{}", "", "build");
    let s = k.to_string();
    assert_eq!(s.len(), 32, "128 bits are 32 hex chars (D24)");
    assert!(s.bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(
        s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "the hex form is lowercase"
    );
    // `Display` and `hex()` agree, and `hex()` round-trips through the
    // stack `KeyStr`'s raw 16-byte view.
    assert_eq!(k.hex().as_str(), s);
    assert_eq!(k.hex().as_bytes128(), *k.as_bytes());
    // One digest, one display: `KeyHash`'s hex IS the `key_of` hex — the
    // two forms cannot drift.
    assert_eq!(
        s,
        key_of("L0", "routes/blog/[slug]/page", "{}", "", "build").as_str()
    );
}

// ------------------------------------------------- D25: the tag index contracts

#[tokio::test]
async fn tag_index_insert_is_o1() {
    let cache = TieredCache::new_memory();
    // Build a 10k-member tag through the real write path.
    for i in 0..10_000u32 {
        cache
            .set(&format!("routes/{i:05}"), entry(b"x", &["hot-tag"]))
            .await
            .expect("set");
    }
    assert_eq!(cache.tags.get("hot-tag").map(|s| s.len()), Some(10_000));
    // The measured operation: 10k FURTHER inserts into that same tag.
    // O(1) amortized (P-18b/D25): the pre-B05 `Vec` + `iter().any`
    // counterfactual is a linear scan per insert (~100× this budget).
    let start = Instant::now();
    for i in 0..10_000u32 {
        cache
            .tags
            .entry("hot-tag".to_string())
            .or_default()
            .insert(format!("extra/{i:05}"));
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(10),
        "10k inserts into a 10k-member tag took {elapsed:?} — a linear scan is showing"
    );
    assert_eq!(cache.tags.get("hot-tag").map(|s| s.len()), Some(20_000));
    // The reverse index still answers for the preloaded keys.
    assert_eq!(cache.tags_of("routes/00042"), vec!["hot-tag".to_string()]);
}

#[tokio::test]
async fn tag_index_reverse_lookup() {
    let cache = TieredCache::new_memory();
    cache
        .set("k1", entry(b"1", &["alpha", "beta"]))
        .await
        .expect("set");
    cache.set("k2", entry(b"2", &["beta"])).await.expect("set");
    // Reverse lookup: key → the tags it was set under (sorted, deterministic).
    assert_eq!(
        cache.tags_of("k1"),
        vec!["alpha".to_string(), "beta".to_string()]
    );
    assert_eq!(cache.tags_of("k2"), vec!["beta".to_string()]);
    assert_eq!(
        cache.tags_of("never-set"),
        Vec::<String>::new(),
        "no tags for a key never set"
    );
    // Retag: a re-set whose tag list changed drops stale memberships.
    cache
        .set("k1", entry(b"1", &["alpha", "gamma"]))
        .await
        .expect("reset");
    assert_eq!(
        cache.tags_of("k1"),
        vec!["alpha".to_string(), "gamma".to_string()]
    );
    assert!(
        !cache.tags.get("beta").is_some_and(|s| s.contains("k1")),
        "the stale membership must not survive the retag"
    );
    // Eager delete: deleting the key cleans both indexes.
    cache.delete("k2").await.expect("delete");
    assert_eq!(
        cache.tags_of("k2"),
        Vec::<String>::new(),
        "the reverse index is cleaned eagerly"
    );
    assert!(
        !cache.tags.get("beta").is_some_and(|s| s.contains("k2")),
        "a deleted key never dangles in a tag set"
    );
}

#[test]
fn tag_index_no_json_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = DiskTier::open(dir.path().join("c")).expect("open");
    disk.set("k1", &entry(b"body-1", &["alpha", "beta"]))
        .expect("set");
    disk.set("k2", &entry(b"body-2", &["beta"])).expect("set");
    // The multimap answers the tag query.
    let mut beta = disk.tag_keys("beta").expect("tag keys");
    beta.sort();
    assert_eq!(beta, vec!["k1".to_string(), "k2".to_string()]);

    // Raw-table proof: the tier releases its redb handle, and the v1 JSON
    // tag table holds ZERO rows — no JSON list was encoded on insert
    // (D25: no read-modify-write of a whole list per tag). The
    // memberships live only in the multimap.
    drop(disk);
    let db = redb::Database::open(dir.path().join("c/index.redb")).expect("reopen");
    let rtxn = db.begin_read().expect("read txn");
    let v1 = rtxn.open_table(V1_JSON_TAGS).expect("v1 table");
    let mut v1_rows = v1.iter().expect("v1 iter");
    assert!(
        v1_rows.next().is_none(),
        "the v1 JSON tag table must stay empty — inserts never write a JSON list"
    );
    let mm = rtxn.open_multimap_table(V2_TAG_KEYS).expect("multimap");
    let members: Vec<String> = mm
        .get("beta")
        .expect("get")
        .map(|row| row.expect("row").value().to_string())
        .collect();
    assert_eq!(members.len(), 2, "both memberships live in the multimap");
}
