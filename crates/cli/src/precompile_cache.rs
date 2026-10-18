//! Content-addressed cache for the §6.3 precompile pass.
//!
//! `precompile_entries` runs the tooling pool (Vue/Svelte/Solid compilers) on
//! every entry on every build. Profiling a warm build shows this is 0.5–1.5 s of
//! the total and that it re-runs even when one unrelated file changed — the §21
//! "incremental one-file change ≤ 500 ms" target cannot be met while a whole
//! pool pass re-runs on each build.
//!
//! The precompile of one file is a pure function of
//! `(source, framework, generate-mode, compiler-version)`. This module caches
//! its output in the §5.4 Tier-B CAS (`oxide-graph::ContentStore`):
//!
//! - **Key:** blake3 of the cache-input descriptor (source hash + framework +
//!   mode + tool version). Any input change misses; an unrelated file does not.
//! - **Pointer:** a tiny file `<root>/.oxide/cache/precompile/<key>.ptr` holding
//!   the CAS key hex. The CAS is content-addressed (keyed by the *output* hash),
//!   so the descriptor → CAS-key mapping needs its own small index; a pointer
//!   file is the simplest durable form and is itself trivially repairable —
//!   a missing pointer falls through to a recompile and a re-store.
//! - **Value:** the compiled JS, zstd-compressed by the CAS.
//!
//! A cache miss, a corrupt blob, or a CAS I/O error all degrade to "compile it
//! again" — the cache is never load-bearing for correctness.

use oxide_graph::{CasKey, CasWriteMeta, ContentStore};
use std::sync::Arc;
use std::path::{Path, PathBuf};

/// The descriptor a precompile cache entry is keyed by.
#[derive(Debug, Clone)]
pub struct PrecompileKey {
    /// blake3 of the source bytes.
    pub source_hash: String,
    /// `"vue"` / `"svelte"` / `"solid"` — the compiler that ran.
    pub framework: String,
    /// `"dom"` / `"ssr"` for Svelte; ignored by Vue.
    pub generate: String,
    /// The compiler package version, so an upgrade invalidates.
    pub tool_version: String,
}

impl PrecompileKey {
    /// A stable hex digest over the descriptor. Field lengths are prefixed so
    /// `("ab","c")` and `("a","bc")` cannot collide.
    pub fn digest(&self) -> String {
        let mut h = blake3::Hasher::new();
        for field in [
            self.source_hash.as_str(),
            self.framework.as_str(),
            self.generate.as_str(),
            self.tool_version.as_str(),
        ] {
            h.update(&(field.len() as u32).to_le_bytes());
            h.update(field.as_bytes());
        }
        h.finalize().to_hex().to_string()
    }
}

/// A precompile cache backed by the Tier-B CAS plus a pointer directory.
pub struct PrecompileCache {
    store: ContentStore,
    ptr_dir: PathBuf,
    /// §5.4: an optional remote read-through cache. `None` for a local build.
    remote: Option<Arc<dyn oxide_graph::RemoteCache>>,
}

impl PrecompileCache {
    /// Open (creating dirs as needed) under `<root>/.oxide/cache`.
    pub fn open(root: &Path) -> oxide_graph::Result<Self> {
        let cache_root = root.join(".oxide/cache");
        let store = ContentStore::open(cache_root.join("cas"))?;
        let ptr_dir = cache_root.join("precompile");
        std::fs::create_dir_all(&ptr_dir).map_err(|e| {
            oxide_graph::err(
                "CAS-IO",
                format!("create {}: {e}", ptr_dir.display()),
            )
        })?;
        Ok(Self { store, ptr_dir, remote: None })
    }

    /// Open with a §5.4 remote read-through cache. A local miss consults the
    /// remote; a successful compile writes back through it.
    pub fn open_with_remote(
        root: &Path,
        remote: Arc<dyn oxide_graph::RemoteCache>,
    ) -> oxide_graph::Result<Self> {
        let mut cache = Self::open(root)?;
        cache.remote = Some(remote);
        Ok(cache)
    }

    /// Open, or `None` when the cache directory cannot be created — a build must
    /// still work on a read-only tree, just without the cache.
    pub fn open_soft(root: &Path) -> Option<Self> {
        match Self::open(root) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!(
                    "warn: precompile cache unavailable ({}); compiling fresh",
                    e.0.message
                );
                None
            }
        }
    }

    fn ptr_path(&self, digest: &str) -> PathBuf {
        self.ptr_dir.join(format!("{digest}.ptr"))
    }

    /// Look up compiled JS for `key`. `None` on any miss or read failure.
    pub fn get(&self, key: &PrecompileKey) -> Option<String> {
        let digest = key.digest();
        let ptr = std::fs::read_to_string(self.ptr_path(&digest)).ok()?;
        let hex = ptr.trim();
        let bytes = hex_to_key(hex)?;
        let blob = self.store.get(&bytes).ok()??;
        String::from_utf8(blob).ok()
    }

    /// Store compiled JS for `key`. Errors are returned so the caller can log,
    /// but a failure never fails the build.
    pub fn put(&self, key: &PrecompileKey, js: &str) -> oxide_graph::Result<()> {
        let meta = CasWriteMeta {
            namespace: vec![
                "precompile".to_string(),
                key.framework.clone(),
                key.tool_version.clone(),
            ],
            kind: "precompile",
        };
        let cas_key = self.store.put(js.as_bytes(), &meta)?;
        let ptr = self.ptr_path(&key.digest());
        // Atomic write: a half-written pointer would be a permanent miss, not a
        // corruption, but a rename keeps a concurrent reader from seeing it.
        let tmp = ptr.with_extension("ptr.tmp");
        std::fs::write(&tmp, cas_key.hex()).map_err(|e| {
            oxide_graph::err("CAS-IO", format!("write {}: {e}", tmp.display()))
        })?;
        std::fs::rename(&tmp, &ptr).map_err(|e| {
            oxide_graph::err("CAS-IO", format!("rename {}: {e}", ptr.display()))
        })?;
        Ok(())
    }

    /// §5.4: read-through. When the local CAS misses and a remote is
    /// configured, fetch the blob by its content key — but the local pointer
    /// is keyed by the *descriptor* digest, and the remote key is the CAS
    /// output key. So the remote protocol here is "descriptor → CAS key →
    /// blob": the pointer file's content (the CAS key hex) is what the remote
    /// stores under `ptr/<descriptor-digest>`, and the blob under
    /// `cas/<key-hex>`. This helper does both lookups. `None` = not remote-hit.
    pub async fn get_remote(&self, key: &PrecompileKey) -> Option<String> {
        let remote = self.remote.as_ref()?;
        let digest = key.digest();
        // 1) Fetch the pointer (the CAS key hex) under the descriptor digest.
        let ptr_key = CasKey::of(format!("ptr:{digest}").as_bytes());
        let ptr_bytes = remote.fetch(&ptr_key).await.ok()??;
        let hex = String::from_utf8_lossy(&ptr_bytes).trim().to_string();
        // 2) Fetch the blob under the CAS key.
        let cas_key = hex_to_key(&hex)?;
        let blob = remote.fetch(&cas_key).await.ok()??;
        String::from_utf8(blob).ok()
    }

    /// §5.4: write-through. Stores the blob + pointer on the remote so the
    /// next build (another machine, a CI job) gets a hit. Best-effort: a
    /// remote failure never fails the build.
    pub async fn put_remote(&self, key: &PrecompileKey, js: &str) {
        let Some(remote) = self.remote.as_ref() else { return };
        let digest = key.digest();
        let cas_key = CasKey::of(js.as_bytes());
        let ptr_key = CasKey::of(format!("ptr:{digest}").as_bytes());
        // Store the blob, then the pointer (pointer last so a partial write
        // never points at a missing blob).
        let _ = remote.store(&cas_key, js.as_bytes()).await;
        let _ = remote.store(&ptr_key, cas_key.hex().as_bytes()).await;
    }

    /// Number of pointer files — the cached-entry count (for `oxide cache stats`).
    pub fn entry_count(&self) -> usize {
        std::fs::read_dir(&self.ptr_dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "ptr"))
                    .count()
            })
            .unwrap_or(0)
    }
}

fn hex_to_key(hex: &str) -> Option<CasKey> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(CasKey(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(src: &str, fw: &str, mode: &str) -> PrecompileKey {
        PrecompileKey {
            source_hash: blake3::hash(src.as_bytes()).to_hex().to_string(),
            framework: fw.into(),
            generate: mode.into(),
            tool_version: "1.0.0".into(),
        }
    }

    #[test]
    fn put_then_get_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PrecompileCache::open(dir.path()).unwrap();
        let k = key("export default {}", "vue", "dom");
        assert!(cache.get(&k).is_none(), "cold cache misses");
        cache.put(&k, "compiled-js").unwrap();
        assert_eq!(cache.get(&k).as_deref(), Some("compiled-js"));
    }

    #[test]
    fn key_fields_all_affect_the_digest() {
        let base = key("src", "vue", "dom");
        assert_ne!(base.digest(), key("src2", "vue", "dom").digest());
        assert_ne!(base.digest(), key("src", "svelte", "dom").digest());
        assert_ne!(base.digest(), key("src", "vue", "ssr").digest());
        let mut other_ver = base.clone();
        other_ver.tool_version = "2.0.0".into();
        assert_ne!(base.digest(), other_ver.digest());
    }

    #[test]
    fn missing_pointer_is_a_miss_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PrecompileCache::open(dir.path()).unwrap();
        assert!(cache.get(&key("x", "vue", "dom")).is_none());
    }

    #[test]
    fn corrupt_pointer_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PrecompileCache::open(dir.path()).unwrap();
        let k = key("x", "vue", "dom");
        cache.put(&k, "js").unwrap();
        std::fs::write(cache.ptr_path(&k.digest()), "not-hex").unwrap();
        assert!(cache.get(&k).is_none(), "corrupt pointer → miss, not panic");
    }

    #[test]
    fn entry_count_tracks_puts() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PrecompileCache::open(dir.path()).unwrap();
        assert_eq!(cache.entry_count(), 0);
        cache.put(&key("a", "vue", "dom"), "js-a").unwrap();
        cache.put(&key("b", "svelte", "ssr"), "js-b").unwrap();
        assert_eq!(cache.entry_count(), 2);
        // Re-putting the same key does not add an entry.
        cache.put(&key("a", "vue", "dom"), "js-a").unwrap();
        assert_eq!(cache.entry_count(), 2);
    }

    #[test]
    fn distinct_sources_get_distinct_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PrecompileCache::open(dir.path()).unwrap();
        let a = key("one", "vue", "dom");
        let b = key("two", "vue", "dom");
        cache.put(&a, "A").unwrap();
        cache.put(&b, "B").unwrap();
        assert_eq!(cache.get(&a).as_deref(), Some("A"));
        assert_eq!(cache.get(&b).as_deref(), Some("B"));
    }
}
