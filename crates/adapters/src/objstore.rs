//! The object-store deploy target (spec v0.2, §V2-F11 remaining half): the
//! `s3://` / `r2://` [`DeployStore`] over the `object_store` crate.
//!
//! # The seam
//!
//! [`ObjectStoreStore`] is the second consumer of the [`DeployStore`]
//! trait (the `dir:` target's [`DirStore`] was the first): every operation
//! — ledger read/write, object put/delete, the `current` pointer — goes
//! through the object-store `ObjectStore` trait, so the delta engine
//! (`crate::delta`) runs unchanged against S3, R2, a local directory or
//! the in-memory backend. [`deploy_target_from_url`] is the spelling the
//! CLI hands a target string: `s3://bucket/prefix`, `r2://bucket/prefix`
//! (S3 API against an endpoint override, the query parameter carries it),
//! `file://` and `mem://` (the network-free backends the tests, the bench
//! and the walkthrough run over).
//!
//! # The pointer swap on an object store
//!
//! `DirStore` swaps the pointer with a temp file plus rename — atomic
//! within one filesystem. Object stores have no rename; the equivalent
//! atomicity primitive is the conditional put: the swap reads the
//! pointer's current version (an etag) and writes with
//! `PutMode::Update(that version)` — or `PutMode::Create` on a fresh
//! store. A write against a moved-or-taken version refuses with
//! [`DEP-POINTER`]: another deploy swapped first, and this deploy's
//! `current` write never lands. That is the honest contract an S3-class
//! store can offer — the same shape the cache crate's B-11 tag markers
//! document — and the pins in `tests/object_store_tdd.rs` hold it.
//!
//! # Sync over async
//!
//! `object_store`'s API is async; [`DeployStore`] is sync (the engine and
//! its pins are). The bridge is a dedicated current-thread tokio runtime
//! the store owns: every method `block_on`s its future.
//!
//! # Panics
//!
//! [`ObjectStoreStore`] methods must not be called from inside an async
//! runtime — `block_on` panics there. The engine's callers (the CLI, the
//! tests) are synchronous.
//!
//! # Errors
//!
//! Everything surfaces as the crate's [`OxideError`] with stable `DEP-*`
//! codes (registered in `docs/errors.toml`): [`DEP-URL`] for a target
//! string that does not parse or names a backend this crate cannot build,
//! [`DEP-MANIFEST`] / [`DEP-POINTER`] for corrupt ledger/pointer bytes or
//! a lost swap race, [`DEP-IO`] for every other store failure.

use std::sync::Arc;

use futures::TryStreamExt;
use object_store::path::Path as ObjPath;
use object_store::{
    Error as ObjError, GetOptions, ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload,
    UpdateVersion,
};

use crate::delta::{
    DeployStore, MANIFEST_NAME, POINTER_NAME, parse_manifest, parse_pointer, validate_build_id,
    validate_object_path,
};
use crate::{OxideError, Result, err};

/// The `s3://` / `r2://` / `file://` / `mem://` deploy target: a
/// [`DeployStore`] over any `object_store` backend.
///
/// Construct with [`ObjectStoreStore::new`] (wrap any `Arc<dyn
/// ObjectStore>`, the cache crate's `with_store` shape),
/// [`ObjectStoreStore::new_in_memory`] (the network-free backend) or
/// [`deploy_target_from_url`] (the target-string spelling).
#[derive(Debug)]
pub struct ObjectStoreStore {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    rt: tokio::runtime::Runtime,
}

/// Parse a target URL into its deploy store — the free-function spelling
/// of [`ObjectStoreStore::from_url`] (the shape `oxide deploy` calls with
/// the target string).
///
/// # Errors
///
/// [`DEP-URL`] — the same refusals [`ObjectStoreStore::from_url`] makes.
pub fn deploy_target_from_url(url: &str) -> crate::Result<ObjectStoreStore> {
    ObjectStoreStore::from_url(url)
}

impl ObjectStoreStore {
    /// A store over `store`, rooted at `prefix` (an empty prefix is the
    /// store root — every object key maps verbatim; a non-empty one
    /// namespaces the deploy under it, so `s3://bucket/releases` deploys
    /// under the `releases/` key prefix).
    ///
    /// # Errors
    ///
    /// [`DEP-IO`] when the object-store runtime cannot build (exhausted
    /// threads); [`DEP-URL`] when the prefix is not a valid store prefix.
    pub fn new(store: Arc<dyn ObjectStore>, prefix: &str) -> Result<Self> {
        validate_prefix(prefix)?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| err("DEP-IO", format!("the object-store runtime: {e}")))?;
        Ok(Self {
            store,
            prefix: prefix.to_string(),
            rt,
        })
    }

    /// The network-free in-memory backend — the shape the TDD pins, the
    /// criterion bench and the walkthrough run over.
    ///
    /// # Errors
    ///
    /// [`DEP-IO`] when the object-store runtime cannot build.
    pub fn new_in_memory() -> Result<Self> {
        Self::new(Arc::new(object_store::memory::InMemory::new()), "")
    }

    /// Parse a target URL and build its backend (the spelling `oxide
    /// deploy` hands the target string).
    ///
    /// The schemes: `s3://bucket/prefix` (credentials and region from the
    /// standard `AWS_*` environment, the cache crate's convention),
    /// `r2://bucket/prefix` (the S3 API against an R2 endpoint — the
    /// `endpoint=` query parameter carries it; without one the target
    /// refuses), `file:///abs/path` (a local directory through
    /// `object_store`'s local backend) and `mem://label` (in-memory). The
    /// optional `endpoint=` query parameter also overrides the endpoint
    /// for `s3://` (self-hosted S3).
    ///
    /// Construction makes no network round trip: the S3 client builds
    /// from the environment and only fails on first use.
    ///
    /// # Errors
    ///
    /// [`DEP-URL`] for an unknown scheme, a bucket-less `s3`/`r2` target,
    /// an `r2://` target without an endpoint, a `file://` target whose
    /// path is not usable, or a backend the builder refuses.
    pub fn from_url(url: &str) -> Result<Self> {
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| bad_url(url, "targets are `scheme://rest`"))?;
        let (target, query) = match rest.split_once('?') {
            Some((t, q)) => (t, Some(q)),
            None => (rest, None),
        };
        let mut endpoint = None;
        if let Some(q) = query {
            for kv in q.split('&') {
                if let Some((k, v)) = kv.split_once('=')
                    && k == "endpoint"
                {
                    endpoint = Some(v.to_string());
                }
            }
        }
        match scheme {
            "mem" => {
                if target.contains('/') {
                    return Err(bad_url(url, "mem:// takes a bare label, no prefix"));
                }
                Self::new_in_memory()
            }
            "file" => {
                let fs = object_store::local::LocalFileSystem::new_with_prefix(target)
                    .map_err(|e| err("DEP-URL", format!("file target `{target}`: {e}")))?;
                Self::new(Arc::new(fs), "")
            }
            "s3" | "r2" => {
                let (bucket, prefix) = match target.split_once('/') {
                    Some((b, p)) => (b, p),
                    None => (target, ""),
                };
                if bucket.is_empty() {
                    return Err(bad_url(url, "the bucket name is empty"));
                }
                if scheme == "r2" && endpoint.is_none() {
                    return Err(err(
                        "DEP-URL",
                        format!(
                            "`{url}`: an r2 target needs its S3 endpoint — append `?endpoint=https://<account>.r2.cloudflarestorage.com`"
                        ),
                    ));
                }
                let mut builder =
                    object_store::aws::AmazonS3Builder::from_env().with_bucket_name(bucket);
                if let Some(ep) = endpoint {
                    builder = builder.with_endpoint(ep);
                }
                let store = builder
                    .build()
                    .map_err(|e| err("DEP-URL", format!("s3 client for `{url}`: {e}")))?;
                Self::new(Arc::new(store), prefix)
            }
            _ => Err(bad_url(url, "known schemes are s3, r2, file and mem")),
        }
    }

    /// The store prefix this deploy namespaces under (`""` = the store
    /// root).
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The pointer's current version — what a conditional swap must name
    /// to land (`None` on a fresh store). Exposed so the swap-race
    /// contract is testable: a caller reads the version, the store moves,
    /// the swap against the stale version refuses.
    ///
    /// # Errors
    ///
    /// [`DEP-IO`] when the head read fails for a reason other than
    /// absence.
    pub fn pointer_version(&self) -> Result<Option<UpdateVersion>> {
        let path = self.key(POINTER_NAME)?;
        let meta = match self.rt.block_on(
            self.store
                .get_opts(&path, GetOptions::new().with_head(true)),
        ) {
            Ok(res) => res.meta,
            Err(ObjError::NotFound { .. }) => return Ok(None),
            Err(e) => {
                return Err(err("DEP-IO", format!("cannot head {POINTER_NAME}: {e}")));
            }
        };
        Ok(Some(UpdateVersion {
            e_tag: meta.e_tag,
            version: meta.version,
        }))
    }

    /// The conditional pointer swap: write `build_id` only if the pointer
    /// still carries `expected` (`None` = the fresh-store case, the write
    /// must create). [`DeployStore::put_pointer`] is
    /// `swap_pointer(build_id, pointer_version()?)` — read, then swap.
    ///
    /// # Errors
    ///
    /// [`DEP-BUILDID`] when `build_id` is not lowercase hex;
    /// [`DEP-POINTER`] when the store moved underneath the swap (the
    /// version is stale, or the pointer exists where absence was
    /// expected) — the refused swap never flips `current`;
    /// [`DEP-IO`] for every other write failure.
    pub fn swap_pointer(&self, build_id: &str, expected: Option<&UpdateVersion>) -> Result<()> {
        validate_build_id(build_id)?;
        let path = self.key(POINTER_NAME)?;
        let mode = match expected {
            Some(v) => PutMode::Update(v.clone()),
            None => PutMode::Create,
        };
        let payload = PutPayload::from(format!("{build_id}\n"));
        match self
            .rt
            .block_on(self.store.put_opts(&path, payload, PutOptions::from(mode)))
        {
            Ok(_) => Ok(()),
            Err(e @ (ObjError::AlreadyExists { .. } | ObjError::Precondition { .. })) => Err(err(
                "DEP-POINTER",
                format!(
                    "{POINTER_NAME} moved underneath this deploy (a concurrent swap won the race): {e}"
                ),
            )),
            Err(e) => Err(err("DEP-IO", format!("cannot swap {POINTER_NAME}: {e}"))),
        }
    }

    /// Every object key currently in the store, sorted (the deterministic
    /// listing the layout assertions and a future prune listing walk).
    ///
    /// # Errors
    ///
    /// [`DEP-IO`] when the list fails.
    pub fn list_keys(&self) -> Result<Vec<String>> {
        let keys: std::result::Result<Vec<String>, ObjError> = self.rt.block_on(async {
            self.store
                .list(None)
                .map_ok(|meta| decode_key(&meta.location, &self.prefix))
                .try_collect()
                .await
        });
        let mut keys = keys.map_err(|e| err("DEP-IO", format!("cannot list the store: {e}")))?;
        keys.sort();
        Ok(keys)
    }

    /// The validated object key for a store-relative `path`, under the
    /// store prefix.
    fn key(&self, path: &str) -> Result<ObjPath> {
        validate_object_path(path)?;
        Ok(ObjPath::from(join_prefix(&self.prefix, path)))
    }
}

/// The full key `prefix` + `path` (no double slash; an empty prefix is
/// the verbatim path).
fn join_prefix(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        path.to_string()
    } else {
        format!("{prefix}/{path}")
    }
}

/// The store-relative key: the listing's full key with the store prefix
/// (and its slash) stripped.
fn decode_key(location: &ObjPath, prefix: &str) -> String {
    let full = location.to_string();
    if prefix.is_empty() {
        return full;
    }
    full.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(&full)
        .to_string()
}

fn bad_url(url: &str, why: &str) -> OxideError {
    err(
        "DEP-URL",
        format!("`{url}` is not a deploy target URL: {why}"),
    )
}

/// A store prefix is an optional relative slash-separated path — the same
/// shape the store keys take, minus the emptiness (`""` is the root).
fn validate_prefix(prefix: &str) -> Result<()> {
    if prefix.is_empty() {
        return Ok(());
    }
    validate_object_path(prefix).map_err(|mut e| {
        e.0.message = format!("the deploy target prefix `{prefix}`: {}", e.0.message);
        e
    })
}

impl DeployStore for ObjectStoreStore {
    fn load_manifest(&self) -> crate::Result<Option<crate::delta::RemoteManifest>> {
        let path = self.key(MANIFEST_NAME)?;
        let bytes = match self
            .rt
            .block_on(async { self.store.get(&path).await?.bytes().await })
        {
            Ok(bytes) => bytes,
            Err(ObjError::NotFound { .. }) => return Ok(None),
            Err(e) => {
                return Err(err("DEP-IO", format!("cannot read {MANIFEST_NAME}: {e}")));
            }
        };
        parse_manifest(&bytes).map(Some)
    }

    fn store_manifest(&self, manifest: &crate::delta::RemoteManifest) -> crate::Result<()> {
        let body = serde_json::to_string_pretty(manifest)
            .map_err(|e| err("DEP-MANIFEST", format!("serialize {MANIFEST_NAME}: {e}")))?;
        self.put(MANIFEST_NAME, format!("{body}\n").as_bytes())
    }

    fn put(&self, path: &str, bytes: &[u8]) -> crate::Result<()> {
        let key = self.key(path)?;
        self.rt
            .block_on(self.store.put(&key, PutPayload::from(bytes.to_vec())))
            .map_err(|e| err("DEP-IO", format!("cannot put `{path}`: {e}")))?;
        Ok(())
    }

    fn delete(&self, path: &str) -> crate::Result<()> {
        let key = self.key(path)?;
        // Absence is success (a mark may name an already-gone file), and
        // so is the delete that removed it.
        match self.rt.block_on(self.store.delete(&key)) {
            Ok(()) | Err(ObjError::NotFound { .. }) => Ok(()),
            Err(e) => Err(err("DEP-IO", format!("cannot delete `{path}`: {e}"))),
        }
    }

    fn put_pointer(&self, build_id: &str) -> crate::Result<()> {
        let expected = self.pointer_version()?;
        self.swap_pointer(build_id, expected.as_ref())
    }

    fn read_pointer(&self) -> crate::Result<Option<String>> {
        let path = self.key(POINTER_NAME)?;
        let bytes = match self
            .rt
            .block_on(async { self.store.get(&path).await?.bytes().await })
        {
            Ok(bytes) => bytes,
            Err(ObjError::NotFound { .. }) => return Ok(None),
            Err(e) => {
                return Err(err("DEP-IO", format!("cannot read {POINTER_NAME}: {e}")));
            }
        };
        parse_pointer(&bytes).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_parser_extracts_scheme_bucket_prefix_and_endpoint() {
        // The structural extraction the pins drive end to end: mem, file,
        // s3 with and without prefix, r2 with an endpoint.
        assert!(ObjectStoreStore::from_url("mem://walkthrough").is_ok());
        assert!(ObjectStoreStore::from_url("mem://").is_ok());
        let s3 = ObjectStoreStore::from_url("s3://bucket").expect("bare bucket");
        assert_eq!(s3.prefix(), "");
        let s3 = ObjectStoreStore::from_url("s3://bucket/site/root").expect("bucket + prefix");
        assert_eq!(s3.prefix(), "site/root");
        let r2 = ObjectStoreStore::from_url(
            "r2://bucket?endpoint=https://acct.r2.cloudflarestorage.com",
        )
        .expect("r2 with endpoint");
        assert_eq!(r2.prefix(), "");
        // Query parameters the parser does not know ride along harmlessly.
        let s3 = ObjectStoreStore::from_url("s3://bucket?region=us-east-2").expect("s3 + region");
        assert_eq!(s3.prefix(), "");
    }

    #[test]
    fn join_prefix_and_decode_key_round_trip() {
        assert_eq!(join_prefix("", "assets/a.js"), "assets/a.js");
        assert_eq!(join_prefix("site", "assets/a.js"), "site/assets/a.js");
        assert_eq!(
            decode_key(&ObjPath::from("site/assets/a.js"), "site"),
            "assets/a.js"
        );
        assert_eq!(decode_key(&ObjPath::from("assets/a.js"), ""), "assets/a.js");
    }
}
