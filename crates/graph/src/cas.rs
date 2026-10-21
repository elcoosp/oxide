//! Tier B content-addressed store (spec §5.4, D6, §17.2).
//!
//! Blobs are content-addressed by blake3, zstd-compressed, one file per key
//! under `<root>/blobs/<2-hex-shard>/<hex>.zst`. The index is a `redb`
//! database mapping `CasKey → (sizes, last_use, namespace, kind)` plus a
//! `namespace → keys` listing for stats. On load the blake3 hash is verified
//! (`CAS-CORRUPT` on mismatch, §17.2).
//!
//! Note on rkyv: artifacts serialized *into* the CAS may use rkyv (validated by
//! hash on load at the artifact layer, §5.4); the store itself only handles
//! opaque bytes, so no rkyv dependency is needed here.

use crate::{OxideError, Result, err};
use redb::ReadableDatabase;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const INDEX_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("oxide-cas-index");
const NS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("oxide-cas-namespaces");

/// Content-addressed store key = blake3 of content.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CasKey(pub [u8; 32]);

impl CasKey {
    pub fn of(data: &[u8]) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(data);
        Self(*h.finalize().as_bytes())
    }
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Metadata recorded in the CAS index for a write (§5.4 cache keys include
/// versions, option hashes, target and tracked env — carried as the namespace).
pub struct CasWriteMeta {
    /// e.g. ["transform", "svelte-compiler", "1.2.3"] — part of the logical namespace.
    pub namespace: Vec<String>,
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    /// Uncompressed payload size (bytes).
    uncompressed: u64,
    /// On-disk (compressed) size — the unit for GC size caps and reclaimed bytes.
    stored: u64,
    /// Last-use timestamp, millis since the Unix epoch (§5.4 "GC by last-use").
    last_use_ms: u64,
    /// Logical namespace, e.g. `transform/svelte-compiler/1.2.3`.
    namespace: String,
    /// Artifact kind tag from [`CasWriteMeta`].
    kind: String,
}

/// Tier B persistent CAS: `.oxide/cache`, blobs content-addressed + zstd-compressed,
/// index in `redb` (§5.4). Keys include Oxide version, adapter version, plugin versions,
/// option hashes, target and tracked env.
pub struct ContentStore {
    pub root: PathBuf,
    index: Database,
}

impl ContentStore {
    /// Open (creating dirs + redb index as needed) at `root` (default `.oxide/cache`).
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("blobs")).map_err(|e| {
            err(
                "CAS-IO",
                format!("create {}: {e}", root.join("blobs").display()),
            )
        })?;
        let index = Database::create(root.join("index.redb"))
            .map_err(|e| err("CAS-DB", format!("open index: {e}")))?;
        // Ensure both tables exist so reads never fail on a fresh store.
        let txn = index.begin_write().map_err(dberr)?;
        {
            txn.open_table(INDEX_TABLE).map_err(dberr)?;
            txn.open_table(NS_TABLE).map_err(dberr)?;
        }
        txn.commit().map_err(dberr)?;
        Ok(Self { root, index })
    }

    /// Store bytes; returns the content key. Idempotent.
    pub fn put(&self, data: &[u8], meta: &CasWriteMeta) -> Result<CasKey> {
        let key = CasKey::of(data);
        let compressed = zstd::bulk::compress(data, 3).map_err(ioerr)?;
        let path = self.blob_path(&key);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(ioerr)?;
            }
            let tmp = path.with_extension("zst.tmp");
            {
                let mut f = std::fs::File::create(&tmp).map_err(ioerr)?;
                f.write_all(&compressed).map_err(ioerr)?;
                f.sync_all().ok();
            }
            std::fs::rename(&tmp, &path).map_err(ioerr)?;
        }
        let entry = IndexEntry {
            uncompressed: data.len() as u64,
            stored: compressed.len() as u64,
            last_use_ms: now_ms(),
            namespace: meta.namespace.join("/"),
            kind: meta.kind.to_string(),
        };
        let hex = key.hex();
        let ns = meta.namespace.join("/");
        let txn = self.index.begin_write().map_err(dberr)?;
        {
            let mut t = txn.open_table(INDEX_TABLE).map_err(dberr)?;
            t.insert(
                hex.as_bytes(),
                serde_json::to_vec(&entry).map_err(jsonerr)?.as_slice(),
            )
            .map_err(dberr)?;
            let mut nst = txn.open_table(NS_TABLE).map_err(dberr)?;
            let mut list: Vec<String> = match nst.get(ns.as_bytes()).map_err(dberr)? {
                Some(g) => serde_json::from_slice(g.value()).unwrap_or_default(),
                None => Vec::new(),
            };
            if !list.contains(&hex) {
                list.push(hex);
            }
            nst.insert(
                ns.as_bytes(),
                serde_json::to_vec(&list).map_err(jsonerr)?.as_slice(),
            )
            .map_err(dberr)?;
        }
        txn.commit().map_err(dberr)?;
        Ok(key)
    }

    /// Fetch bytes by key, verifying the hash on load (§17.2), decompressing zstd.
    pub fn get(&self, key: &CasKey) -> Result<Option<Vec<u8>>> {
        if self.lookup_entry(key)?.is_none() {
            return Ok(None);
        }
        let path = self.blob_path(key);
        let raw = std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                err(
                    "CAS-CORRUPT",
                    format!("index entry {} has no blob file on disk", key.hex()),
                )
            } else {
                ioerr(e)
            }
        })?;
        let data = zstd::stream::decode_all(raw.as_slice()).map_err(|e| {
            err(
                "CAS-CORRUPT",
                format!("blob {} failed to decompress: {e}", key.hex()),
            )
        })?;
        if CasKey::of(&data) != *key {
            return Err(err(
                "CAS-CORRUPT",
                format!("blob {} failed blake3 verification on load", key.hex()),
            ));
        }
        // Record the access for last-use GC.
        self.touch(key, SystemTime::now())?;
        Ok(Some(data))
    }

    /// Last-use timestamp for GC (§5.4 "garbage collection by last-use time and size cap").
    pub fn touch(&self, key: &CasKey, now: SystemTime) -> Result<()> {
        let hex = key.hex();
        let ms = to_ms(now);
        let txn = self.index.begin_write().map_err(dberr)?;
        {
            let mut t = txn.open_table(INDEX_TABLE).map_err(dberr)?;
            // Clone the entry bytes out first: the AccessGuard borrows the table.
            let existing: Option<Vec<u8>> = t
                .get(hex.as_bytes())
                .map_err(dberr)?
                .map(|g| g.value().to_vec());
            match existing {
                Some(bytes) => {
                    let mut e: IndexEntry = serde_json::from_slice(&bytes).map_err(jsonerr)?;
                    e.last_use_ms = ms;
                    t.insert(
                        hex.as_bytes(),
                        serde_json::to_vec(&e).map_err(jsonerr)?.as_slice(),
                    )
                    .map_err(dberr)?;
                }
                None => {
                    return Err(err(
                        "CAS-NO-ENTRY",
                        format!("touch: key {} is not in the index", hex),
                    ));
                }
            }
        }
        txn.commit().map_err(dberr)?;
        Ok(())
    }

    /// GC: drop entries unused for `max_idle` or when total size exceeds `max_bytes`.
    /// Returns (removed entries, bytes reclaimed). Size pressure evicts in
    /// last-use order (LRU first).
    pub fn gc(&self, max_idle: Duration, max_bytes: u64) -> Result<(usize, u64)> {
        let now = now_ms();
        let idle_ms = max_idle.as_millis() as u64;

        let mut all: Vec<(String, IndexEntry)> = Vec::new();
        {
            let txn = self.index.begin_read().map_err(dberr)?;
            let t = txn.open_table(INDEX_TABLE).map_err(dberr)?;
            for kv in t.iter().map_err(dberr)? {
                let (k, v) = kv.map_err(dberr)?;
                let hex = String::from_utf8_lossy(k.value()).into_owned();
                let entry: IndexEntry = serde_json::from_slice(v.value()).map_err(jsonerr)?;
                all.push((hex, entry));
            }
        }

        let mut removed: Vec<(String, IndexEntry)> = Vec::new();
        let mut keep: Vec<(String, IndexEntry)> = Vec::new();
        for e in all {
            if now.saturating_sub(e.1.last_use_ms) > idle_ms {
                removed.push(e);
            } else {
                keep.push(e);
            }
        }
        let mut total: u64 = keep.iter().map(|e| e.1.stored).sum();
        if total > max_bytes {
            keep.sort_by(|a, b| {
                a.1.last_use_ms
                    .cmp(&b.1.last_use_ms)
                    .then_with(|| a.0.cmp(&b.0))
            });
            let mut i = 0;
            while total > max_bytes && i < keep.len() {
                total -= keep[i].1.stored;
                removed.push(keep[i].clone());
                i += 1;
            }
            keep.drain(0..i);
        }

        let count = removed.len();
        let bytes: u64 = removed.iter().map(|e| e.1.stored).sum();

        let txn = self.index.begin_write().map_err(dberr)?;
        {
            let mut t = txn.open_table(INDEX_TABLE).map_err(dberr)?;
            let mut nst = txn.open_table(NS_TABLE).map_err(dberr)?;
            for (hex, e) in &removed {
                t.remove(hex.as_bytes()).map_err(dberr)?;
                if e.namespace.is_empty() {
                    continue;
                }
                let affected: Vec<String> = match nst.get(e.namespace.as_bytes()).map_err(dberr)? {
                    Some(g) => serde_json::from_slice(g.value()).unwrap_or_default(),
                    None => Vec::new(),
                };
                let mut list: Vec<String> = affected;
                list.retain(|h| h != hex);
                if list.is_empty() {
                    nst.remove(e.namespace.as_bytes()).map_err(dberr)?;
                } else {
                    nst.insert(
                        e.namespace.as_bytes(),
                        serde_json::to_vec(&list).map_err(jsonerr)?.as_slice(),
                    )
                    .map_err(dberr)?;
                }
            }
        }
        txn.commit().map_err(dberr)?;

        // Blob files go after the index commit: a crash here leaves an orphan
        // file, never a missing blob behind a live index entry.
        for (hex, _) in &removed {
            if let Some(k) = key_from_hex(hex) {
                let _ = std::fs::remove_file(self.blob_path(&k));
            }
        }
        Ok((count, bytes))
    }

    /// Total stored bytes (for tests + `oxide cache stats`).
    pub fn total_bytes(&self) -> Result<u64> {
        let txn = self.index.begin_read().map_err(dberr)?;
        let t = txn.open_table(INDEX_TABLE).map_err(dberr)?;
        let mut total = 0u64;
        for kv in t.iter().map_err(dberr)? {
            let (_, v) = kv.map_err(dberr)?;
            let e: IndexEntry = serde_json::from_slice(v.value()).map_err(jsonerr)?;
            total += e.stored;
        }
        Ok(total)
    }

    /// Keys recorded under a namespace (§5.4: namespaces are logical listings
    /// over one content-addressed blob pool).
    pub fn list_namespace(&self, namespace: &str) -> Result<Vec<CasKey>> {
        let txn = self.index.begin_read().map_err(dberr)?;
        let nst = txn.open_table(NS_TABLE).map_err(dberr)?;
        match nst.get(namespace.as_bytes()).map_err(dberr)? {
            Some(g) => {
                let list: Vec<String> = serde_json::from_slice(g.value()).unwrap_or_default();
                Ok(list.iter().filter_map(|h| key_from_hex(h)).collect())
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) fn lookup_entry(&self, key: &CasKey) -> Result<Option<u64>> {
        let hex = key.hex();
        let txn = self.index.begin_read().map_err(dberr)?;
        let t = txn.open_table(INDEX_TABLE).map_err(dberr)?;
        match t.get(hex.as_bytes()).map_err(dberr)? {
            Some(g) => {
                let e: IndexEntry = serde_json::from_slice(g.value()).map_err(jsonerr)?;
                Ok(Some(e.stored))
            }
            None => Ok(None),
        }
    }

    /// Blob file path: `<root>/blobs/<aa>/<hex>.zst`.
    pub(crate) fn blob_path(&self, key: &CasKey) -> PathBuf {
        let hex = key.hex();
        self.root
            .join("blobs")
            .join(&hex[..2])
            .join(format!("{hex}.zst"))
    }
    /// §5-C: store `body` wrapped in the rkyv artifact envelope (see
    /// [`crate::artifacts`]). The envelope carries a `blake3` of the body, so
    /// a corrupt or tampered blob is caught twice on load: once by the CAS's
    /// own key check (`CasKey::of(blob) == key`), and once by the envelope's
    /// `archived_hash`. Returns the CAS key.
    ///
    /// The key is the blake3 of the **sealed** bytes, so the plain
    /// [`ContentStore::get`] path stays valid — it verifies the blob against
    /// the key without knowing the envelope exists.
    pub fn put_sealed(
        &self,
        kind: crate::artifacts::ArtifactKind,
        body: &[u8],
        meta: &CasWriteMeta,
    ) -> Result<CasKey> {
        let sealed = crate::artifacts::seal(kind, body.to_vec())?;
        self.put(&sealed, meta)
    }

    /// §5-C: read a sealed artifact. `None` when the key is not in the CAS.
    /// A `GRAPH-CORRUPT` error when the blob is present but the envelope
    /// fails to open or its `archived_hash` does not match the body.
    ///
    /// A caller whose cache may contain pre-envelope (raw) blobs should fall
    /// back to [`ContentStore::get`] on `GRAPH-CORRUPT` — the key is still
    /// valid, only the format differs.
    pub fn get_sealed(&self, key: &CasKey) -> Result<Option<crate::artifacts::ArtifactEnvelope>> {
        let Some(bytes) = self.get(key)? else {
            return Ok(None);
        };
        crate::artifacts::open(&bytes).map(Some)
    }
}

/// Extra introspection used by tests, `oxide cache stats` and cache tooling.
pub trait ContentStoreExt {
    /// On-disk blob path for a key.
    fn blob_path(&self, key: &CasKey) -> PathBuf;
    /// Compressed size recorded in the index for a key.
    fn stored_bytes(&self, key: &CasKey) -> Result<u64>;
    /// Number of blob files under `root/blobs`.
    fn blob_file_count(&self) -> usize;
}

impl ContentStoreExt for ContentStore {
    fn blob_path(&self, key: &CasKey) -> PathBuf {
        ContentStore::blob_path(self, key)
    }

    fn stored_bytes(&self, key: &CasKey) -> Result<u64> {
        Ok(self.lookup_entry(key)?.unwrap_or(0))
    }

    fn blob_file_count(&self) -> usize {
        let mut n = 0;
        let blobs = self.root.join("blobs");
        let Ok(shards) = std::fs::read_dir(&blobs) else {
            return 0;
        };
        for shard in shards.flatten() {
            if let Ok(files) = std::fs::read_dir(shard.path()) {
                n += files
                    .filter(|e| e.as_ref().map(|e| e.path().is_file()).unwrap_or(false))
                    .count();
            }
        }
        n
    }
}

fn now_ms() -> u64 {
    to_ms(SystemTime::now())
}

fn to_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn key_from_hex(hex: &str) -> Option<CasKey> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(CasKey(out))
}

fn dberr<E: std::fmt::Display>(e: E) -> OxideError {
    err("CAS-DB", format!("redb: {e}"))
}

fn ioerr(e: std::io::Error) -> OxideError {
    err("CAS-IO", format!("io: {e}"))
}

fn jsonerr(e: serde_json::Error) -> OxideError {
    err("CAS-DB", format!("index serialization: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_artifact_roundtrips_and_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ContentStore::open(tmp.path()).unwrap();
        let meta = CasWriteMeta {
            namespace: vec!["test".to_string()],
            kind: "compile",
        };
        let body = b"export const x = 1;".to_vec();
        let key = store
            .put_sealed(crate::artifacts::ArtifactKind::CompiledJs, &body, &meta)
            .unwrap();

        let env = store
            .get_sealed(&key)
            .unwrap()
            .expect("sealed artifact present");
        assert_eq!(env.body, body, "body round-trips byte-identical");
        assert_eq!(
            env.kind,
            crate::artifacts::ArtifactKind::CompiledJs,
            "kind preserved",
        );

        // The plain path still sees opaque bytes whose key verifies — that
        // is what makes this backward compatible with pre-envelope blobs.
        let raw = store.get(&key).unwrap().expect("raw blob present");
        assert_eq!(CasKey::of(&raw), key, "outer key covers the sealed bytes");

        // Corrupt the envelope, re-store under a fresh key (so the CAS's
        // key check passes) and prove the envelope's own verification fires.
        let mut tampered = raw.clone();
        let n = tampered.len();
        tampered[n / 2] ^= 0xFF;
        let bad_key = store.put(&tampered, &meta).expect("store arbitrary bytes");
        match store.get_sealed(&bad_key) {
            Ok(Some(_)) => panic!("tampered envelope must not open cleanly"),
            Ok(None) => panic!("the tampered blob is present, not missing"),
            Err(e) => assert!(
                e.0.code == "GRAPH-CORRUPT",
                "expected GRAPH-CORRUPT, got {}: {}",
                e.0.code,
                e.0.message,
            ),
        }
    }

    #[test]
    fn hex_roundtrip_and_paths() {
        let key = CasKey::of(b"abc");
        let hex = key.hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(key_from_hex(&hex), Some(key.clone()));
        assert_eq!(key_from_hex("zz"), None);
        let store = ContentStore::open(std::env::temp_dir().join("oxide-cas-test-paths")).unwrap();
        let p = store.blob_path(&key);
        assert!(p.to_string_lossy().ends_with(&format!("{hex}.zst")));
    }

    #[test]
    fn namespace_listing_updates_on_gc() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ContentStore::open(tmp.path()).unwrap();
        let k = store
            .put(
                b"ns-gc-data",
                &CasWriteMeta {
                    namespace: vec!["ns".into()],
                    kind: "x",
                },
            )
            .unwrap();
        assert_eq!(store.list_namespace("ns").unwrap(), vec![k.clone()]);
        store
            .touch(&k, SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        store.gc(Duration::from_secs(1800), u64::MAX).unwrap();
        assert!(store.list_namespace("ns").unwrap().is_empty());
    }
}
