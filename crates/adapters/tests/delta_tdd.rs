//! TDD pins for the delta-deploy engine (spec v0.2 §V2-F11: "`oxide deploy`
//! uploads only what changed and swaps atomically").
//!
//! The contracts pinned here:
//!
//! - [`plan`] is pure and spec-verbatim: `upload` / `keep` / `delete_later`
//!   / `bytes_up` over a `(hash, size)` local map and a hash-only remote
//!   map, in sorted rel-path order.
//! - A first deploy uploads everything (immutable files at their rel path,
//!   non-hashed files under `releases/<build-id>/`) and the `current`
//!   pointer — written LAST — names the build; a failure mid-upload leaves
//!   `current` untouched (the fake store with an injected error).
//! - A re-deploy of an unchanged tree uploads nothing and leaves the
//!   manifest byte-identical (idempotence); a one-line change uploads
//!   under 5% of the site's bytes (the §V2-F11 acceptance).
//! - [`prune`] deletes only pending marks older than the skew window
//!   (rollback-safe: the release the pointer names is never pruned),
//!   [`rollback`] rewrites `current` to the recorded previous build id.
//! - The store contract refuses path traversal, refuses a non-hex build
//!   id, and surfaces a corrupt manifest/pointer as coded `DEP-*`
//!   diagnostics (the crate's `OxideError` convention).

use std::collections::BTreeMap;
use std::time::Duration;

use oxide_fw_adapters::{
    DeployOptions, DeployStore, DirStore, FileEntry, PruneOptions, deploy_dir, err, plan, prune,
    rollback, scan_dist,
};

/// A build id in the A04 shape (lowercase hex).
const V1: &str = "1111111111111111";
const V2: &str = "2222222222222222";

/// Epoch millis the fixtures deploy at (deterministic, no clock reads).
const T1: u64 = 1_000;
const T2: u64 = 2_000;
/// The F10 default skew window, spelled in the same unit the options take.
const WINDOW: Duration = Duration::from_mins(15);
const WINDOW_MS: u64 = 15 * 60 * 1000;

/// Write one file of `content` under `dist/rel` (creating parents).
fn put_file(dist: &std::path::Path, rel: &str, content: &str) {
    let path = dist.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent creates");
    }
    std::fs::write(path, content).expect("fixture write");
}

/// A one-kibibyte asset body (the acceptance fixture's bulk).
fn asset_body(n: usize) -> String {
    format!("asset {n} ").repeat(32)
}

/// The error code of an [`OxideError`] (the Diagnostic's `code` field).
fn code_of(e: &oxide_fw_adapters::OxideError) -> String {
    e.0.code.clone()
}

// ---------------------------------------------------------------------------
// 1. the pure plan table (new / changed / unchanged / removed)
// ---------------------------------------------------------------------------

#[test]
fn plan_classifies_new_changed_unchanged_removed() {
    let entry = |h: &str, sz: u64| FileEntry {
        hash: h.to_string(),
        size: sz,
    };
    let local: BTreeMap<String, FileEntry> = BTreeMap::from([
        ("assets/unchanged.js".to_string(), entry("h-a", 10)),
        ("assets/changed.js".to_string(), entry("h-b2", 20)),
        ("assets/new.js".to_string(), entry("h-c", 30)),
        ("index.html".to_string(), entry("h-i", 5)),
    ]);
    let remote: BTreeMap<String, String> = BTreeMap::from([
        ("assets/unchanged.js".to_string(), "h-a".to_string()),
        ("assets/changed.js".to_string(), "h-b1".to_string()), // hash changed
        ("assets/gone.js".to_string(), "h-d".to_string()),     // removed locally
        ("old.html".to_string(), "h-e".to_string()),           // removed locally
    ]);
    let p = plan(&local, &remote);
    // Sorted rel-path order, deterministic.
    assert_eq!(
        p.upload,
        vec!["assets/changed.js", "assets/new.js", "index.html"]
    );
    assert_eq!(p.keep, vec!["assets/unchanged.js"]);
    assert_eq!(p.delete_later, vec!["assets/gone.js", "old.html"]);
    assert_eq!(p.bytes_up, 20 + 30 + 5);
}

// ---------------------------------------------------------------------------
// 2. the store layout + pointer swap
// ---------------------------------------------------------------------------

#[test]
fn first_deploy_uploads_everything_and_swaps_the_pointer_last() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    put_file(&dist, "assets/app.abc123.js", &asset_body(1));
    put_file(&dist, "index.html", "<html>v1</html>");
    let store = DirStore::new(tmp.path().join("store"));

    let p = deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect("the first deploy");

    // Every local file is an upload on a fresh store.
    assert_eq!(p.upload.len(), 2);
    assert!(p.keep.is_empty(), "a fresh store keeps nothing");
    assert_eq!(
        p.bytes_up,
        asset_body(1).len() as u64 + "<html>v1</html>".len() as u64
    );

    // Immutable files land at their rel path; non-hashed under the release
    // prefix; the pointer names the build.
    let store_root = tmp.path().join("store");
    assert!(
        store_root.join("assets/app.abc123.js").is_file(),
        "immutable asset lives at its rel path"
    );
    assert!(
        store_root
            .join("releases")
            .join(V1)
            .join("index.html")
            .is_file(),
        "non-hashed file lives under releases/<build-id>/"
    );
    assert_eq!(
        store.read_pointer().expect("pointer reads").as_deref(),
        Some(V1),
        "the pointer names the deployed build"
    );
}

#[test]
fn non_hex_build_id_refused_and_missing_dist_root_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = DirStore::new(tmp.path().join("store"));

    let e = deploy_dir(
        &tmp.path().join("nowhere"),
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect_err("a missing dist root refuses");
    assert_eq!(code_of(&e), "DEP-ROOT");

    put_file(&tmp.path().join("dist"), "index.html", "<html></html>");
    let e = deploy_dir(
        &tmp.path().join("dist"),
        &store,
        &DeployOptions {
            build_id: "not-hex!".to_string(),
            now_ms: T1,
        },
    )
    .expect_err("a non-hex build id refuses");
    assert_eq!(code_of(&e), "DEP-BUILDID");
}

// ---------------------------------------------------------------------------
// 3. idempotence + the <5% acceptance
// ---------------------------------------------------------------------------

#[test]
fn re_deploy_of_unchanged_tree_uploads_nothing_and_is_byte_identical() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    put_file(&dist, "assets/app.abc123.js", &asset_body(1));
    put_file(&dist, "index.html", "<html>v1</html>");
    let store = DirStore::new(tmp.path().join("store"));
    let opts = DeployOptions {
        build_id: V1.to_string(),
        now_ms: T1,
    };

    deploy_dir(&dist, &store, &opts).expect("first deploy");
    let manifest_before = std::fs::read_to_string(tmp.path().join("store").join("manifest.json"))
        .expect("manifest reads");

    let p2 = deploy_dir(&dist, &store, &opts).expect("second deploy");
    assert!(
        p2.upload.is_empty(),
        "an unchanged tree re-uploads nothing: {:?}",
        p2.upload
    );
    assert_eq!(p2.keep.len(), 2);
    assert_eq!(p2.bytes_up, 0, "zero billed bytes on an idempotent re-run");

    let manifest_after = std::fs::read_to_string(tmp.path().join("store").join("manifest.json"))
        .expect("manifest reads");
    assert_eq!(
        manifest_before, manifest_after,
        "the idempotent re-run leaves the manifest byte-identical"
    );
}

#[test]
fn one_line_change_uploads_under_five_percent_of_bytes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    let n_assets = 400usize;
    let mut total = 0u64;
    for i in 0..n_assets {
        let body = asset_body(i); // 1 KiB
        total += body.len() as u64;
        put_file(&dist, &format!("assets/chunk-{i:04}.deadbeef.js"), &body);
    }
    let html = "<html>v1</html>";
    total += html.len() as u64;
    put_file(&dist, "index.html", html);
    let store = DirStore::new(tmp.path().join("store"));

    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect("full deploy");

    // The one-line change: exactly one hashed chunk is rewritten.
    let changed = asset_body(7) + "patched";
    put_file(&dist, "assets/chunk-0007.deadbeef.js", &changed);

    let p = deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V2.to_string(),
            now_ms: T2,
        },
    )
    .expect("delta deploy");

    // The plan is the hash delta: exactly one changed immutable chunk.
    // (The unchanged HTML is a `keep` in the plan — the EXECUTOR still
    // mirrors every non-hashed file into the new release prefix, pinned
    // below.)
    assert_eq!(p.upload, vec!["assets/chunk-0007.deadbeef.js"]);
    assert_eq!(p.keep.len(), n_assets - 1 + 1);
    // 5% of total, in exact integer arithmetic (bytes_up * 20 < total).
    assert!(
        p.bytes_up * 20 < total,
        "the delta uploads {} bytes, the 5% budget is {}",
        p.bytes_up,
        total / 20
    );
    // And the real number is tiny: one 1 KiB chunk.
    assert_eq!(p.bytes_up, changed.len() as u64);

    // The executor mirrored the unchanged HTML into the v2 release prefix
    // (a release is self-contained for the tabs reading the new pointer).
    assert!(
        tmp.path()
            .join("store")
            .join("releases")
            .join(V2)
            .join("index.html")
            .is_file(),
        "the unchanged HTML mirrors into the fresh release prefix"
    );
}

// ---------------------------------------------------------------------------
// 4. failure mid-upload leaves `current` untouched
// ---------------------------------------------------------------------------

/// A store wrapper whose `put` fails at the Nth call (the spec's "fake store
/// with an injected error").
struct FailingStore {
    inner: DirStore,
    fail_at_put: usize,
    puts: std::sync::Mutex<usize>,
}

impl DeployStore for FailingStore {
    fn load_manifest(
        &self,
    ) -> oxide_fw_adapters::Result<Option<oxide_fw_adapters::RemoteManifest>> {
        self.inner.load_manifest()
    }
    fn store_manifest(
        &self,
        m: &oxide_fw_adapters::RemoteManifest,
    ) -> oxide_fw_adapters::Result<()> {
        self.inner.store_manifest(m)
    }
    fn put(&self, path: &str, bytes: &[u8]) -> oxide_fw_adapters::Result<()> {
        let mut n = self.puts.lock().expect("the put counter never poisons");
        *n += 1;
        if *n == self.fail_at_put {
            return Err(err("DEP-IO", "injected failure"));
        }
        drop(n);
        self.inner.put(path, bytes)
    }
    fn delete(&self, path: &str) -> oxide_fw_adapters::Result<()> {
        self.inner.delete(path)
    }
    fn put_pointer(&self, build_id: &str) -> oxide_fw_adapters::Result<()> {
        self.inner.put_pointer(build_id)
    }
    fn read_pointer(&self) -> oxide_fw_adapters::Result<Option<String>> {
        self.inner.read_pointer()
    }
}

#[test]
fn a_failed_upload_leaves_current_untouched() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    put_file(&dist, "assets/one.aaaa.js", "one");
    put_file(&dist, "assets/two.bbbb.js", "two");
    put_file(&dist, "index.html", "v1");
    let store = DirStore::new(tmp.path().join("store"));
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect("v1 lands");

    // v2 changes everything (three uploads) and the store fails on the 2nd.
    put_file(&dist, "assets/one.aaaa.js", "one-v2");
    put_file(&dist, "assets/two.bbbb.js", "two-v2");
    put_file(&dist, "index.html", "v2");
    let failing = FailingStore {
        inner: DirStore::new(tmp.path().join("store")),
        fail_at_put: 2,
        puts: std::sync::Mutex::new(0),
    };
    let e = deploy_dir(
        &dist,
        &failing,
        &DeployOptions {
            build_id: V2.to_string(),
            now_ms: T2,
        },
    )
    .expect_err("the injected failure aborts the deploy");
    assert_eq!(code_of(&e), "DEP-IO");

    assert_eq!(
        store.read_pointer().expect("pointer reads").as_deref(),
        Some(V1),
        "the pointer still names v1: a failed deploy never flips `current`"
    );
    let manifest = store
        .load_manifest()
        .expect("manifest reads")
        .expect("manifest exists");
    assert_eq!(manifest.build_id, V1, "the manifest still records v1");
}

// ---------------------------------------------------------------------------
// 5. prune: the skew window
// ---------------------------------------------------------------------------

#[test]
fn prune_deletes_only_expired_marks_and_never_the_pointed_release() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    put_file(&dist, "assets/gone.1111.js", "asset-v1");
    put_file(&dist, "about.html", "about-v1");
    put_file(&dist, "index.html", "v1");
    let store = DirStore::new(tmp.path().join("store"));
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect("v1 lands");

    // v2 removes the asset and the about page, changes index.html.
    std::fs::remove_file(dist.join("assets/gone.1111.js")).expect("fixture removal");
    std::fs::remove_file(dist.join("about.html")).expect("fixture removal");
    put_file(&dist, "index.html", "v2");
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V2.to_string(),
            now_ms: T2,
        },
    )
    .expect("v2 lands");

    let store_root = tmp.path().join("store");
    let gone_asset = store_root.join("assets/gone.1111.js");
    let old_about = store_root.join("releases").join(V1).join("about.html");
    assert!(
        gone_asset.is_file() && old_about.is_file(),
        "v1 objects exist"
    );

    // Before the window expires nothing is deleted (the marks are young).
    let young = prune(
        &store,
        &PruneOptions {
            now_ms: T2 + WINDOW_MS - 1,
            window: WINDOW,
        },
    )
    .expect("young prune runs");
    assert!(young.deleted.is_empty(), "young marks are kept: {young:?}");
    assert!(gone_asset.is_file() && old_about.is_file());

    // After the window the expired marks are deleted…
    let report = prune(
        &store,
        &PruneOptions {
            now_ms: T2 + WINDOW_MS + 1,
            window: WINDOW,
        },
    )
    .expect("expired prune runs");
    assert_eq!(
        report.deleted.len(),
        3,
        "all three v1 marks expire: {report:?}"
    );
    assert!(!gone_asset.is_file(), "the removed asset is pruned");
    assert!(!old_about.is_file(), "the old release copy is pruned");
    assert!(
        store_root
            .join("releases")
            .join(V2)
            .join("index.html")
            .is_file(),
        "the pointed release is never pruned"
    );

    // …and a second prune is a no-op (the manifest no longer carries them).
    let again = prune(
        &store,
        &PruneOptions {
            now_ms: T2 + WINDOW_MS + 2,
            window: WINDOW,
        },
    )
    .expect("second prune runs");
    assert!(
        again.deleted.is_empty(),
        "the second prune finds nothing left"
    );
}

#[test]
fn rollback_then_prune_keeps_the_rolled_back_release_alive() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    put_file(&dist, "index.html", "v1");
    let store = DirStore::new(tmp.path().join("store"));
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect("v1 lands");
    put_file(&dist, "index.html", "v2");
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V2.to_string(),
            now_ms: T2,
        },
    )
    .expect("v2 lands");

    // Roll back to v1, then run a prune long after the window.
    rollback(&store).expect("rollback");
    let report = prune(
        &store,
        &PruneOptions {
            now_ms: T2 + WINDOW_MS + 1,
            window: WINDOW,
        },
    )
    .expect("prune after rollback");
    assert!(
        report
            .deleted
            .iter()
            .all(|p| !p.starts_with(&format!("releases/{V1}/"))),
        "the release the pointer names is never pruned: {report:?}"
    );
    assert!(
        tmp.path()
            .join("store")
            .join("releases")
            .join(V1)
            .join("index.html")
            .is_file(),
        "the rolled-back-to release still serves"
    );
}

// ---------------------------------------------------------------------------
// 6. rollback
// ---------------------------------------------------------------------------

#[test]
fn rollback_repoints_current_to_the_recorded_previous() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    put_file(&dist, "index.html", "v1");
    let store = DirStore::new(tmp.path().join("store"));
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect("v1 lands");
    put_file(&dist, "index.html", "v2");
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V2.to_string(),
            now_ms: T2,
        },
    )
    .expect("v2 lands");

    let now_current = rollback(&store).expect("rollback");
    assert_eq!(now_current, V1, "rollback returns the id it re-pointed to");
    assert_eq!(
        store.read_pointer().expect("pointer reads").as_deref(),
        Some(V1)
    );

    // A second rollback swaps back: the previous pointer was v2.
    let again = rollback(&store).expect("second rollback");
    assert_eq!(again, V2);
    assert_eq!(
        store.read_pointer().expect("pointer reads").as_deref(),
        Some(V2)
    );
}

#[test]
fn rollback_without_a_previous_build_refuses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = DirStore::new(tmp.path().join("store"));
    let e = rollback(&store).expect_err("no manifest, no previous");
    assert_eq!(code_of(&e), "DEP-ROLLBACK");

    // A first deploy records no previous: rollback still refuses.
    let dist = tmp.path().join("dist");
    put_file(&dist, "index.html", "v1");
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect("v1 lands");
    let e = rollback(&store).expect_err("a first deploy has no previous");
    assert_eq!(code_of(&e), "DEP-ROLLBACK");
}

// ---------------------------------------------------------------------------
// 7. coded diagnostics for corrupt store state
// ---------------------------------------------------------------------------

#[test]
fn a_corrupt_remote_manifest_refuses_with_a_dep_manifest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store_root = tmp.path().join("store");
    std::fs::create_dir_all(&store_root).expect("store mkdir");
    std::fs::write(store_root.join("manifest.json"), "{ not json").expect("garbage");
    let dist = tmp.path().join("dist");
    put_file(&dist, "index.html", "v1");
    let e = deploy_dir(
        &dist,
        &DirStore::new(&store_root),
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect_err("garbage manifest refuses");
    assert_eq!(code_of(&e), "DEP-MANIFEST");
}

#[test]
fn an_unknown_manifest_version_refuses_with_dep_manifest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store_root = tmp.path().join("store");
    std::fs::create_dir_all(&store_root).expect("store mkdir");
    std::fs::write(
        store_root.join("manifest.json"),
        r#"{"version": 99, "build_id": "aabb", "files": {}}"#,
    )
    .expect("future manifest");
    let dist = tmp.path().join("dist");
    put_file(&dist, "index.html", "v1");
    let e = deploy_dir(
        &dist,
        &DirStore::new(&store_root),
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect_err("an unknown version refuses");
    assert_eq!(code_of(&e), "DEP-MANIFEST");
}

#[test]
fn a_corrupt_pointer_refuses_with_dep_pointer() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store_root = tmp.path().join("store");
    std::fs::create_dir_all(&store_root).expect("store mkdir");
    std::fs::write(store_root.join("current"), "!!!").expect("garbage pointer");
    let e = DirStore::new(&store_root)
        .read_pointer()
        .expect_err("a garbage pointer refuses");
    assert_eq!(code_of(&e), "DEP-POINTER");
}

// ---------------------------------------------------------------------------
// 8. store-path safety + the scan contract
// ---------------------------------------------------------------------------

#[test]
fn the_store_refuses_path_traversal_and_absolute_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = DirStore::new(tmp.path().join("store"));
    for bad in ["../evil", "/abs", "a/../../b", "", "a//b"] {
        let e = store.put(bad, b"x").expect_err(&format!("`{bad}` refuses"));
        assert_eq!(code_of(&e), "DEP-IO", "`{bad}` is a DEP-IO");
    }
    assert!(
        !tmp.path().join("evil").exists(),
        "nothing was written outside the store root"
    );
}

#[test]
fn scan_dist_hashes_with_blake3_and_reports_sizes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    put_file(&dist, "assets/a.1111.js", "hello");
    put_file(&dist, "nested/deep/page.html", "page");
    let local = scan_dist(&dist).expect("the scan walks");
    assert_eq!(local.len(), 2, "every file, none of the directories");
    let a = &local["assets/a.1111.js"];
    assert_eq!(
        a.hash,
        blake3::hash(b"hello").to_hex().to_string(),
        "the hash is the blake3 content digest (the D6 hashing rule)"
    );
    assert_eq!(a.size, 5);
    let p = &local["nested/deep/page.html"];
    assert_eq!(p.hash, blake3::hash(b"page").to_hex().to_string());
    assert_eq!(p.size, 4);
}
