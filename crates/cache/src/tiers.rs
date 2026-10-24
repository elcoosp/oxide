//! Tiered store (spec §11.2): memory (in-house weighted LRU) → local disk
//! (`redb` index + zstd blob files) → pluggable remote [`CacheHandler`].
//!
//! # Memory tier (moka, spec §11.2)
//!
//! L0/L1 memory tier is a [`moka::sync::Cache`] with a weigher of [`entry_weight`];
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

use crate::{CacheHandler, Entry, Hit, Result, err};
use dashmap::DashMap;
use moka::sync::Cache as MokaCache;
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use redb::ReadableDatabase;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Current wall clock in unix milliseconds (the crate-wide time base for §11.3).
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

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

const ENTRY_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("oxide-cache-entries");
const TAG_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("oxide-cache-tags");

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
    OxideError(
        Diagnostic::error("CAC-DB", format!("redb: {e}"))
            .with_fix("check disk space/permissions for the cache root"),
    )
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
/// zstd-compressed JSON serialization of an [`Entry`]. On load the stored blake3 is
/// verified; any mismatch/decode failure/missing blob deletes the entry and reports a
/// miss (silent self-healing eviction — corruption must never serve, §17.2).
pub struct DiskTier {
    pub root: PathBuf,
    index: Database,
    /// S-08: total compressed bytes currently indexed (admission + prune base).
    cur_bytes: AtomicU64,
    /// S-08: budget — the store prunes oldest-first past this total.
    pub max_bytes: u64,
    /// S-08: reject a single entry larger than this before writing it.
    pub max_entry: u64,
}

/// S-08 default disk budget (1 GiB) and per-entry cap (32 MiB). A project can
/// override via config; these bounds stop attacker-minted keys filling the disk.
pub const DEFAULT_DISK_MAX_BYTES: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_DISK_MAX_ENTRY: u64 = 32 * 1024 * 1024;

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
            OxideError(
                Diagnostic::error("CAC-IO", format!("create {}: {e}", root.display()))
                    .with_fix("check that the cache root is writable"),
            )
        })?;
        std::fs::create_dir_all(root.join("blobs")).map_err(ioerr)?;
        let index = Database::create(root.join("index.redb")).map_err(dberr)?;
        let txn = index.begin_write().map_err(dberr)?;
        {
            // Ensure both tables exist so reads never fail on a fresh store.
            txn.open_table(ENTRY_TABLE).map_err(dberr)?;
            txn.open_table(TAG_TABLE).map_err(dberr)?;
        }
        txn.commit().map_err(dberr)?;
        // S-08: seed the current-size counter from the existing index.
        let mut cur = 0u64;
        {
            let rtxn = index.begin_read().map_err(dberr)?;
            let table = rtxn.open_table(ENTRY_TABLE).map_err(dberr)?;
            let iter = table.iter().map_err(dberr)?;
            for row in iter {
                if let Ok((_, v)) = row {
                    if let Ok(idx) = serde_json::from_slice::<DiskIndexEntry>(v.value()) {
                        cur = cur.saturating_add(idx.size);
                    }
                }
            }
        }
        Ok(Self {
            root,
            index,
            cur_bytes: AtomicU64::new(cur),
            max_bytes,
            max_entry,
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
            format!(
                "h{}.bin",
                blake3::hash(key.as_bytes()).to_string()[..32].to_string()
            )
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
        let serialized = serde_json::to_vec(entry).map_err(jsonerr)?;
        let hash = blake3::hash(&serialized).to_string();
        let compressed = zstd::bulk::compress(&serialized, 3)
            .map_err(|e| err("CAC-ZSTD", format!("compress: {e}")))?;
        // The previous row's size, so the byte counter stays exact on replace.
        let prev_size = self.indexed_size(key);
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
            let mut tags = txn.open_table(TAG_TABLE).map_err(dberr)?;
            for tag in &entry.tags {
                let list: Vec<String> = match tags.get(tag.as_bytes()).map_err(dberr)? {
                    Some(v) => serde_json::from_slice(v.value()).unwrap_or_default(),
                    None => Vec::new(),
                };
                let mut list = list;
                if !list.iter().any(|k| k == key) {
                    list.push(key.to_string());
                }
                let bytes = serde_json::to_vec(&list).map_err(jsonerr)?;
                tags.insert(tag.as_bytes(), bytes.as_slice())
                    .map_err(dberr)?;
            }
        }
        txn.commit().map_err(dberr)?;
        // S-08: update the byte counter and prune oldest-first past the budget.
        let new_size = compressed.len() as u64;
        self.cur_bytes.fetch_sub(prev_size, Ordering::Relaxed);
        let after = self.cur_bytes.fetch_add(new_size, Ordering::Relaxed) + new_size;
        if after > self.max_bytes {
            self.prune_oldest_until(self.max_bytes * 9 / 10);
        }
        Ok(())
    }

    /// S-08: the indexed compressed size of `key`, or 0 when absent.
    fn indexed_size(&self, key: &str) -> u64 {
        let Ok(txn) = self.index.begin_read() else {
            return 0;
        };
        let Ok(table) = txn.open_table(ENTRY_TABLE) else {
            return 0;
        };
        match table.get(key.as_bytes()) {
            Ok(Some(v)) => serde_json::from_slice::<DiskIndexEntry>(v.value())
                .map(|i| i.size)
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// S-08: evict least-recently-stored entries until the total is under
    /// `target`. Ordering is by `stored_at` ascending (LRU by write time —
    /// the durable proxy the GC already tracks).
    fn prune_oldest_until(&self, target: u64) {
        loop {
            if self.cur_bytes.load(Ordering::Relaxed) <= target {
                return;
            }
            // Find the oldest key + its size.
            let victim = {
                let Ok(txn) = self.index.begin_read() else {
                    return;
                };
                let Ok(table) = txn.open_table(ENTRY_TABLE) else {
                    return;
                };
                let Ok(iter) = table.iter() else {
                    return;
                };
                let mut oldest: Option<(String, u64, u64)> = None; // (key, stored_at, size)
                for row in iter {
                    let Ok((k, v)) = row else { continue };
                    let Some(idx) = serde_json::from_slice::<DiskIndexEntry>(v.value()).ok() else {
                        continue;
                    };
                    let k = String::from_utf8_lossy(k.value()).into_owned();
                    if oldest.as_ref().is_none_or(|(_, ts, _)| idx.stored_at < *ts) {
                        oldest = Some((k, idx.stored_at, idx.size));
                    }
                }
                oldest
            };
            let Some((key, _, _)) = victim else {
                return;
            };
            // `delete` decrements `cur_bytes` itself.
            if self.delete(&key).is_err() {
                return;
            }
        }
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
            let removed = match t.remove(key.as_bytes()).map_err(dberr)? {
                Some(g) => Some(g.value().to_vec()),
                None => None,
            };
            removed
        };
        let idx: Option<DiskIndexEntry> = removed.and_then(|b| serde_json::from_slice(&b).ok());
        if let Some(idx) = &idx {
            let mut tags = txn.open_table(TAG_TABLE).map_err(dberr)?;
            for tag in &idx.tags {
                let current: Option<Vec<u8>> = tags
                    .get(tag.as_bytes())
                    .map_err(dberr)?
                    .map(|g| g.value().to_vec());
                if let Some(current) = current {
                    let list: Vec<String> = serde_json::from_slice(&current).unwrap_or_default();
                    let filtered: Vec<String> = list.into_iter().filter(|k| k != key).collect();
                    let bytes = serde_json::to_vec(&filtered).map_err(jsonerr)?;
                    tags.insert(tag.as_bytes(), bytes.as_slice())
                        .map_err(dberr)?;
                }
            }
        }
        txn.commit().map_err(dberr)?;
        if let Some(idx) = idx {
            // S-08: keep the byte budget counter exact.
            self.cur_bytes.fetch_sub(idx.size, Ordering::Relaxed);
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

    /// Keys tagged with `tag` (from the durable tag table).
    pub fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        let txn = self.index.begin_read().map_err(dberr)?;
        let table = txn.open_table(TAG_TABLE).map_err(dberr)?;
        match table.get(tag.as_bytes()).map_err(dberr)? {
            Some(v) => Ok(serde_json::from_slice(v.value()).unwrap_or_default()),
            None => Ok(Vec::new()),
        }
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
    pub disk: Option<DiskTier>,
    pub remote: Option<Arc<dyn CacheHandler>>,
    /// Tag index: tag → keys.
    pub tags: DashMap<String, Vec<String>>,
    hits: AtomicU64,
    misses: AtomicU64,
    stale_served: AtomicU64,
    coalesced: AtomicU64,
}

impl TieredCache {
    /// Memory-only cache with the default 512 MiB cap, wrapped in an Arc.
    pub fn new_memory() -> Arc<Self> {
        Arc::new(Self {
            memory: MemoryTier::new(512 * 1024 * 1024),
            disk: None,
            remote: None,
            tags: DashMap::new(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            stale_served: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
        })
    }

    /// Owned constructor (compose + wrap in Arc yourself, e.g. to attach a disk tier).
    pub fn new(max_bytes: u64) -> Self {
        Self {
            memory: MemoryTier::new(max_bytes),
            disk: None,
            remote: None,
            tags: DashMap::new(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            stale_served: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
        }
    }

    /// Attach a local disk tier (L1).
    pub fn with_disk(mut self, root: std::path::PathBuf) -> Result<Self> {
        self.disk = Some(DiskTier::open(root)?);
        Ok(self)
    }

    /// Attach a remote tier (L2) — a handler like `HttpCacheHandler` that
    /// serves the §11.2 protocol. Reads fall through memory → disk → remote;
    /// writes write through all three. This is the constructor
    /// `cache.handler = { type: 'http', url }` wires to.
    pub fn with_remote(mut self, remote: Arc<dyn CacheHandler>) -> Self {
        self.remote = Some(remote);
        self
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
                    return Ok(Some(Hit::Fresh(e)));
                }
                Freshness::Stale => {
                    self.bump_hit(true);
                    return Ok(Some(Hit::Stale(e)));
                }
                Freshness::Expired => {
                    self.memory.remove(key); // lazy delete
                }
            }
        }

        // L1: disk (promote on hit).
        if let Some(disk) = &self.disk {
            if let Some(e) = disk.get(key)? {
                match classify(&e, now) {
                    Freshness::Fresh => {
                        self.memory.insert(key, e.clone());
                        self.bump_hit(false);
                        return Ok(Some(Hit::Fresh(Arc::new(e))));
                    }
                    Freshness::Stale => {
                        self.memory.insert(key, e.clone());
                        self.bump_hit(true);
                        return Ok(Some(Hit::Stale(Arc::new(e))));
                    }
                    Freshness::Expired => {
                        disk.delete(key)?;
                    }
                }
            }
        }

        // L2: remote (promote on hit).
        if let Some(remote) = &self.remote {
            if let Some(e) = remote.get(key).await? {
                match classify(&e, now) {
                    Freshness::Fresh => {
                        self.memory.insert(key, e.clone());
                        self.bump_hit(false);
                        return Ok(Some(Hit::Fresh(Arc::new(e))));
                    }
                    Freshness::Stale => {
                        self.memory.insert(key, e.clone());
                        self.bump_hit(true);
                        return Ok(Some(Hit::Stale(Arc::new(e))));
                    }
                    Freshness::Expired => {
                        remote.delete(key).await?;
                    }
                }
            }
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
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
        if let Some(disk) = &self.disk {
            disk.set(key, &entry)?;
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
            let mut list = self.tags.entry(tag.clone()).or_default();
            if !list.iter().any(|k| k == key) {
                list.push(key.to_string());
            }
        }
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

    /// Delete from every tier. Tag index rows for the key are cleaned lazily on the
    /// next tag invalidation (a dangling key never matches a live entry).
    pub async fn delete(&self, key: &str) -> Result<()> {
        self.memory.remove(key);
        if let Some(disk) = &self.disk {
            disk.delete(key)?;
        }
        if let Some(remote) = &self.remote {
            remote.delete(key).await?;
        }
        Ok(())
    }

    /// Keys tagged with `tag` across the in-memory index, disk and remote (deduped,
    /// sorted for determinism).
    pub async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        let mut keys = self.tags.get(tag).map(|l| l.clone()).unwrap_or_default();
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
            entries_memory: self.memory.len() as u64,
            bytes_memory: self.memory.total_bytes(),
        }
    }

    /// Feed the coalesced counter from the render layer's single-flight usage
    /// (the number of followers that were coalesced onto an owner).
    pub fn record_coalesced(&self, followers: u64) {
        self.coalesced.fetch_add(followers, Ordering::Relaxed);
    }

    /// Tier-by-tier lookup without touching stats or promoting (used by invalidation).
    async fn lookup_quiet(&self, key: &str) -> Result<Option<Arc<Entry>>> {
        if let Some(e) = self.memory.get(key) {
            return Ok(Some(e));
        }
        if let Some(disk) = &self.disk {
            if let Some(e) = disk.get(key)? {
                return Ok(Some(Arc::new(e)));
            }
        }
        if let Some(remote) = &self.remote {
            if let Some(e) = remote.get(key).await? {
                return Ok(Some(Arc::new(e)));
            }
        }
        Ok(None)
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
        // cover tier behavior; this pins the key derivation plumbing).
        let cache = TieredCache::new(1 << 20);
        let key = CacheKey {
            layer: Layer::L0,
            route: "/".into(),
            params: "".into(),
            vary: "".into(),
            build_id: crate::build_id(&[b"m"]),
        };
        cache
            .set(&key.canonical(), entry(b"hello", &[]))
            .await
            .unwrap();
        assert!(cache.get(&key.canonical()).await.unwrap().is_some());
        cache.record_coalesced(3);
        assert_eq!(cache.stats().coalesced, 3);
        assert_eq!(path_tag("/x"), "path:/x");
        assert!(PATH_TAG_PREFIX.starts_with("path"));
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
            Some(Hit::Fresh(e)) | Some(Hit::Stale(e)) => Ok(Some((*e).clone())),
            None => Ok(None),
        }
    }

    async fn get_arc(&self, key: &str) -> Result<Option<Arc<Entry>>> {
        // P-03: hand back the shared entry — no body copy on a hot hit.
        match TieredCache::get(self, key).await? {
            Some(Hit::Fresh(e)) | Some(Hit::Stale(e)) => Ok(Some(e)),
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
        Ok(self.tags.get(tag).map(|v| v.clone()).unwrap_or_default())
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
}
