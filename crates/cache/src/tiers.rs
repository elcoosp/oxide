//! Tiered store (spec §11.2): memory (in-house weighted LRU) → local disk
//! (`redb` index + zstd blob files) → pluggable remote [`CacheHandler`].
//!
//! # Memory tier (moka, spec §11.2)
//!
//! L0/L1 memory tier is a [`moka::sync::Cache`] with a weigher of `entry_weight`;
//! moka enforces `max_capacity` by weight and uses its TinyLFU admission policy
//! (spec §11.2: "moka, TinyLFU, weighted by bytes"). Entries heavier than the cap
//! are refused by moka's own admission rule, matching the previous policy.
//! moka amortizes eviction internally; `run_pending_tasks()` is called only on
//! `len`/`is_empty`/`total_bytes` so those reads stay exact (P-17 removed the
//! forced pass on the hot `insert` path).
//!
//! Entry byte weight = `body.len() + Σ(header key.len + value.len) + 32` (the fixed
//! 32 bytes roughly cover status/etag/timestamp bookkeeping). The canonical cache key
//! length is not counted (it is a constant per layer in practice).

use crate::observer::{CacheEvent, CacheObserver, CacheOp};
use crate::{CacheHandler, Entry, Hit, Layer, Result, err};
use dashmap::DashMap;
use moka::sync::Cache as MokaCache;
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use redb::ReadableDatabase;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Current wall clock in unix milliseconds (the crate-wide time base for §11.3).
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// V2-B06: disk-tier tag → keys as a redb multimap (set-valued, one insert
/// per membership — no JSON read-modify-write of a whole list per tag).
const TAG_KEYS: redb::MultimapTableDefinition<&str, &str> =
    redb::MultimapTableDefinition::new("tag_keys_v2");
/// V2-B06: schema marker so the v1 → v2 migration runs exactly once.
const META_TABLE: redb::TableDefinition<&str, u64> = redb::TableDefinition::new("meta");
const META_SCHEMA_VERSION: u64 = 2;

/// Byte weight of an entry (see module docs): body + header key/value bytes + 32.
pub(crate) fn entry_weight(e: &Entry) -> u64 {
    e.body.len() as u64
        + e.headers
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>() as u64
        + 32
}

// --------------------------------------------------------------------------- MemoryTier

/// moka-backed L0/L1 memory tier (spec §11.2: moka, TinyLFU, weighted by bytes).
pub struct MemoryTier {
    pub max_bytes: u64,
    cache: MokaCache<String, Arc<Entry>>,
}

impl MemoryTier {
    pub fn new(max_bytes: u64) -> Self {
        let cache = MokaCache::builder()
            .weigher(|_k: &String, v: &Arc<Entry>| {
                let w = entry_weight(v);
                if w > u32::MAX as u64 {
                    u32::MAX
                } else {
                    w as u32
                }
            })
            .max_capacity(max_bytes)
            .build();
        Self { max_bytes, cache }
    }

    /// Get a cached entry.
    pub fn get(&self, key: &str) -> Option<Arc<Entry>> {
        self.cache.get(key)
    }

    /// Insert (or replace) an entry; moka evicts as needed to stay under the byte cap.
    /// Entries heavier than the cap are refused (moka admission rule).
    pub fn insert(&self, key: &str, entry: Entry) {
        let weight = entry_weight(&entry);
        if weight > self.max_bytes {
            return;
        }
        self.cache.insert(key.to_string(), Arc::new(entry));
        // P-17: moka amortizes maintenance internally; forcing a full pass on
        // every insert serialized concurrent writers. `len`/`total_bytes` still
        // flush before reading so metrics stay exact.
    }

    /// Remove a key; returns the stored entry if it was present.
    pub fn remove(&self, key: &str) -> Option<Arc<Entry>> {
        self.cache.remove(key)
    }

    /// Drop everything.
    pub fn clear(&self) {
        self.cache.invalidate_all();
        self.cache.run_pending_tasks();
    }

    /// Number of entries stored.
    pub fn len(&self) -> usize {
        self.cache.run_pending_tasks();
        self.cache.entry_count() as usize
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.cache.run_pending_tasks();
        self.cache.entry_count() == 0
    }

    /// Total stored byte weight (≤ `max_bytes` after any insert completes).
    pub fn total_bytes(&self) -> u64 {
        self.cache.run_pending_tasks();
        self.cache.weighted_size()
    }
}

// --------------------------------------------------------------------------- DiskTier

const ENTRY_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("oxide-fw-cache-entries");
const TAG_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("oxide-fw-cache-tags");

/// Index row: blob path (relative to root), compressed size, tags, stored_at and the
/// blake3 of the *uncompressed* serialized entry (verified on load, §17.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiskIndexEntry {
    path: String,
    size: u64,
    tags: Vec<String>,
    stored_at: u64,
    hash: String,
}

fn dberr(e: impl std::fmt::Display) -> OxideError {
    OxideError(Box::new(
        Diagnostic::error("CAC-DB", format!("redb: {e}"))
            .with_fix("check disk space/permissions for the cache root"),
    ))
}

fn jsonerr(e: serde_json::Error) -> OxideError {
    err("CAC-JSON", format!("index codec: {e}"))
}

fn ioerr(e: std::io::Error) -> OxideError {
    err("CAC-IO", format!("disk tier: {e}"))
}

/// Disk tier: redb index + zstd blob files under `root`.
///
/// Layout: `root/blobs/<first-2-chars-of-blob-name>/<blob-name>.bin`; the blob is the
/// zstd-compressed serialization of an [`Entry`]. PERF-L1 (readiness): the body is
/// stored RAW next to the metadata JSON ([`BLOB_MAGIC`] header) — the previous
/// format ran the whole entry through `serde_json`, which expands a binary body
/// into a JSON number array (3-4× the bytes) before zstd ever sees it. On load the
/// stored blake3 is verified; any mismatch/decode failure/missing blob deletes the
/// entry and reports a miss (silent self-healing eviction — corruption must never
/// serve, §17.2). Legacy (JSON-body) blobs still decode via the fallback path.
pub struct DiskTier {
    pub root: PathBuf,
    index: Database,
    /// S-08: total compressed bytes currently indexed (admission + prune base).
    cur_bytes: AtomicU64,
    /// S-08: budget — the store prunes oldest-first past this total.
    pub max_bytes: u64,
    /// S-08: reject a single entry larger than this before writing it.
    pub max_entry: u64,
    /// PERF-L1 (readiness): in-memory `((stored_at, key) → compressed size)`
    /// order index. The old prune did a FULL TABLE SCAN with a JSON parse per
    /// row on EVERY eviction — O(N²) for an N-entry prune burst. The map is
    /// maintained on set/delete/open; picking a victim is `first_key_value`.
    /// Bounded by the entry count (keys ~120 bytes) — acceptable next to the
    /// redb index itself.
    order: std::sync::Mutex<std::collections::BTreeMap<(u64, String), u64>>,
    /// F51: evictions the S-08 prune completed (saturating — the F30
    /// counter rule: a counter that reaches `u64::MAX` reads "at or past
    /// the bound", never a silently-restarted total).
    evictions: AtomicU64,
    /// F51: the compressed blob bytes those evictions freed (saturating,
    /// the same rule — the S-08 budget's own accounting unit).
    evicted_bytes: AtomicU64,
    /// F51: the eviction event's push half — the observer the F28 seam
    /// attached. The tiered cache wires its own observer down here at
    /// attach (both builder orders); a standalone tier attaches through
    /// [`DiskTier::with_eviction_observer`]. `None` until attached: the
    /// counters ALWAYS answer (the pull half is unconditional), the event
    /// rides only when a listener exists — the seam's no-observer path is
    /// one branch.
    eviction_observer: OnceLock<Arc<dyn CacheObserver>>,
}

/// PERF-L1: blob v2 magic — `magic || u32 BE meta_len || meta_json || raw body`.
const BLOB_MAGIC: &[u8; 4] = b"OXD1";

/// S-08 default disk budget (1 GiB) and per-entry cap (32 MiB). A project can
/// override via config; these bounds stop attacker-minted keys filling the disk.
pub const DEFAULT_DISK_MAX_BYTES: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_DISK_MAX_ENTRY: u64 = 32 * 1024 * 1024;

/// V2-B06: migrate the v1 `TAG_TABLE` (tag → JSON key list) into the
/// `TAG_KEYS` multimap, then record `schema=2` in the meta table so it runs
/// exactly once. Fresh stores carry no v1 rows, so this is a no-op for them.
fn migrate_tag_table_v1_to_v2(index: &Database) -> Result<()> {
    let schema: Option<u64> = {
        let rtxn = index.begin_read().map_err(dberr)?;
        let meta = rtxn.open_table(META_TABLE).map_err(dberr)?;
        meta.get("schema").map_err(dberr)?.map(|g| g.value())
    };
    if schema == Some(META_SCHEMA_VERSION) {
        return Ok(());
    }
    let txn = index.begin_write().map_err(dberr)?;
    {
        // One binding for the whole txn: redb forbids opening the same table
        // twice. Read every v1 row first (one pass), then consume them.
        let mut old = txn.open_table(TAG_TABLE).map_err(dberr)?;
        let mut pairs: Vec<(String, Vec<String>)> = Vec::new();
        for row in old.iter().map_err(dberr)?.flatten() {
            let tag = String::from_utf8_lossy(row.0.value()).into_owned();
            // A corrupt v1 row migrates as an EMPTY list (the house
            // convention: the empty-set behavior is unchanged and
            // self-healing) — but the invalidation gap must be VISIBLE,
            // not silent.
            let list: Vec<String> = match serde_json::from_slice(row.1.value()) {
                Ok(list) => list,
                Err(e) => {
                    tracing::warn!(
                        code = "CACHE-CODEC",
                        tag = %tag,
                        error = %e,
                        "a v1 tag row did not parse as a JSON key list; migrating it as empty"
                    );
                    Vec::new()
                }
            };
            pairs.push((tag, list));
        }
        let mut mm = txn.open_multimap_table(TAG_KEYS).map_err(dberr)?;
        for (tag, keys) in &pairs {
            for key in keys {
                // Multimap insert is set-valued: duplicates are no-ops, so a
                // partial prior migration cannot corrupt the table.
                mm.insert(tag.as_str(), key.as_str()).map_err(dberr)?;
            }
        }
        drop(mm);
        // V1 rows are consumed; drop them so the old table stays empty.
        for (tag, _) in &pairs {
            old.remove(tag.as_bytes()).map_err(dberr)?;
        }
        let mut meta = txn.open_table(META_TABLE).map_err(dberr)?;
        meta.insert("schema", META_SCHEMA_VERSION).map_err(dberr)?;
    }
    txn.commit().map_err(dberr)?;
    Ok(())
}

impl DiskTier {
    /// Open (creating dirs + redb index as needed) at `root`.
    pub fn open(root: std::path::PathBuf) -> Result<Self> {
        Self::open_with_budget(root, DEFAULT_DISK_MAX_BYTES, DEFAULT_DISK_MAX_ENTRY)
    }

    /// Open with an explicit disk budget + per-entry cap.
    pub fn open_with_budget(
        root: std::path::PathBuf,
        max_bytes: u64,
        max_entry: u64,
    ) -> Result<Self> {
        std::fs::create_dir_all(&root).map_err(|e| {
            OxideError(Box::new(
                Diagnostic::error("CAC-IO", format!("create {}: {e}", root.display()))
                    .with_fix("check that the cache root is writable"),
            ))
        })?;
        std::fs::create_dir_all(root.join("blobs")).map_err(ioerr)?;
        let index = Database::create(root.join("index.redb")).map_err(dberr)?;
        let txn = index.begin_write().map_err(dberr)?;
        {
            // Ensure all tables exist so reads never fail on a fresh store.
            txn.open_table(ENTRY_TABLE).map_err(dberr)?;
            txn.open_table(TAG_TABLE).map_err(dberr)?;
            txn.open_multimap_table(TAG_KEYS).map_err(dberr)?;
            txn.open_table(META_TABLE).map_err(dberr)?;
        }
        txn.commit().map_err(dberr)?;
        // V2-B06: migrate the v1 JSON-list tag table into the multimap once.
        migrate_tag_table_v1_to_v2(&index)?;
        // S-08: seed the current-size counter from the existing index, and the
        // PERF-L1 stored_at order index alongside it (one scan instead of a
        // per-eviction scan later).
        let mut cur = 0u64;
        let mut order = std::collections::BTreeMap::new();
        {
            let rtxn = index.begin_read().map_err(dberr)?;
            let table = rtxn.open_table(ENTRY_TABLE).map_err(dberr)?;
            let iter = table.iter().map_err(dberr)?;
            for row in iter {
                if let Ok((k, v)) = row
                    && let Ok(idx) = serde_json::from_slice::<DiskIndexEntry>(v.value())
                {
                    cur = cur.saturating_add(idx.size);
                    order.insert(
                        (
                            idx.stored_at,
                            String::from_utf8_lossy(k.value()).into_owned(),
                        ),
                        idx.size,
                    );
                }
            }
        }
        Ok(Self {
            root,
            index,
            cur_bytes: AtomicU64::new(cur),
            max_bytes,
            max_entry,
            order: std::sync::Mutex::new(order),
            evictions: AtomicU64::new(0),
            evicted_bytes: AtomicU64::new(0),
            eviction_observer: OnceLock::new(),
        })
    }

    /// Filesystem-safe blob file name for a key: plain `[A-Za-z0-9_-]` keys (≤120
    /// chars) are used verbatim; anything else is replaced by a content-derived name.
    fn blob_name(key: &str) -> String {
        let plain = !key.is_empty()
            && key.len() <= 120
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if plain {
            format!("{key}.bin")
        } else {
            format!("h{}.bin", &blake3::hash(key.as_bytes()).to_string()[..32])
        }
    }

    fn blob_path(&self, key: &str) -> PathBuf {
        let name = Self::blob_name(key);
        self.root
            .join("blobs")
            .join(&name[..2.min(name.len())])
            .join(name)
    }

    /// Store an entry: compress with zstd, write the blob (tmp + rename), update the
    /// index row and the tag → keys table.
    pub fn set(&self, key: &str, entry: &Entry) -> Result<()> {
        // S-08: refuse an entry over the per-entry cap before touching disk.
        let raw_len = entry.body.len() as u64;
        if raw_len > self.max_entry {
            return Err(err(
                "CAC-DISK",
                format!("entry {raw_len} exceeds per-entry cap {}", self.max_entry),
            ));
        }
        // PERF-L1: blob v2 — raw body appended after the metadata JSON, so the
        // body never passes through serde_json (a binary body becomes a JSON
        // number array, 3-4× its bytes).
        let mut meta = entry.clone();
        meta.body = Vec::new();
        let meta_json = serde_json::to_vec(&meta).map_err(jsonerr)?;
        let mut serialized =
            Vec::with_capacity(BLOB_MAGIC.len() + 4 + meta_json.len() + entry.body.len());
        serialized.extend_from_slice(BLOB_MAGIC);
        serialized.extend_from_slice(&(meta_json.len() as u32).to_be_bytes());
        serialized.extend_from_slice(&meta_json);
        serialized.extend_from_slice(&entry.body);
        let hash = blake3::hash(&serialized).to_string();
        let compressed = zstd::bulk::compress(&serialized, 3)
            .map_err(|e| err("CAC-ZSTD", format!("compress: {e}")))?;
        // The previous row's size + stored_at, so the byte counter and the
        // order index stay exact on replace.
        let prev = self.indexed_meta(key);
        let prev_size = prev.map(|(s, _)| s).unwrap_or(0);
        let path = self.blob_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ioerr)?;
        }
        let tmp = path.with_extension("bin.tmp");
        std::fs::write(&tmp, &compressed).map_err(ioerr)?;
        std::fs::rename(&tmp, &path).map_err(ioerr)?;

        let idx = DiskIndexEntry {
            path: path
                .strip_prefix(&self.root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned(),
            size: compressed.len() as u64,
            tags: entry.tags.clone(),
            stored_at: entry.stored_at,
            hash,
        };
        let txn = self.index.begin_write().map_err(dberr)?;
        {
            let mut t = txn.open_table(ENTRY_TABLE).map_err(dberr)?;
            let idx_bytes = serde_json::to_vec(&idx).map_err(jsonerr)?;
            t.insert(key.as_bytes(), idx_bytes.as_slice())
                .map_err(dberr)?;
            // V2-B06: one multimap insert per tag — set-valued (idempotent),
            // replacing the v1 JSON read-modify-write of the whole key list.
            let mut mm = txn.open_multimap_table(TAG_KEYS).map_err(dberr)?;
            for tag in &entry.tags {
                mm.insert(tag.as_str(), key).map_err(dberr)?;
            }
        }
        txn.commit().map_err(dberr)?;
        // S-08: update the byte counter and prune oldest-first past the budget.
        // PERF-L1: maintain the order index (drop the previous placement).
        let new_size = compressed.len() as u64;
        if let Some((_, prev_stored)) = prev {
            self.order_forget_placement(key, prev_stored);
        }
        self.order_record(key, entry.stored_at, new_size);
        self.cur_bytes.fetch_sub(prev_size, Ordering::Relaxed);
        let after = self.cur_bytes.fetch_add(new_size, Ordering::Relaxed) + new_size;
        if after > self.max_bytes {
            self.prune_oldest_until(self.max_bytes * 9 / 10);
        }
        Ok(())
    }

    /// PERF-L1: forget a specific (stored_at, key) placement.
    fn order_forget_placement(&self, key: &str, stored_at: u64) {
        let mut order = self.order.lock().unwrap_or_else(|p| p.into_inner());
        order.remove(&(stored_at, key.to_string()));
    }

    /// S-08: the indexed (compressed size, stored_at) of `key`, or `None` when absent.
    fn indexed_meta(&self, key: &str) -> Option<(u64, u64)> {
        let Ok(txn) = self.index.begin_read() else {
            return None;
        };
        let Ok(table) = txn.open_table(ENTRY_TABLE) else {
            return None;
        };
        match table.get(key.as_bytes()) {
            Ok(Some(v)) => serde_json::from_slice::<DiskIndexEntry>(v.value())
                .ok()
                .map(|i| (i.size, i.stored_at)),
            _ => None,
        }
    }

    /// PERF-L1: record the (stored_at, key) order entry for `key`. The caller
    /// forgets any previous placement first (it knows the old stored_at).
    fn order_record(&self, key: &str, stored_at: u64, size: u64) {
        let mut order = self.order.lock().unwrap_or_else(|p| p.into_inner());
        order.insert((stored_at, key.to_string()), size);
    }

    /// PERF-L1: forget `key`'s order entry (on delete).
    fn order_forget(&self, key: &str) {
        let mut order = self.order.lock().unwrap_or_else(|p| p.into_inner());
        let stale: Vec<(u64, String)> = order.keys().filter(|sk| sk.1 == key).cloned().collect();
        for sk in stale {
            order.remove(&sk);
        }
    }

    /// S-08: evict least-recently-stored entries until the total is under
    /// `target`. Ordering is by `stored_at` ascending (LRU by write time —
    /// the durable proxy the GC already tracks). PERF-L1: victims come from
    /// the in-memory order index (`first_key_value`), not a full redb scan
    /// with a JSON parse per row per eviction.
    ///
    /// F51: every completed victim is a first-class cache event — the
    /// saturating counters answer unconditionally (the pull half), and
    /// when the push half is attached the row rides the F28 seam as
    /// [`CacheOp::Evict`] with the layer and the freed size.
    fn prune_oldest_until(&self, target: u64) {
        loop {
            if self.cur_bytes.load(Ordering::Relaxed) <= target {
                return;
            }
            // PERF-L1: the victim and its compressed size ride ONE lock
            // pass (F51: the eviction row needs the size the store freed).
            let victim = self
                .order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .next()
                .map(|((_, k), size)| (k.clone(), *size));
            let Some((key, size)) = victim else {
                return;
            };
            // `delete` decrements `cur_bytes` and forgets the order entry itself.
            if self.delete(&key).is_err() {
                // Never spin on an undecodable victim: forget it locally.
                self.order_forget(&key);
                if self.cur_bytes.load(Ordering::Relaxed) > target {
                    continue;
                }
                return;
            }
            // F51: the eviction is a completed operation. The counters
            // saturate (the F30 rule); the event is delivered AFTER the
            // victim's delete completed — the observer contract's
            // "completed operations only".
            crate::metrics::saturating_add(&self.evictions, 1);
            crate::metrics::saturating_add(&self.evicted_bytes, size);
            if let Some(observer) = self.eviction_observer.get() {
                observer.on_event(&CacheEvent {
                    key: &key,
                    op: CacheOp::Evict {
                        layer: Layer::L1,
                        bytes: size,
                    },
                });
            }
        }
    }

    /// F51: evictions the S-08 prune completed since the tier opened.
    /// An explicit delete and a corrupt-miss self-heal are not here —
    /// the caller asked for the first, the second is §17.2's documented
    /// silent self-healing; only the tier's own budget prune counts.
    #[must_use]
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    /// F51: the compressed blob bytes those evictions freed — the tier's
    /// own accounting unit, the unit the S-08 budget itself speaks.
    #[must_use]
    pub fn evicted_bytes(&self) -> u64 {
        self.evicted_bytes.load(Ordering::Relaxed)
    }

    /// F51: attach the eviction event's push half to a STANDALONE tier —
    /// the composition arm for a tier the host opened with an explicit
    /// budget and uses outside a [`TieredCache`]. The tiered cache wires
    /// its own observer down through [`TieredCache::with_observer`]
    /// (both builder orders); this builder is the tier's own arm.
    /// Consuming, pre-`Arc` — the same shape every builder in the crate
    /// rides.
    #[must_use = "the observer attaches through the builder chain; dropping it loses the seam"]
    pub fn with_eviction_observer(self, observer: Arc<dyn CacheObserver>) -> Self {
        // A fresh tier attaches exactly once (the consuming builder runs
        // pre-`Arc`, pre-share); the `Err` arm — "already set" — would
        // mean the same Arc raced itself, so the first attach stands.
        let _ = self.eviction_observer.set(observer);
        self
    }

    /// Load an entry; corrupt/missing blobs are deleted and reported as a miss.
    pub fn get(&self, key: &str) -> Result<Option<Entry>> {
        let idx = {
            let txn = self.index.begin_read().map_err(dberr)?;
            let table = txn.open_table(ENTRY_TABLE).map_err(dberr)?;
            match table.get(key.as_bytes()).map_err(dberr)? {
                Some(v) => serde_json::from_slice::<DiskIndexEntry>(v.value()).map_err(jsonerr)?,
                None => return Ok(None),
            }
        };
        let raw = match std::fs::read(self.root.join(&idx.path)) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.delete(key)?;
                return Ok(None);
            }
            Err(e) => return Err(ioerr(e)),
        };
        let decoded = match zstd::stream::decode_all(raw.as_slice()) {
            Ok(d) => d,
            Err(_) => {
                self.delete(key)?; // corrupt stream — silent miss (§17.2)
                return Ok(None);
            }
        };
        if blake3::hash(&decoded).to_string() != idx.hash {
            self.delete(key)?; // hash mismatch — silent miss (§17.2)
            return Ok(None);
        }
        // PERF-L1: blob v2 (raw body) first, legacy JSON-entry fallback.
        if decoded.len() >= BLOB_MAGIC.len() + 4 && &decoded[..BLOB_MAGIC.len()] == BLOB_MAGIC {
            let meta_len = u32::from_be_bytes(
                decoded[BLOB_MAGIC.len()..BLOB_MAGIC.len() + 4]
                    .try_into()
                    .unwrap_or([0; 4]),
            ) as usize;
            let body_start = BLOB_MAGIC.len() + 4 + meta_len;
            if body_start <= decoded.len()
                && let Ok(mut entry) =
                    serde_json::from_slice::<Entry>(&decoded[BLOB_MAGIC.len() + 4..body_start])
            {
                entry.body = decoded[body_start..].to_vec();
                return Ok(Some(entry));
            }
            self.delete(key)?;
            return Ok(None);
        }
        match serde_json::from_slice::<Entry>(&decoded) {
            Ok(entry) => Ok(Some(entry)),
            Err(_) => {
                self.delete(key)?;
                Ok(None)
            }
        }
    }

    /// Delete a key: blob file, index row and tag-table rows. Returns whether the key
    /// was indexed.
    pub fn delete(&self, key: &str) -> Result<bool> {
        let txn = self.index.begin_write().map_err(dberr)?;
        let removed: Option<Vec<u8>> = {
            let mut t = txn.open_table(ENTRY_TABLE).map_err(dberr)?;
            t.remove(key.as_bytes())
                .map_err(dberr)?
                .map(|g| g.value().to_vec())
        };
        let idx: Option<DiskIndexEntry> = removed.and_then(|b| match serde_json::from_slice(&b) {
            Ok(parsed) => Some(parsed),
            Err(e) => {
                // V2-F70 session micro-debt: the corrupt index row was a
                // SILENT `None` (the F58-session repair's class): the
                // delete still commits (the row is gone either way, the
                // index self-heals on the next write) but the S-08 byte
                // budget never decrements and the blob file orphans — the
                // gap must be VISIBLE. Behavior unchanged; the SILENCE is
                // the repair.
                tracing::warn!(
                    code = "CACHE-CODEC",
                    key = %key,
                    error = %e,
                    "the disk index row did not parse; the delete proceeds, the byte budget may drift and the blob orphan"
                );
                None
            }
        });
        if let Some(idx) = &idx {
            // V2-B06: one multimap remove per membership (was a JSON
            // read-filter-write of the whole list per tag).
            let mut mm = txn.open_multimap_table(TAG_KEYS).map_err(dberr)?;
            for tag in &idx.tags {
                mm.remove(tag.as_str(), key).map_err(dberr)?;
            }
        }
        txn.commit().map_err(dberr)?;
        if let Some(idx) = idx {
            // S-08: keep the byte budget counter exact.
            self.cur_bytes.fetch_sub(idx.size, Ordering::Relaxed);
            // PERF-L1: forget the order entry.
            self.order_forget_placement(key, idx.stored_at);
            // Best-effort blob removal; a missing file is already "deleted".
            let _ = std::fs::remove_file(self.root.join(&idx.path));
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// S-08: current indexed byte total (for tests + stats).
    pub fn total_bytes(&self) -> u64 {
        self.cur_bytes.load(Ordering::Relaxed)
    }

    /// Keys tagged with `tag` (from the durable tag multimap).
    pub fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        let txn = self.index.begin_read().map_err(dberr)?;
        let mm = txn.open_multimap_table(TAG_KEYS).map_err(dberr)?;
        let mut out = Vec::new();
        for v in mm.get(tag).map_err(dberr)? {
            out.push(v.map_err(dberr)?.value().to_string());
        }
        Ok(out)
    }
}

// --------------------------------------------------------------------------- TieredCache

/// Cache counters snapshot for `oxide cache stats` + metrics (§20).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub stale_served: u64,
    pub coalesced: u64,
    /// F51: evictions the disk tier's S-08 prune completed — the push
    /// seam's `Evict` rows, counted pull-side. An explicit delete and a
    /// corrupt-miss self-heal are not here (the caller asked for the
    /// first, §17.2 documents the second). The memory tier's moka
    /// eviction is amortized maintenance (P-17) and is not counted: the
    /// disk tier is the durable data-loss path this counter covers.
    #[serde(default)]
    pub evictions: u64,
    pub entries_memory: u64,
    pub bytes_memory: u64,
}

/// Tag prefix used for path-based invalidation (`revalidate.path`, §11.3).
pub const PATH_TAG_PREFIX: &str = "path:";

/// Tag name for a path (`path:<path>`).
pub fn path_tag(path: &str) -> String {
    format!("{PATH_TAG_PREFIX}{path}")
}

enum Freshness {
    Fresh,
    Stale,
    Expired,
}

fn classify(e: &Entry, now: u64) -> Freshness {
    if e.is_fresh(now) {
        Freshness::Fresh
    } else if now <= e.stale_until() {
        Freshness::Stale
    } else {
        Freshness::Expired
    }
}

/// The default tiered store: memory (weighted LRU, TinyLFU-style admission) →
/// local disk (`redb` index + zstd blobs) → pluggable remote (Redis, S3, KV, HTTP).
///
/// Freshness (§11.3): `Hit::Fresh` within `ttl`, `Hit::Stale` within the SWR window,
/// past both a miss (entries deleted lazily where they were found). `stale_if_error`
/// is honored by the caller (render layer), not here.
pub struct TieredCache {
    pub memory: MemoryTier,
    /// PERF-L1 (readiness): `Arc` so the blocking disk calls can move onto the
    /// blocking pool (`spawn_blocking` needs `'static`).
    pub disk: Option<Arc<DiskTier>>,
    pub remote: Option<Arc<dyn CacheHandler>>,
    /// Tag index: tag → keys. A `HashSet` because membership is the only
    /// question the write path asks (P-18b: an entry re-set under a tag that
    /// already indexes thousands of keys is O(1), not an O(n) linear scan);
    /// readers need no order — [`TieredCache::tag_keys`] sorts for
    /// determinism before returning.
    pub tags: DashMap<String, HashSet<String>>,
    /// V2-B05: reverse index — key → its tag list. Keeps `delete` eager (no
    /// dangling tag members) and re-`set` under a changed tag list removes
    /// stale memberships. Private; the `tags` map stays the read API.
    key_tags: DashMap<String, Vec<String>>,
    hits: AtomicU64,
    misses: AtomicU64,
    stale_served: AtomicU64,
    coalesced: AtomicU64,
    /// The observability seam's subscriber (§20): `None` = the pull-only
    /// default (`stats()`), `Some` = one infallible callback per completed
    /// event. The `CaptureLayer::with_gate` posture — private field, the
    /// [`TieredCache::with_observer`] builder attaches.
    observer: Option<Arc<dyn CacheObserver>>,
}

impl TieredCache {
    /// Memory-only cache with the default 512 MiB cap, wrapped in an Arc.
    pub fn new_memory() -> Arc<Self> {
        Arc::new(Self {
            memory: MemoryTier::new(512 * 1024 * 1024),
            disk: None,
            remote: None,
            tags: DashMap::new(),
            key_tags: DashMap::new(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            stale_served: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
            observer: None,
        })
    }

    /// Owned constructor (compose + wrap in Arc yourself, e.g. to attach a disk tier).
    pub fn new(max_bytes: u64) -> Self {
        Self {
            memory: MemoryTier::new(max_bytes),
            disk: None,
            remote: None,
            tags: DashMap::new(),
            key_tags: DashMap::new(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            stale_served: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
            observer: None,
        }
    }

    /// Attach a local disk tier (L1).
    pub fn with_disk(mut self, root: std::path::PathBuf) -> Result<Self> {
        let tier = Arc::new(DiskTier::open(root)?);
        // F51: the observer-first builder order wires the eviction seam
        // down here; `with_observer` first lands in `with_observer`.
        if let Some(observer) = &self.observer {
            let _ = tier.eviction_observer.set(Arc::clone(observer));
        }
        self.disk = Some(tier);
        Ok(self)
    }

    /// Attach a caller-built disk tier (F51) — the composition arm for a
    /// tier the host opened with an explicit budget
    /// ([`DiskTier::open_with_budget`]). Same contract as
    /// [`TieredCache::with_disk`]; the tier the caller owns rides as-is.
    #[must_use = "the tier attaches through the builder chain; dropping it loses the disk tier"]
    pub fn with_disk_tier(mut self, tier: Arc<DiskTier>) -> Self {
        if let Some(observer) = &self.observer {
            let _ = tier.eviction_observer.set(Arc::clone(observer));
        }
        self.disk = Some(tier);
        self
    }

    /// Attach a remote tier (L2) — a handler like `HttpCacheHandler` that
    /// serves the §11.2 protocol. Reads fall through memory → disk → remote;
    /// writes write through all three. This is the constructor
    /// `cache.handler = { type: 'http', url }` wires to.
    pub fn with_remote(mut self, remote: Arc<dyn CacheHandler>) -> Self {
        self.remote = Some(remote);
        self
    }

    /// Attach the observability seam's subscriber (`observer.rs`, §20):
    /// one infallible [`CacheObserver`] callback per completed event —
    /// hits (with the serving layer and stale flag), misses, write-through
    /// sets, deletes, one aggregate row per completed tag invalidation,
    /// and one aggregate row per completed single-flight the host records
    /// through [`TieredCache::record_coalesced`]. Push-only: events flow
    /// from the attach onward, no replay; the default (`None`) keeps the
    /// pull-only [`TieredCache::stats`] contract untouched.
    #[must_use = "the observer attaches through the builder chain; dropping it loses the seam"]
    pub fn with_observer(mut self, observer: Arc<dyn CacheObserver>) -> Self {
        // F51: the eviction events ride the SAME one slot — the disk
        // tier hears what the tiered cache hears (one event, every
        // consumer). The observer-first builder order lands here;
        // disk-first lands in `with_disk`/`with_disk_tier`. The `Err`
        // arm would mean both orders attached — the same Arc, so the
        // first attach stands.
        if let Some(disk) = &self.disk {
            let _ = disk.eviction_observer.set(Arc::clone(&observer));
        }
        self.observer = Some(observer);
        self
    }

    /// Deliver one event to the attached observer. The no-observer path
    /// is one `None` branch; the attached path is one virtual call. The
    /// emit sites hand the `&str` they already hold, so a delivery
    /// allocates nothing.
    fn observe(&self, key: &str, op: CacheOp) {
        if let Some(observer) = &self.observer {
            observer.on_event(&CacheEvent { key, op });
        }
    }

    /// Get across tiers (memory → disk → remote). Every tier hit promotes the entry to
    /// memory. Records hits/misses/stale for [`TieredCache::stats`].
    pub async fn get(&self, key: &str) -> Result<Option<Hit>> {
        let now = now_ms();

        // L0: memory.
        if let Some(e) = self.memory.get(key) {
            match classify(&e, now) {
                Freshness::Fresh => {
                    self.bump_hit(false);
                    self.observe(
                        key,
                        CacheOp::Hit {
                            layer: Layer::L0,
                            stale: false,
                            bytes: e.body.len() as u64,
                        },
                    );
                    return Ok(Some(Hit::Fresh(e)));
                }
                Freshness::Stale => {
                    self.bump_hit(true);
                    self.observe(
                        key,
                        CacheOp::Hit {
                            layer: Layer::L0,
                            stale: true,
                            bytes: e.body.len() as u64,
                        },
                    );
                    return Ok(Some(Hit::Stale(e)));
                }
                Freshness::Expired => {
                    self.memory.remove(key); // lazy delete
                }
            }
        }

        // L1: disk (promote on hit). PERF-L1: redb + std::fs are BLOCKING —
        // run them on the blocking pool so a slow disk does not stall a
        // Tokio worker. `spawn_blocking` needs 'static: the disk tier is
        // behind an Arc; failures on the join handle become a miss (the
        // panic path is a poisoned pool, not a correctness bug). The handle
        // is cloned at each use site instead of being moved once, so the
        // expired-delete arm below does not re-unwrap `self.disk`.
        if let Some(disk) = &self.disk {
            let hit = {
                let disk = Arc::clone(disk);
                let k = key.to_string();
                tokio::task::spawn_blocking(move || disk.get(&k)).await
            };
            let e = match hit {
                Ok(Ok(Some(e))) => Some(e),
                Ok(_) => None,
                Err(_) => None,
            };
            if let Some(e) = e {
                match classify(&e, now) {
                    Freshness::Fresh => {
                        self.memory.insert(key, e.clone());
                        self.bump_hit(false);
                        self.observe(
                            key,
                            CacheOp::Hit {
                                layer: Layer::L1,
                                stale: false,
                                bytes: e.body.len() as u64,
                            },
                        );
                        return Ok(Some(Hit::Fresh(Arc::new(e))));
                    }
                    Freshness::Stale => {
                        self.memory.insert(key, e.clone());
                        self.bump_hit(true);
                        self.observe(
                            key,
                            CacheOp::Hit {
                                layer: Layer::L1,
                                stale: true,
                                bytes: e.body.len() as u64,
                            },
                        );
                        return Ok(Some(Hit::Stale(Arc::new(e))));
                    }
                    Freshness::Expired => {
                        let disk = Arc::clone(disk);
                        let k = key.to_string();
                        // V2-F70 session micro-debt: the expired entry's
                        // delete task was a silent `let _ =` swallow (the
                        // F65-session repair's class): a panicked task or a
                        // failed blob removal left an ORPHANED blob while
                        // the S-08 budget had already released it — the
                        // operator must see the gap. Behavior unchanged
                        // (the L0 answer already served the delete); the
                        // SILENCE is the repair.
                        match tokio::task::spawn_blocking(move || disk.delete(&k)).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => tracing::warn!(
                                code = "CAC-DISK",
                                key = %key,
                                error = %e.0.message,
                                "the expired entry's blob removal failed; the blob may be orphaned on disk"
                            ),
                            Err(join_err) => tracing::warn!(
                                code = "CAC-DISK",
                                key = %key,
                                error = %join_err,
                                "the expired entry's delete task panicked; the blob may be orphaned on disk"
                            ),
                        }
                    }
                }
            }
        }

        // L2: remote (promote on hit).
        if let Some(remote) = &self.remote
            && let Some(e) = remote.get(key).await?
        {
            match classify(&e, now) {
                Freshness::Fresh => {
                    self.memory.insert(key, e.clone());
                    self.bump_hit(false);
                    self.observe(
                        key,
                        CacheOp::Hit {
                            layer: Layer::L2,
                            stale: false,
                            bytes: e.body.len() as u64,
                        },
                    );
                    return Ok(Some(Hit::Fresh(Arc::new(e))));
                }
                Freshness::Stale => {
                    self.memory.insert(key, e.clone());
                    self.bump_hit(true);
                    self.observe(
                        key,
                        CacheOp::Hit {
                            layer: Layer::L2,
                            stale: true,
                            bytes: e.body.len() as u64,
                        },
                    );
                    return Ok(Some(Hit::Stale(Arc::new(e))));
                }
                Freshness::Expired => {
                    remote.delete(key).await?;
                }
            }
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
        self.observe(key, CacheOp::Miss);
        Ok(None)
    }

    fn bump_hit(&self, stale: bool) {
        self.hits.fetch_add(1, Ordering::Relaxed);
        if stale {
            self.stale_served.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Set writes through all tiers and indexes tags (deduped).
    pub async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        // S-08: an entry heavier than the memory cap was never servable, so
        // skip every tier — it must not fill the disk/remote either.
        let weight = entry_weight(&entry);
        if weight > self.memory.max_bytes {
            return Ok(());
        }
        self.memory.insert(key, entry.clone());
        // PERF-L1: the disk write (zstd + fsync-ish fs + redb txn) is blocking.
        if let Some(disk) = &self.disk {
            let disk = Arc::clone(disk);
            let k = key.to_string();
            let e = entry.clone();
            // The disk tier is best-effort relative to the memory tier: a
            // poisoned blocking task must not fail the request.
            let _ = tokio::task::spawn_blocking(move || disk.set(&k, &e)).await;
        }
        // P-18: the remote L2 write is off the request path — the local tiers
        // already satisfy the caller, so a slow/hanging remote must not block
        // the response. The task is fire-and-forget; `set` still returns `Ok`
        // (a remote error is logged by the handler, degrading silently).
        if let Some(remote) = &self.remote {
            let remote = Arc::clone(remote);
            let key = key.to_string();
            let entry = entry.clone();
            tokio::spawn(async move {
                let _ = remote.set(&key, entry).await;
            });
        }
        for tag in &entry.tags {
            self.tags
                .entry(tag.clone())
                .or_default()
                .insert(key.to_string());
        }
        // V2-B05: one hash-map entry op maintains the reverse index — a
        // re-set whose tag list changed must drop memberships the entry no
        // longer has (stale members would survive invalidation).
        match self.key_tags.entry(key.to_string()) {
            dashmap::mapref::entry::Entry::Occupied(mut o) => {
                let old = o.insert(entry.tags.clone());
                for t in old.iter().filter(|t| !entry.tags.contains(t)) {
                    if let Some(mut s) = self.tags.get_mut(t) {
                        s.remove(key);
                    }
                }
            }
            dashmap::mapref::entry::Entry::Vacant(v) => {
                v.insert(entry.tags.clone());
            }
        }
        self.observe(
            key,
            CacheOp::Set {
                bytes: entry.body.len() as u64,
            },
        );
        Ok(())
    }

    /// Like [`TieredCache::set`], but also tags the entry `path:<path>` so
    /// [`TieredCache::invalidate_path`] can find it (§11.3).
    pub async fn set_with_path(&self, key: &str, entry: Entry, path: &str) -> Result<()> {
        let mut entry = entry;
        let tag = path_tag(path);
        if !entry.tags.iter().any(|t| t == &tag) {
            entry.tags.push(tag);
        }
        self.set(key, entry).await
    }

    /// Delete from every tier and clean the tag indexes eagerly (V2-B05) —
    /// a deleted key never dangles in a tag set.
    pub async fn delete(&self, key: &str) -> Result<()> {
        self.memory.remove(key);
        if let Some(disk) = &self.disk {
            disk.delete(key)?;
        }
        if let Some(remote) = &self.remote {
            remote.delete(key).await?;
        }
        if let Some((_, tags)) = self.key_tags.remove(key) {
            for t in tags {
                if let Some(mut s) = self.tags.get_mut(&t) {
                    s.remove(key);
                }
            }
        }
        self.observe(key, CacheOp::Delete);
        Ok(())
    }

    /// Keys tagged with `tag` across the in-memory index, disk and remote (deduped,
    /// sorted for determinism).
    pub async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        let mut keys: Vec<String> = self
            .tags
            .get(tag)
            .map(|l| l.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(disk) = &self.disk {
            for k in disk.tag_keys(tag)? {
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
        }
        if let Some(remote) = &self.remote {
            for k in remote.tag_keys(tag).await? {
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    /// V2-B05 (D25): the reverse index's read half — the tags `key` was
    /// last set under, sorted for determinism. Empty when the key was
    /// never set or its delete already ran (the reverse index is cleaned
    /// eagerly, so a deleted key never dangles). This is the observable
    /// face of the private `key_tags` map the write path maintains: one
    /// hash-map entry op per set, one removal per delete.
    #[must_use = "the tag list answers the caller's attribution question; dropping it loses the answer"]
    pub fn tags_of(&self, key: &str) -> Vec<String> {
        let mut tags = self
            .key_tags
            .get(key)
            .map_or_else(Vec::new, |entry| entry.value().clone());
        tags.sort();
        tags
    }

    /// `revalidate.tag('x')`: soft marks stale (serve-then-regen) or hard deletes (§11.3).
    ///
    /// Soft invalidation rewrites each matching entry with `ttl = 0` and
    /// `swr = original_swr + original_ttl`, so it is immediately stale but remains
    /// servable for the original freshness window (documented marker scheme). An entry
    /// that is already past its whole usable window is deleted instead of resurrected.
    /// Returns the number of distinct keys affected.
    pub async fn invalidate_tag(&self, tag: &str, soft: bool) -> Result<usize> {
        let now = now_ms();
        let keys = self.tag_keys(tag).await?;
        let mut count = 0usize;
        for key in &keys {
            let Some(entry) = self.lookup_quiet(key).await? else {
                continue; // dangling tag row (entry already gone from every tier)
            };
            if soft {
                if now > entry.stale_until() {
                    self.delete(key).await?; // already dead: soft on a corpse = delete
                } else {
                    let mut marked = (*entry).clone();
                    let orig = marked.policy.clone();
                    marked.policy.ttl = Duration::ZERO;
                    marked.policy.swr = Some(orig.swr.unwrap_or_default() + orig.ttl);
                    // Rewind one millisecond: the zeroed TTL must take effect
                    // immediately, and is_fresh uses `now <= stored_at + ttl`, so an
                    // entry rewritten within the same wall-clock millisecond could
                    // otherwise still read as fresh at ms resolution.
                    marked.stored_at = marked.stored_at.saturating_sub(1);
                    self.set(key, marked).await?;
                }
            } else {
                self.delete(key).await?;
            }
            count += 1;
        }
        if !soft {
            self.tags.remove(tag);
        }
        if let Some(remote) = &self.remote {
            remote.publish_invalidation(tag, soft).await?;
        }
        // The invalidate row keys on the TAG (the event the operator asks
        // about is "tag X invalidated"), not on each affected cache key —
        // the per-key traffic during the sweep emits its own rows.
        self.observe(
            tag,
            CacheOp::Invalidate {
                tag: tag.to_owned(),
                soft,
                count,
            },
        );
        Ok(count)
    }

    /// `revalidate.path('/blog/a')` — matches entries tagged `path:<path>` at set time
    /// (via [`TieredCache::set_with_path`]).
    pub async fn invalidate_path(&self, path: &str, soft: bool) -> Result<usize> {
        self.invalidate_tag(&path_tag(path), soft).await
    }

    /// Build seeding (§11.3): insert prerendered entries at deploy start.
    /// Returns the number of entries successfully written.
    pub async fn seed(&self, entries: Vec<(String, Entry)>) -> Result<usize> {
        let mut seeded = 0usize;
        for (key, entry) in entries {
            if self.set(&key, entry).await.is_ok() {
                seeded += 1;
            }
        }
        Ok(seeded)
    }

    /// Stats for `oxide cache stats` + metrics (§20). `hits` counts fresh and stale
    /// serves; `stale_served` is the stale subset; seeding writes do not count as
    /// traffic.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            stale_served: self.stale_served.load(Ordering::Relaxed),
            coalesced: self.coalesced.load(Ordering::Relaxed),
            // F51: the disk tier's eviction counter (the honest zero
            // without a disk tier — no tier, no counted eviction).
            evictions: self.disk.as_ref().map_or(0, |disk| disk.evictions()),
            entries_memory: self.memory.len() as u64,
            bytes_memory: self.memory.total_bytes(),
        }
    }

    /// The posture pull (`health.rs`, F32): the whole stack in one
    /// [`CacheHealth`] value — tier attachment, resident occupancy, the
    /// tag-index pressure, and the observability seam's own attachment
    /// state. The two pulls sample the same live counters
    /// (`health().memory_entries == stats().entries_memory`); what
    /// `stats()` cannot say is the tier attachment facts, and those are a
    /// fact of the tiers, not of the traffic. The remote tier is an
    /// opaque `Arc<dyn CacheHandler>` — its posture rides the
    /// [`CacheHandler::handler_kind`] introspection method, honestly
    /// `None` when no tier is attached.
    #[must_use]
    pub fn health(&self) -> crate::health::CacheHealth {
        crate::health::CacheHealth {
            memory_entries: self.memory.len() as u64,
            memory_bytes: self.memory.total_bytes(),
            memory_capacity_bytes: self.memory.max_bytes,
            disk_attached: self.disk.is_some(),
            disk_bytes: self.disk.as_ref().map_or(0, |disk| disk.total_bytes()),
            remote_attached: self.remote.is_some(),
            remote_kind: self.remote.as_ref().map(|h| h.handler_kind().to_string()),
            tags_indexed: self.tags.len() as u64,
            keys_tracked: self.key_tags.len() as u64,
            observer_attached: self.observer.is_some(),
            // F52: the disk tier's eviction total rides the posture —
            // the same counter `stats().evictions` and the push seam
            // answer, honestly zero without a disk tier.
            disk_evictions: self.disk.as_ref().map_or(0, |disk| disk.evictions()),
        }
    }

    /// Record one completed single-flight and emit its observer row (the
    /// push twin of the pull channel's `coalesced` counter, F33): the
    /// HOST's flight owner calls this when the owner's render finishes,
    /// `followers` being the count the publish reached (what
    /// [`crate::SingleFlight::publish_and_cleanup`] returns). The counter
    /// bump feeds [`TieredCache::stats`]; the
    /// [`CacheOp::Coalesce`] row joins the §20 push channel, so the
    /// exporter and the feed see the joins the pull channel always
    /// counted. One aggregate row per flight — the key names the flight's
    /// key, a zero-follower flight still records.
    pub fn record_coalesced(&self, key: &str, followers: u64) {
        self.coalesced.fetch_add(followers, Ordering::Relaxed);
        self.observe(key, CacheOp::Coalesce { followers });
    }

    /// Tier-by-tier lookup without touching stats or promoting (used by invalidation).
    async fn lookup_quiet(&self, key: &str) -> Result<Option<Arc<Entry>>> {
        if let Some(e) = self.memory.get(key) {
            return Ok(Some(e));
        }
        if let Some(disk) = &self.disk
            && let Some(e) = disk.get(key)?
        {
            return Ok(Some(Arc::new(e)));
        }
        if let Some(remote) = &self.remote
            && let Some(e) = remote.get(key).await?
        {
            return Ok(Some(Arc::new(e)));
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// §11.2: `TieredCache` as a `CacheHandler`.
//
// The concrete tiered store already has the machinery (`get` across
// memory → disk → remote, `set`, tag indices); this is the trait surface the
// pipeline and the worker op dispatch against.
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl CacheHandler for TieredCache {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        // `TieredCache::get` returns a `Hit` (fresh / stale) or a miss. The
        // trait's `get` is "the entry if present" — the caller decides what
        // to do with a stale one, and both fresh and stale entries are
        // returned here so that decision stays with the caller.
        match TieredCache::get(self, key).await? {
            Some(Hit::Fresh(e) | Hit::Stale(e)) => Ok(Some((*e).clone())),
            None => Ok(None),
        }
    }

    async fn get_arc(&self, key: &str) -> Result<Option<Arc<Entry>>> {
        // P-03: hand back the shared entry — no body copy on a hot hit.
        match TieredCache::get(self, key).await? {
            Some(Hit::Fresh(e) | Hit::Stale(e)) => Ok(Some(e)),
            None => Ok(None),
        }
    }

    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        // `TieredCache::set` writes memory + disk and records the tags.
        TieredCache::set(self, key, entry).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.memory.remove(key);
        if let Some(disk) = &self.disk {
            let _ = disk.delete(key);
        }
        Ok(())
    }

    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        Ok(self
            .tags
            .get(tag)
            .map(|v| v.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default())
    }

    async fn invalidate_tag(&self, tag: &str, _soft: bool) -> Result<usize> {
        // The soft/hard distinction lives in the policy, not the store: a
        // soft invalidation marks entries stale, which the render layer
        // honours by serving stale while regenerating. This method deletes
        // the tag's keys — the hard form — and returns the count.
        let keys = self.tag_keys(tag).await?;
        for k in &keys {
            self.delete(k).await?;
        }
        self.tags.remove(tag);
        Ok(keys.len())
    }

    async fn publish_invalidation(&self, _tag: &str, _soft: bool) -> Result<()> {
        // No pub/sub on the local tiers; a remote handler does this.
        Ok(())
    }

    fn handler_kind(&self) -> &'static str {
        "tiered"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CacheKey, Layer, Policy};

    fn entry(body: &[u8], tags: &[&str]) -> Entry {
        Entry {
            body: body.to_vec(),
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            tags: tags.iter().map(|s| s.to_string()).collect(),
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

    #[test]
    fn memory_remove_and_clear_reset_accounting() {
        let mem = MemoryTier::new(10_000);
        mem.insert("a", entry(b"aaa", &[]));
        mem.insert("b", entry(b"bbb", &[]));
        assert_eq!(mem.len(), 2);
        assert!(mem.remove("a").is_some());
        assert!(mem.remove("a").is_none());
        assert_eq!(mem.len(), 1);
        assert!(mem.total_bytes() < 10_000);
        mem.clear();
        assert!(mem.is_empty());
        assert_eq!(mem.total_bytes(), 0);
        // Usable after clear (weights not corrupted by the manual-style reset).
        mem.insert("c", entry(b"ccc", &[]));
        assert!(mem.get("c").is_some());
    }

    #[test]
    fn memory_respects_cap_under_churn() {
        let mem = MemoryTier::new(2_000);
        for i in 0..20 {
            mem.insert(&format!("k{i:02}"), entry(b"x", &[]));
        }
        for round in 0..40 {
            for i in 0..20 {
                let _ = mem.get(&format!("k{i:02}"));
            }
            if round % 5 == 0 {
                mem.insert(&format!("k{:02}", round % 20), entry(b"x", &[]));
            }
        }
        assert!(mem.total_bytes() <= 2_000);
        assert!(!mem.is_empty());
    }

    #[test]
    fn memory_replacement_updates_weight_accounting() {
        let mem = MemoryTier::new(500);
        mem.insert("a", entry(&[b'a'; 100], &[])); // w ≈ 153
        let single = mem.total_bytes();
        mem.insert("a", entry(&[b'a'; 10], &[])); // smaller replacement
        assert_eq!(mem.len(), 1);
        assert!(
            mem.total_bytes() <= single,
            "replacement shrank or held total weight"
        );
        mem.insert("b", entry(&[b'a'; 100], &[]));
        assert!(mem.total_bytes() <= 500);
        assert!(mem.get("b").is_some());
    }

    #[test]
    fn disk_blob_name_sanitizes_path_like_keys() {
        assert_eq!(DiskTier::blob_name("abc123"), "abc123.bin");
        assert_eq!(DiskTier::blob_name("a-b_c"), "a-b_c.bin");
        let weird = DiskTier::blob_name("../etc/passwd");
        assert!(!weird.contains('/'));
        assert!(weird.starts_with('h'));
        assert_eq!(weird.len(), "h".len() + 32 + ".bin".len());
        let long = DiskTier::blob_name(&"k".repeat(200));
        assert!(long.len() < 200);
    }

    #[test]
    fn disk_open_is_idempotent_and_reopens_existing_index() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("c");
        let d1 = DiskTier::open(root.clone()).unwrap();
        d1.set("k", &entry(b"v", &["t"])).unwrap();
        drop(d1);
        let d2 = DiskTier::open(root).unwrap(); // reopens the same redb file
        assert!(d2.get("k").unwrap().is_some());
        assert_eq!(d2.tag_keys("t").unwrap(), vec!["k".to_string()]);
    }

    #[test]
    fn disk_hash_mismatch_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let disk = DiskTier::open(dir.path().join("c")).unwrap();
        disk.set("k", &entry(b"payload", &[])).unwrap();
        // Tamper with the index hash directly through a rewrite of the blob contents:
        // write a *valid zstd stream of different bytes* — decode succeeds, hash fails.
        let other = zstd::bulk::compress(b"tampered", 3).unwrap();
        let idx_path = {
            let mut blobs = std::fs::read_dir(dir.path().join("c/blobs")).unwrap();
            let shard = blobs.next().unwrap().unwrap().path();
            std::fs::read_dir(&shard)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path()
        };
        std::fs::write(idx_path, other).unwrap();
        assert!(
            disk.get("k").unwrap().is_none(),
            "hash mismatch must not serve"
        );
        assert!(disk.get("k").unwrap().is_none(), "evicted after mismatch");
    }

    #[test]
    fn disk_rejects_entry_over_per_entry_cap() {
        let dir = tempfile::tempdir().unwrap();
        let disk = DiskTier::open_with_budget(dir.path().join("c"), 1 << 20, 100).unwrap();
        // Body 200 > cap 100 — rejected, nothing written.
        assert!(disk.set("big", &entry(&[b'x'; 200], &[])).is_err());
        assert!(disk.get("big").unwrap().is_none());
        assert_eq!(disk.total_bytes(), 0);
    }

    #[test]
    fn disk_prunes_oldest_to_stay_under_budget() {
        let dir = tempfile::tempdir().unwrap();
        // Budget 4 KiB; each body compresses small, so write many and check bound.
        let disk = DiskTier::open_with_budget(dir.path().join("c"), 4096, 1 << 20).unwrap();
        for i in 0..200 {
            let mut e = entry(&[b'y'; 512], &[]);
            e.stored_at = i; // monotonic write order
            disk.set(&format!("k{i:03}"), &e).unwrap();
        }
        assert!(
            disk.total_bytes() <= 4096,
            "disk budget exceeded: {}",
            disk.total_bytes()
        );
        // The most recently written key must survive; the earliest must be gone.
        assert!(disk.get("k199").unwrap().is_some(), "newest pruned");
        assert!(disk.get("k000").unwrap().is_none(), "oldest not pruned");
    }

    #[test]
    fn disk_delete_keeps_byte_counter_exact() {
        let dir = tempfile::tempdir().unwrap();
        let disk = DiskTier::open(dir.path().join("c")).unwrap();
        disk.set("a", &entry(&[b'a'; 100], &[])).unwrap();
        let after_set = disk.total_bytes();
        assert!(after_set > 0);
        assert!(disk.delete("a").unwrap());
        assert_eq!(disk.total_bytes(), 0);
    }

    #[tokio::test]
    async fn tiered_cache_key_and_layer_helpers_available() {
        // CacheKey/Layer flow through the tiered API's key space (integration tests
        // cover tier behavior; this pins the key derivation plumbing — the D24
        // KeyHash's hex display is the string the trait boundary carries).
        let cache = TieredCache::new(1 << 20);
        let key = CacheKey {
            layer: Layer::L0,
            route: "/".into(),
            params: String::new(),
            vary: String::new(),
            build_id: crate::build_id(&[b"m"]),
        };
        let digest = key.canonical().to_string();
        cache.set(&digest, entry(b"hello", &[])).await.unwrap();
        assert!(cache.get(&digest).await.unwrap().is_some());
        cache.record_coalesced(&digest, 3);
        assert_eq!(cache.stats().coalesced, 3);
        assert_eq!(path_tag("/x"), "path:/x");
        assert!(PATH_TAG_PREFIX.starts_with("path"));
    }
}
