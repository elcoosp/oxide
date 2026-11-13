//! The delta-deploy engine (spec v0.2, §V2-F11): plan and execute a deploy
//! that uploads only what changed and swaps atomically.
//!
//! # The seam
//!
//! Everything the store can do — read the previous manifest, put one
//! object, delete one object, read/write the `current` pointer — lives
//! behind the object-safe [`DeployStore`] trait. The pure [`plan`] and the
//! orchestrators ([`deploy_dir`], [`prune`], [`rollback`]) never touch
//! `std::fs` directly; [`DirStore`] is the `dir:/path` target's impl over
//! `std::fs`, and [`crate::objstore::ObjectStoreStore`] is the
//! `s3://` / `r2://` target's impl over `object_store` — the second
//! consumer of the same trait, behind the crate's `s3` feature. The trait
//! keeps the whole engine testable against in-memory fakes — including the
//! spec's "a failure mid-upload leaves `current` untouched" pin, which
//! needs a store that can fail on demand.
//!
//! # The store layout
//!
//! - Immutable, content-hashed files (the `assets/` and `_oxide/` trees the
//!   bundler emits — [`is_immutable`]) are uploaded at their rel path, so
//!   identical bytes across builds are stored once and old tabs keep
//!   loading them.
//! - Every non-hashed file (HTML, manifests, `_headers`) mirrors into
//!   `releases/<build-id>/<rel>`, so a release is a self-contained prefix:
//!   a client that read the old pointer keeps reading a consistent old
//!   tree, never a half-updated mix.
//! - `manifest.json` is the upload ledger: the build id whose files the
//!   store holds (`build_id` + `files`), the id `current` named before the
//!   last swap (`previous_build_id`, the rollback target) and the pending
//!   deletions (`pending_delete`).
//! - `current` is a small pointer object containing the build id — the
//!   atomicity contract: it is written LAST, after every upload and the
//!   manifest, so a deploy that dies mid-flight leaves the production
//!   build serving.
//!
//! # Order of operations (§V2-F11)
//!
//! 1. Upload changed immutable files at their rel path.
//! 2. Upload every non-hashed file under `releases/<build-id>/`.
//! 3. Rewrite `manifest.json` (the ledger catches up: new files map, the
//!    outgoing release's non-hashed copies become pending deletions, the
//!    pointer's old value becomes `previous_build_id`).
//! 4. Write `current` — one small write, the switch itself.
//!
//! After `deploy.skewWindow` (the F10 default lives in
//! [`DEFAULT_SKEW_WINDOW`]) a second invocation — [`prune`] — deletes the
//! pending marks older than the window and old release copies. Rollback is
//! rewriting `current` to `previous_build_id` ([`rollback`]).
//!
//! Pending deletions keep their EARLIEST mark across re-marks, so a file
//! superseded by three deploys in an hour does not outlive its window by
//! being re-stamped. And [`prune`] never deletes a pending mark under the
//! release prefix the pointer currently names: after a rollback, the
//! rolled-back-to release must stay alive even if its marks have expired.
//!
//! # Errors
//!
//! Everything surfaces as the crate's [`OxideError`] with stable `DEP-*`
//! codes (registered in `docs/errors.toml`): [`DEP-ROOT`] for a bad dist
//! root, [`DEP-IO`] for store/scan I/O and invalid object paths,
//! [`DEP-MANIFEST`] for a corrupt or future-versioned ledger,
//! [`DEP-POINTER`] for a corrupt `current`, [`DEP-BUILDID`] for a build id
//! outside the A04 shape and [`DEP-ROLLBACK`] for a rollback with nothing
//! to roll back to.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{OxideError, err};

/// The pointer object's name — the last write of every deploy (§V2-F11
/// step 3 of the order of operations).
pub const POINTER_NAME: &str = "current";

/// The upload ledger's name (`manifest.json` at the store root).
pub const MANIFEST_NAME: &str = "manifest.json";

/// The release prefix root: every non-hashed file mirrors into
/// `releases/<build-id>/<rel>` so a release is self-contained.
pub const RELEASE_ROOT: &str = "releases";

/// The manifest schema version this engine writes and accepts.
pub const MANIFEST_VERSION: u64 = 1;

/// The rel-path prefixes whose files are immutable, content-hashed build
/// output (the bundler's hashed chunk trees). Immutable files are stored at
/// their rel path — identical bytes across builds are stored once — and
/// removed only through `pending_delete` after the skew window.
pub const IMMUTABLE_PREFIXES: &[&str] = &["assets/", "_oxide/"];

/// The F10 default skew window (`deploy.skewWindow`, 15 minutes): how long
/// superseded files stay on the store before [`prune`] removes them.
pub const DEFAULT_SKEW_WINDOW: Duration = Duration::from_mins(15);

/// `true` when `rel` is immutable, content-hashed build output (see
/// [`IMMUTABLE_PREFIXES`]): it is stored at its rel path and shared across
/// releases. Everything else mirrors under [`RELEASE_ROOT`].
#[must_use]
pub fn is_immutable(rel: &str) -> bool {
    IMMUTABLE_PREFIXES.iter().any(|p| rel.starts_with(p))
}

/// One local file the scan produced: the blake3 content digest (the D6
/// hashing rule) and its size in bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// The blake3 content digest, lowercase hex.
    pub hash: String,
    /// The file size in bytes.
    pub size: u64,
}

/// The §V2-F11 plan: pure, no I/O, no clock reads.
///
/// `upload` / `keep` / `delete_later` are rel paths in sorted (map) order;
/// `bytes_up` sums the sizes of the changed files (the upload list). The
/// ledger view: a rel path whose hash matches the store's ledger is
/// `keep`, everything else uploads; rel paths the store holds but the
/// build no longer produces are `delete_later` (they are removed after
/// the skew window, never during it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Rel paths whose content hash differs from the ledger (or that are
    /// new) — the files this deploy uploads.
    pub upload: Vec<String>,
    /// Rel paths whose content hash matches the ledger — the store
    /// already holds these bytes.
    pub keep: Vec<String>,
    /// Rel paths the ledger holds but the build no longer produces —
    /// deleted after the skew window by [`prune`], never during the
    /// deploy.
    pub delete_later: Vec<String>,
    /// The sum of the changed files' sizes: the bytes this deploy bills.
    pub bytes_up: u64,
}

/// The store-side upload ledger (§V2-F11's remote manifest).
///
/// The field names are `snake_case`, matching the crate's emitted JSON (`deploy.json`)
/// convention. `files` maps rel paths to content hashes — the ledger the
/// next [`plan`] diffs against; for non-hashed files the hash names the
/// bytes under `releases/<build_id>/<rel>` of the build `build_id` names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteManifest {
    /// The schema version ([`MANIFEST_VERSION`]); a different value is a
    /// [`DEP-MANIFEST`] refusal (fail closed on an unknown future).
    pub version: u64,
    /// The build id whose files the ledger's `files` map describes.
    pub build_id: String,
    /// The id `current` named before the last successful swap — the
    /// [`rollback`] target.
    pub previous_build_id: Option<String>,
    /// rel path → content hash: the upload ledger.
    pub files: BTreeMap<String, String>,
    /// Files removed from the store (or waiting to be), each with the
    /// epoch millis of its EARLIEST pending mark (the skew window runs
    /// from the first mark, not the last).
    pub pending_delete: Vec<PendingDelete>,
}

/// One file waiting out its skew window before deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDelete {
    /// The store-relative object path (for non-hashed files this carries
    /// the release prefix).
    pub path: String,
    /// The epoch millis of the earliest mark (see [`RemoteManifest`]).
    pub marked_at_ms: u64,
}

/// Everything a deploy store can do (§V2-F11: the `s3://` / `r2://` /
/// `dir:` targets differ exactly here).
///
/// Methods take `&self`: implementations keep their bookkeeping inside
/// the store root, and fakes use interior mutability (the injected-failure
/// test builds one with a `Mutex` counter). All paths are store-relative
/// object keys; implementations validate them (see [`DirStore`]'s
/// traversal refusal) and surface failures as coded `DEP-*` diagnostics.
pub trait DeployStore {
    /// Read the upload ledger; `None` when the store is fresh (first
    /// deploy). A corrupt ledger is an error, never `None`.
    ///
    /// # Errors
    ///
    /// [`DEP-MANIFEST`] when the ledger exists but does not parse or
    /// carries an unknown version; [`DEP-IO`] when it exists but cannot
    /// be read.
    fn load_manifest(&self) -> Result<Option<RemoteManifest>, OxideError>;

    /// Persist the upload ledger (before the pointer swap, §V2-F11 step
    /// 3).
    ///
    /// # Errors
    ///
    /// [`DEP-IO`] when the write fails.
    fn store_manifest(&self, manifest: &RemoteManifest) -> Result<(), OxideError>;

    /// Upload one object, creating parent prefixes as needed.
    ///
    /// # Errors
    ///
    /// [`DEP-IO`] when `path` is not a valid store key or the write
    /// fails.
    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), OxideError>;

    /// Delete one object; deleting an absent object succeeds (prune runs
    /// concurrently with nothing, but a mark may name an already-gone
    /// file).
    ///
    /// # Errors
    ///
    /// [`DEP-IO`] when `path` is not a valid store key or the delete
    /// fails for a reason other than absence.
    fn delete(&self, path: &str) -> Result<(), OxideError>;

    /// Write the `current` pointer atomically: readers see either the old
    /// build id or the new one, never a torn value.
    ///
    /// # Errors
    ///
    /// [`DEP-BUILDID`] when `build_id` is not lowercase hex;
    /// [`DEP-IO`] when the write fails.
    fn put_pointer(&self, build_id: &str) -> Result<(), OxideError>;

    /// Read the `current` pointer; `None` before the first deploy.
    ///
    /// # Errors
    ///
    /// [`DEP-POINTER`] when the pointer exists but is corrupt;
    /// [`DEP-IO`] when it exists but cannot be read.
    fn read_pointer(&self) -> Result<Option<String>, OxideError>;
}

/// The `dir:/path` target: a plain directory tree on local disk, laid out
/// exactly like the object store (§V2-F11's third target spelling). The
/// pointer swap is a temp-file write plus `rename`, atomic within one
/// filesystem.
#[derive(Debug, Clone)]
pub struct DirStore {
    root: std::path::PathBuf,
}

impl DirStore {
    /// A store rooted at `root` (created lazily, on first write).
    #[must_use]
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The absolute path of a validated store key.
    fn resolve(&self, path: &str) -> Result<std::path::PathBuf, OxideError> {
        validate_object_path(path)?;
        Ok(self.root.join(path))
    }
}

impl DeployStore for DirStore {
    fn load_manifest(&self) -> Result<Option<RemoteManifest>, OxideError> {
        let path = self.root.join(MANIFEST_NAME);
        let Ok(bytes) = std::fs::read(&path) else {
            // Distinguish absence from a read failure: absence is the
            // fresh store, any other error is loud.
            if path.exists() {
                return Err(err(
                    "DEP-IO",
                    format!("cannot read {}: permission denied", path.display()),
                ));
            }
            return Ok(None);
        };
        parse_manifest(&bytes).map(Some)
    }

    fn store_manifest(&self, manifest: &RemoteManifest) -> Result<(), OxideError> {
        let body = serde_json::to_string_pretty(manifest)
            .map_err(|e| err("DEP-MANIFEST", format!("serialize {MANIFEST_NAME}: {e}")))?;
        self.put(MANIFEST_NAME, format!("{body}\n").as_bytes())
    }

    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), OxideError> {
        let dst = self.resolve(path)?;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| err("DEP-IO", format!("cannot create {}: {e}", parent.display())))?;
        }
        std::fs::write(&dst, bytes)
            .map_err(|e| err("DEP-IO", format!("cannot write {}: {e}", dst.display())))?;
        Ok(())
    }

    fn delete(&self, path: &str) -> Result<(), OxideError> {
        let dst = self.resolve(path)?;
        match std::fs::remove_file(&dst) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(err(
                "DEP-IO",
                format!("cannot delete {}: {e}", dst.display()),
            )),
        }
    }

    fn put_pointer(&self, build_id: &str) -> Result<(), OxideError> {
        validate_build_id(build_id)?;
        std::fs::create_dir_all(&self.root).map_err(|e| {
            err(
                "DEP-IO",
                format!("cannot create {}: {e}", self.root.display()),
            )
        })?;
        // Temp file + rename: the atomic small write the order of
        // operations ends with. Same filesystem, so the rename is atomic.
        let tmp = self.root.join(format!(".{POINTER_NAME}.tmp"));
        std::fs::write(&tmp, format!("{build_id}\n"))
            .map_err(|e| err("DEP-IO", format!("cannot write {}: {e}", tmp.display())))?;
        let dst = self.root.join(POINTER_NAME);
        std::fs::rename(&tmp, &dst)
            .map_err(|e| err("DEP-IO", format!("cannot swap {}: {e}", dst.display())))?;
        Ok(())
    }

    fn read_pointer(&self) -> Result<Option<String>, OxideError> {
        let path = self.root.join(POINTER_NAME);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(err(
                    "DEP-IO",
                    format!("cannot read {}: {e}", path.display()),
                ));
            }
        };
        parse_pointer(&bytes).map(Some)
    }
}

/// Parse ledger bytes (§V2-F11's `manifest.json`): UTF-8 JSON of the
/// schema version this engine writes — a UTF-8 failure, a parse failure
/// or an unknown version is a [`DEP-MANIFEST`] refusal, never a
/// best-effort parse (fail closed on an unknown future). The shared
/// parser behind [`DirStore::load_manifest`] and the object-store target.
pub(crate) fn parse_manifest(bytes: &[u8]) -> Result<RemoteManifest, OxideError> {
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|e| err("DEP-MANIFEST", format!("{MANIFEST_NAME} is not UTF-8: {e}")))?;
    let manifest: RemoteManifest = serde_json::from_str(&text).map_err(|e| {
        err(
            "DEP-MANIFEST",
            format!("{MANIFEST_NAME} does not parse: {e}"),
        )
    })?;
    if manifest.version != MANIFEST_VERSION {
        return Err(err(
            "DEP-MANIFEST",
            format!(
                "{MANIFEST_NAME} carries version {} but this engine writes {MANIFEST_VERSION}",
                manifest.version
            ),
        ));
    }
    Ok(manifest)
}

/// Parse pointer bytes (§V2-F11's `current`): one build id, UTF-8,
/// newline-terminated. An empty or non-build-id value is a
/// [`DEP-POINTER`] refusal (torn write or truncation). The shared parser
/// behind [`DirStore::read_pointer`] and the object-store target.
pub(crate) fn parse_pointer(bytes: &[u8]) -> Result<String, OxideError> {
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|e| err("DEP-POINTER", format!("{POINTER_NAME} is not UTF-8: {e}")))?;
    let id = text.trim_end_matches(['\n', '\r']);
    if id.is_empty() {
        return Err(err(
            "DEP-POINTER",
            format!("{POINTER_NAME} is empty (torn write or truncation)"),
        ));
    }
    validate_build_id(id).map_err(|_| {
        err(
            "DEP-POINTER",
            format!("{POINTER_NAME} is corrupt: `{id}` is not a build id"),
        )
    })?;
    Ok(id.to_string())
}

/// Walk `dist` and hash every regular file: rel path → blake3 digest +
/// size. Deterministic (`BTreeMap` order, the D6 hashing rule); symlinks
/// are skipped (the bundler does not emit them — the same rule the static
/// exporter's recursive copy applies).
///
/// # Errors
///
/// [`DEP-ROOT`] when `dist` is not a directory; [`DEP-IO`] when a file
/// cannot be read.
pub fn scan_dist(dist: &Path) -> Result<BTreeMap<String, FileEntry>, OxideError> {
    if !dist.is_dir() {
        return Err(err(
            "DEP-ROOT",
            format!("dist root {} is not a directory", dist.display()),
        ));
    }
    let mut out = BTreeMap::new();
    scan_into(dist, dist, &mut out)?;
    Ok(out)
}

fn scan_into(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, FileEntry>,
) -> Result<(), OxideError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| err("DEP-IO", format!("cannot read {}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| err("DEP-IO", format!("read_dir entry: {e}")))?;
        let file_type = entry.file_type().map_err(|e| {
            err(
                "DEP-IO",
                format!("file_type for {}: {e}", entry.path().display()),
            )
        })?;
        let path = entry.path();
        if file_type.is_dir() {
            scan_into(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = rel_key(&path, root)?;
            let bytes = std::fs::read(&path)
                .map_err(|e| err("DEP-IO", format!("cannot read {}: {e}", path.display())))?;
            let hash = blake3::hash(&bytes).to_hex().to_string();
            out.insert(
                rel,
                FileEntry {
                    hash,
                    size: bytes.len() as u64,
                },
            );
        }
        // Symlinks and other types: skip (the bundler doesn't emit them).
    }
    Ok(())
}

/// The store key of `path` relative to `root`, with `/` separators.
fn rel_key(path: &Path, root: &Path) -> Result<String, OxideError> {
    let rel = path
        .strip_prefix(root)
        .map_err(|e| err("DEP-IO", format!("relativize {}: {e}", path.display())))?;
    let mut s = String::new();
    for component in rel.components() {
        if !s.is_empty() {
            s.push('/');
        }
        s.push_str(&component.as_os_str().to_string_lossy());
    }
    Ok(s)
}

/// The §V2-F11 plan function: diff the local scan against the store's
/// ledger. Pure — identical inputs produce identical plans (the pins in
/// `tests/delta_tdd.rs` hold the table).
#[must_use]
pub fn plan(local: &BTreeMap<String, FileEntry>, remote: &BTreeMap<String, String>) -> Plan {
    let (mut upload, mut keep, mut bytes_up) = (Vec::new(), Vec::new(), 0u64);
    for (rel, entry) in local {
        if remote.get(rel) == Some(&entry.hash) {
            keep.push(rel.clone());
        } else {
            upload.push(rel.clone());
            bytes_up += entry.size;
        }
    }
    let delete_later = remote
        .keys()
        .filter(|rel| !local.contains_key(*rel))
        .cloned()
        .collect();
    Plan {
        upload,
        keep,
        delete_later,
        bytes_up,
    }
}

/// The deploy knobs: which build is landing and when (epoch millis, the
/// injected-clock convention — tests pass fixed values, the CLI passes
/// the wall clock).
#[derive(Debug, Clone)]
pub struct DeployOptions {
    /// The build id landing (A04 shape: non-empty lowercase hex).
    pub build_id: String,
    /// The deploy instant, epoch millis.
    pub now_ms: u64,
}

/// The prune knobs: when the prune runs and how long superseded files
/// must stay (the skew window; `deploy.skewWindow`, default
/// [`DEFAULT_SKEW_WINDOW`]).
#[derive(Debug, Clone)]
pub struct PruneOptions {
    /// The prune instant, epoch millis.
    pub now_ms: u64,
    /// The skew window: marks younger than this survive.
    pub window: Duration,
}

/// What [`prune`] did: the marks it expired (deleted) and the ones still
/// inside their window (or protecting the pointed release).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneReport {
    /// The store paths deleted by this prune.
    pub deleted: Vec<String>,
    /// The store paths still pending (young marks, or under the release
    /// the pointer names — the rollback-safety rule).
    pub kept: Vec<String>,
}

/// Execute a delta deploy of `dist` into `store` (§V2-F11's order of
/// operations). Returns the plan it executed.
///
/// Re-deploying the SAME build id with the pointer already naming it is a
/// no-op (the idempotent re-run: nothing uploads, the ledger is not
/// rewritten, the returned plan is the empty delta).
///
/// # Errors
///
/// [`DEP-BUILDID`] for a build id outside the A04 shape, [`DEP-ROOT`]
/// when `dist` is not a directory, [`DEP-MANIFEST`] / [`DEP-POINTER`]
/// when the store's ledger or pointer is corrupt, [`DEP-IO`] for every
/// other store or scan failure. The pointer swap happens last: any error
/// before it leaves `current` naming the previous build.
pub fn deploy_dir(
    dist: &Path,
    store: &impl DeployStore,
    opts: &DeployOptions,
) -> Result<Plan, OxideError> {
    validate_build_id(&opts.build_id)?;
    if !dist.is_dir() {
        return Err(err(
            "DEP-ROOT",
            format!("dist root {} is not a directory", dist.display()),
        ));
    }
    let local = scan_dist(dist)?;
    let prior = store.load_manifest()?;
    let pointer_now = store.read_pointer()?;
    let prior_files: BTreeMap<String, String> = prior.as_ref().map_or_default(|m| m.files.clone());
    let plan = plan(&local, &prior_files);

    // The idempotent re-run: the ledger already describes this build and
    // production already serves it — nothing to do. (A pointer that
    // DISAGREES with the ledger — a rollback happened — takes the full
    // path: the release prefix is re-uploaded from current content.)
    let idempotent = prior.as_ref().is_some_and(|m| {
        m.build_id == opts.build_id && pointer_now.as_deref() == Some(opts.build_id.as_str())
    });
    if idempotent {
        tracing::debug!(build_id = %opts.build_id, "delta deploy: idempotent re-run, nothing to upload");
        return Ok(plan);
    }

    // 1. Changed immutable files at their rel path (content-addressed:
    //    only genuinely new bytes upload).
    for rel in &plan.upload {
        if is_immutable(rel) {
            let bytes = read_dist_file(dist, rel)?;
            store.put(rel, &bytes)?;
        }
    }
    // 2. Every non-hashed file mirrors into the fresh release prefix —
    //    changed or not, so the release is self-contained for the tabs
    //    that read the new pointer.
    for rel in local.keys() {
        if !is_immutable(rel) {
            let bytes = read_dist_file(dist, rel)?;
            let path = format!("{RELEASE_ROOT}/{}/{rel}", opts.build_id);
            store.put(&path, &bytes)?;
        }
    }
    tracing::info!(
        build_id = %opts.build_id,
        upload = plan.upload.len(),
        keep = plan.keep.len(),
        delete_later = plan.delete_later.len(),
        bytes_up = plan.bytes_up,
        "delta deploy: plan executed, swapping the pointer"
    );

    // 3. The ledger catches up BEFORE the pointer swap.
    let mut manifest = prior.unwrap_or_else(|| RemoteManifest {
        version: MANIFEST_VERSION,
        build_id: opts.build_id.clone(),
        previous_build_id: None,
        files: BTreeMap::new(),
        pending_delete: Vec::new(),
    });
    // The OUTGOING release's non-hashed copies are superseded by this
    // deploy: they must survive the skew window (old tabs read them), then
    // go. The earliest mark wins — a re-mark never extends the window.
    let outgoing = manifest.build_id.clone();
    let mut pending: BTreeMap<String, u64> = manifest
        .pending_delete
        .iter()
        .map(|pd| (pd.path.clone(), pd.marked_at_ms))
        .collect();
    if outgoing != opts.build_id {
        for rel in prior_files.keys() {
            if !is_immutable(rel) {
                let path = format!("{RELEASE_ROOT}/{outgoing}/{rel}");
                pending
                    .entry(path)
                    .and_modify(|t| {
                        if *t > opts.now_ms {
                            *t = opts.now_ms;
                        }
                    })
                    .or_insert(opts.now_ms);
            }
        }
    }
    // Removed immutable files leave their content-addressed object behind:
    // pending at their rel path, deleted after the window.
    for rel in &plan.delete_later {
        if is_immutable(rel) {
            pending
                .entry(rel.clone())
                .and_modify(|t| {
                    if *t > opts.now_ms {
                        *t = opts.now_ms;
                    }
                })
                .or_insert(opts.now_ms);
        }
    }
    manifest.pending_delete = pending
        .into_iter()
        .map(|(path, marked_at_ms)| PendingDelete { path, marked_at_ms })
        .collect();
    manifest.files = local
        .into_iter()
        .map(|(rel, entry)| (rel, entry.hash))
        .collect();
    manifest.previous_build_id = pointer_now;
    manifest.build_id.clone_from(&opts.build_id);
    store.store_manifest(&manifest)?;

    // 4. THE SWITCH: one small write, last.
    store.put_pointer(&opts.build_id)?;
    Ok(plan)
}

fn read_dist_file(dist: &Path, rel: &str) -> Result<Vec<u8>, OxideError> {
    std::fs::read(dist.join(rel)).map_err(|e| {
        err(
            "DEP-IO",
            format!("cannot read {}: {e}", dist.join(rel).display()),
        )
    })
}

/// Delete the pending marks that have served their skew window
/// (§V2-F11 step 4 of the order of operations: the second invocation).
///
/// Rollback safety: a mark under the release prefix the pointer currently
/// names is never deleted, however old — after a rollback, the
/// rolled-back-to release must stay alive. Marks younger than the window
/// stay pending (they keep their EARLIEST timestamp, so re-running prune
/// cannot extend a file's life).
///
/// # Errors
///
/// [`DEP-MANIFEST`] / [`DEP-POINTER`] when the store state is corrupt;
/// [`DEP-IO`] when a delete fails. A fresh store prunes to an empty
/// report.
pub fn prune(store: &impl DeployStore, opts: &PruneOptions) -> Result<PruneReport, OxideError> {
    let Some(mut manifest) = store.load_manifest()? else {
        return Ok(PruneReport::default());
    };
    let window_ms = u64::try_from(opts.window.as_millis()).unwrap_or(u64::MAX);
    // The release the pointer names is live in production — its copies
    // never delete, even past the window (the rollback-safety rule).
    let live_prefix = store
        .read_pointer()?
        .map(|id| format!("{RELEASE_ROOT}/{id}/"));
    let mut deleted = Vec::new();
    let mut kept = Vec::new();
    for pd in &manifest.pending_delete {
        let expired = pd.marked_at_ms.saturating_add(window_ms) <= opts.now_ms;
        let protects_live = live_prefix
            .as_ref()
            .is_some_and(|prefix| pd.path.starts_with(prefix));
        if expired && !protects_live {
            store.delete(&pd.path)?;
            deleted.push(pd.path.clone());
        } else {
            kept.push(pd.path.clone());
        }
    }
    manifest.pending_delete.retain(|pd| kept.contains(&pd.path));
    store.store_manifest(&manifest)?;
    tracing::info!(
        deleted = deleted.len(),
        kept = kept.len(),
        "deploy prune: expired marks removed"
    );
    Ok(PruneReport { deleted, kept })
}

/// Instant rollback (§V2-F11): rewrite `current` to the id it named
/// before the last deploy. Returns the id now current. The ledger's
/// `files` map is NOT rolled back — it stays the upload ledger (what the
/// store holds), which is exactly what the next delta deploy must diff
/// against.
///
/// # Errors
///
/// [`DEP-ROLLBACK`] when no manifest or no previous id is recorded;
/// [`DEP-MANIFEST`] / [`DEP-POINTER`] when the store state is corrupt;
/// [`DEP-IO`] when the swap fails.
pub fn rollback(store: &impl DeployStore) -> Result<String, OxideError> {
    let Some(mut manifest) = store.load_manifest()? else {
        return Err(err(
            "DEP-ROLLBACK",
            "cannot roll back: no deploy manifest exists in the store yet",
        ));
    };
    let Some(target) = manifest.previous_build_id.clone() else {
        return Err(err(
            "DEP-ROLLBACK",
            "cannot roll back: no previous build id is recorded",
        ));
    };
    let current = store.read_pointer()?;
    if current.as_deref() != Some(target.as_str()) {
        store.put_pointer(&target)?;
    }
    manifest.previous_build_id = current;
    store.store_manifest(&manifest)?;
    tracing::info!(build_id = %target, "deploy rollback: pointer swapped back");
    Ok(target)
}

/// A build id must be non-empty lowercase hex (the A04 shape: `build_id_of_dist`
/// digests truncate to 16 hex chars). The shared validator behind both
/// store targets' pointer writes.
pub(crate) fn validate_build_id(id: &str) -> Result<(), OxideError> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if ok {
        Ok(())
    } else {
        Err(err(
            "DEP-BUILDID",
            format!("the deploy build id `{id}` must be non-empty lowercase hex (the A04 shape)"),
        ))
    }
}

/// A store key must be a relative slash-separated path with no `.`/`..`
/// segments — the traversal guard behind [`DirStore`]'s writes and
/// deletes and the object-store target's keys.
pub(crate) fn validate_object_path(path: &str) -> Result<(), OxideError> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..");
    if bad {
        Err(err(
            "DEP-IO",
            format!(
                "invalid deploy object path `{path}`: store keys are relative slash-separated paths without `.` or `..` segments"
            ),
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_prefixes_classify_the_bundler_trees() {
        assert!(is_immutable("assets/app.abc123.js"));
        assert!(is_immutable("_oxide/chunks/x.js"));
        assert!(!is_immutable("index.html"));
        assert!(!is_immutable("manifest.json"));
        assert!(!is_immutable("about.html"));
    }

    #[test]
    fn build_id_validation_accepts_the_a04_shape_only() {
        assert!(validate_build_id("0123456789abcdef").is_ok());
        assert!(validate_build_id("").is_err());
        assert!(validate_build_id("ABCDEF").is_err());
        assert!(validate_build_id("not-hex!").is_err());
    }

    #[test]
    fn plan_of_identical_maps_is_all_keep() {
        let local: BTreeMap<String, FileEntry> = BTreeMap::from([(
            "index.html".to_string(),
            FileEntry {
                hash: "h1".to_string(),
                size: 5,
            },
        )]);
        let remote: BTreeMap<String, String> =
            BTreeMap::from([("index.html".to_string(), "h1".to_string())]);
        let p = plan(&local, &remote);
        assert!(p.upload.is_empty(), "identical hashes keep every file");
        assert_eq!(p.keep, vec!["index.html"]);
        assert_eq!(p.bytes_up, 0);
    }
}
