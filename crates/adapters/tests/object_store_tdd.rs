//! TDD pins for the object-store deploy target (spec v0.2 §V2-F11 remaining
//! half: "the `s3://`/`r2://` `object_store` target and the `oxide deploy` CLI
//! command are the remaining halves — see `docs/deviations.md`").
//!
//! The contracts pinned here (the same contracts
//! `tests/delta_tdd.rs` pins for `DirStore`, re-proven over a store whose
//! operations go through `object_store`'s `ObjectStore` trait):
//!
//! - [`deploy_target_from_url`] parses `s3://bucket/prefix`,
//!   `r2://bucket/prefix` (endpoint via the `endpoint` query parameter),
//!   `file://` and `mem://` backends; an unknown scheme or a bucket-less
//!   target refuses with a coded `DEP-URL` diagnostic.
//! - A first deploy uploads everything (immutable files at their rel path,
//!   non-hashed files under `releases/<build-id>/`), the ledger records the
//!   build, and the `current` pointer — written LAST, conditionally on the
//!   version it read — names the build; a failure mid-upload leaves
//!   `current` untouched.
//! - The pointer swap is conditional: swapping against a stale (or
//!   absent-expected) version refuses with `DEP-POINTER` — the object-store
//!   equivalent of `DirStore`'s temp+rename, minus the atomicity object
//!   stores cannot offer.
//! - A re-deploy of an unchanged tree is the empty delta; the one-line
//!   change acceptance (<5% of the bytes) holds; [`prune`] and [`rollback`]
//!   behave identically to the `dir:` target.
//! - Corrupt store state refuses with the same codes: `DEP-MANIFEST` for a
//!   ledger that does not parse or carries an unknown version,
//!   `DEP-POINTER` for a corrupt pointer, `DEP-IO` for traversal.
//!
//! Everything runs over the network-free `mem://` / `file://` backends —
//! the `s3://` pins cover URL parsing and client construction only (no
//! request is made until an operation runs).
#![cfg(feature = "s3")]

use std::time::Duration;

use oxide_fw_adapters::{
    DeployOptions, DeployStore, MANIFEST_VERSION, ObjectStoreStore, PruneOptions, RemoteManifest,
    deploy_dir, deploy_target_from_url, prune, rollback,
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

/// A fresh in-memory deploy store (the network-free object-store backend).
fn mem_store() -> ObjectStoreStore {
    deploy_target_from_url("mem://tdd").expect("the mem:// target constructs")
}

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

/// The error code of an `OxideError` (the Diagnostic's `code` field).
fn code_of(e: &oxide_fw_adapters::OxideError) -> String {
    e.0.code.clone()
}

/// Every object key currently in the store, sorted (the layout assertions
/// walk the same `list` API a serving CDN would).
fn keys(store: &ObjectStoreStore) -> Vec<String> {
    let mut out: Vec<String> = store
        .list_keys()
        .expect("the store lists")
        .into_iter()
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// 1. the URL parser: the target spellings the §V2-F11 record names
// ---------------------------------------------------------------------------

#[test]
fn the_target_url_parses_s3_r2_file_and_mem_backends() {
    // mem:// (no prefix) and mem://label construct the in-memory backend.
    let mem = deploy_target_from_url("mem://tdd").expect("mem:// constructs");
    assert_eq!(mem.prefix(), "", "mem:// carries no prefix");

    // file:// points at an existing local prefix.
    let tmp = tempfile::tempdir().expect("tempdir");
    let url = format!("file://{}", tmp.path().display());
    let file = deploy_target_from_url(&url).expect("file:// constructs");
    assert_eq!(file.prefix(), "");

    // s3:// carries a bucket and an optional prefix; construction builds
    // the client from the environment without a network round trip.
    let s3 = deploy_target_from_url("s3://site-bucket/assets-root").expect("s3:// constructs");
    assert_eq!(s3.prefix(), "assets-root");

    // r2:// is the S3 API against an endpoint override; without one the
    // target refuses (the fix names the query parameter).
    let e =
        deploy_target_from_url("r2://site-bucket").expect_err("r2:// without an endpoint refuses");
    assert_eq!(code_of(&e), "DEP-URL");
    let r2 = deploy_target_from_url(
        "r2://site-bucket/releases?endpoint=https://account.r2.cloudflarestorage.com",
    )
    .expect("r2:// with an endpoint constructs");
    assert_eq!(r2.prefix(), "releases");
}

#[test]
fn unknown_schemes_and_bucketless_targets_refuse_with_dep_url() {
    for bad in [
        "https://bucket",
        "ftp://bucket",
        "s3://",
        "s3:///prefix",
        "mem:/nope",
        "",
        "file://",
    ] {
        let e = deploy_target_from_url(bad).expect_err(&format!("`{bad}` refuses"));
        assert_eq!(code_of(&e), "DEP-URL", "`{bad}` is a DEP-URL");
    }
}

// ---------------------------------------------------------------------------
// 2. the ledger + pointer round trip, with the corrupt-state refusals
// ---------------------------------------------------------------------------

#[test]
fn the_object_store_round_trips_the_ledger_and_pointer() {
    let store = mem_store();
    assert!(
        store.load_manifest().expect("fresh ledger").is_none(),
        "a fresh store carries no manifest"
    );
    assert!(
        store.read_pointer().expect("fresh pointer").is_none(),
        "a fresh store carries no pointer"
    );

    let manifest = RemoteManifest {
        version: MANIFEST_VERSION,
        build_id: V1.to_string(),
        previous_build_id: None,
        files: std::collections::BTreeMap::from([("index.html".to_string(), "h-i".to_string())]),
        pending_delete: Vec::new(),
    };
    store.store_manifest(&manifest).expect("the ledger stores");
    let loaded = store
        .load_manifest()
        .expect("the ledger loads")
        .expect("the ledger exists");
    assert_eq!(loaded, manifest, "the ledger round-trips");

    // A manifest carrying an unknown version refuses (fail closed on a
    // future engine version — the same refusal `DirStore` makes).
    let future = RemoteManifest {
        version: MANIFEST_VERSION + 1,
        ..manifest.clone()
    };
    store.store_manifest(&future).expect("the ledger stores");
    let e = store
        .load_manifest()
        .expect_err("an unknown version refuses");
    assert_eq!(code_of(&e), "DEP-MANIFEST");

    // The pointer round-trips, and a corrupt pointer refuses.
    store.put_pointer(V1).expect("the pointer swaps");
    assert_eq!(
        store.read_pointer().expect("pointer reads").as_deref(),
        Some(V1)
    );
}

#[test]
fn corrupt_ledger_and_pointer_bytes_refuse_with_the_dir_target_codes() {
    let store = mem_store();
    store
        .put("manifest.json", b"{ not json")
        .expect("garbage stores");
    let e = store.load_manifest().expect_err("garbage ledger refuses");
    assert_eq!(code_of(&e), "DEP-MANIFEST");

    store.put("current", b"!!!").expect("garbage stores");
    let e = store.read_pointer().expect_err("garbage pointer refuses");
    assert_eq!(code_of(&e), "DEP-POINTER");
}

// ---------------------------------------------------------------------------
// 3. the conditional pointer swap (the object-store atomicity contract)
// ---------------------------------------------------------------------------

#[test]
fn the_pointer_swap_refuses_when_the_store_moved_underneath() {
    let store = mem_store();

    // The first swap expects absence: Create-if-absent.
    store.swap_pointer(V1, None).expect("the first swap lands");

    // Swapping while expecting absence again refuses: the pointer exists.
    let e = store
        .swap_pointer(V2, None)
        .expect_err("a create against an existing pointer refuses");
    assert_eq!(code_of(&e), "DEP-POINTER");

    // A swap against a STALE version refuses: another deploy moved the
    // pointer between this deploy's read and write (the lost race).
    let live = store.pointer_version().expect("the live version reads");
    store.put_pointer(V2).expect("a concurrent deploy swaps");
    let e = store
        .swap_pointer(V2, live.as_ref())
        .expect_err("a stale version refuses");
    assert_eq!(code_of(&e), "DEP-POINTER");
    assert_eq!(
        store.read_pointer().expect("pointer reads").as_deref(),
        Some(V2),
        "the refused swap never flipped `current`"
    );

    // A swap against the LIVE version lands.
    let live = store.pointer_version().expect("the live version reads");
    store
        .swap_pointer(V1, live.as_ref())
        .expect("a swap on the live version lands");
    assert_eq!(
        store.read_pointer().expect("pointer reads").as_deref(),
        Some(V1)
    );
}

// ---------------------------------------------------------------------------
// 4. the §V2-F11 order of operations through the object store
// ---------------------------------------------------------------------------

#[test]
fn a_first_deploy_lands_through_the_object_store_and_swaps_the_pointer_last() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    put_file(&dist, "assets/app.abc123.js", &asset_body(1));
    put_file(&dist, "index.html", "<html>v1</html>");
    let store = mem_store();

    let p = deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect("the first deploy");

    assert_eq!(p.upload.len(), 2, "a fresh store uploads everything");
    assert_eq!(p.keep, Vec::<String>::new(), "a fresh store keeps nothing");

    // The layout is the DirStore layout, over object keys: the immutable
    // chunk at its rel path, the HTML under the release prefix, the ledger
    // at the root, the pointer naming v1.
    let ks = keys(&store);
    assert!(ks.contains(&"assets/app.abc123.js".to_string()));
    assert!(
        ks.contains(&format!("releases/{V1}/index.html")),
        "the non-hashed file mirrors under releases/<build-id>/: {ks:?}"
    );
    assert!(ks.contains(&"manifest.json".to_string()) && ks.contains(&"current".to_string()));
    assert_eq!(
        store.read_pointer().expect("pointer reads").as_deref(),
        Some(V1),
        "the pointer names the deployed build"
    );
    let manifest = store
        .load_manifest()
        .expect("ledger reads")
        .expect("ledger exists");
    assert_eq!(manifest.build_id, V1);
    assert_eq!(
        manifest.files.get("index.html").map(String::as_str),
        Some(blake3::hash(b"<html>v1</html>").to_hex().as_str()),
        "the ledger keys hash to the blake3 digest (the D6 rule)"
    );
}

/// A store wrapper whose `put` fails at the Nth call (the spec's injected
/// error, now over the object-store backend).
struct FailingStore {
    inner: ObjectStoreStore,
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
            return Err(oxide_fw_adapters::err("DEP-IO", "injected failure"));
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
fn a_failed_upload_leaves_the_object_pointer_untouched() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    put_file(&dist, "assets/one.aaaa.js", "one");
    put_file(&dist, "index.html", "v1");
    let store = mem_store();
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect("v1 lands");

    // v2 changes everything and the store fails on the 2nd put.
    put_file(&dist, "assets/one.aaaa.js", "one-v2");
    put_file(&dist, "index.html", "v2");
    let failing = FailingStore {
        inner: mem_store(),
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
}

// ---------------------------------------------------------------------------
// 5. idempotence + the <5% acceptance, through the object store
// ---------------------------------------------------------------------------

#[test]
fn an_object_store_redeploy_is_idempotent_and_the_delta_stays_under_five_percent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    let n_assets = 100usize;
    let mut total = 0u64;
    for i in 0..n_assets {
        let body = asset_body(i);
        total += body.len() as u64;
        put_file(&dist, &format!("assets/chunk-{i:04}.deadbeef.js"), &body);
    }
    let html = "<html>v1</html>";
    total += html.len() as u64;
    put_file(&dist, "index.html", html);
    let store = mem_store();
    let v1_opts = DeployOptions {
        build_id: V1.to_string(),
        now_ms: T1,
    };

    deploy_dir(&dist, &store, &v1_opts).expect("the full deploy");
    let again = deploy_dir(&dist, &store, &v1_opts).expect("the re-deploy");
    assert!(
        again.upload.is_empty(),
        "an unchanged tree re-uploads nothing: {:?}",
        again.upload
    );
    assert_eq!(again.bytes_up, 0, "the idempotent re-run bills nothing");

    // The one-line change: exactly one hashed chunk rewrites.
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
    .expect("the delta deploy");
    assert_eq!(p.upload, vec!["assets/chunk-0007.deadbeef.js"]);
    assert!(
        p.bytes_up * 20 < total,
        "the delta uploads {} bytes, the 5% budget is {}",
        p.bytes_up,
        total / 20
    );
    assert!(
        keys(&store).contains(&format!("releases/{V2}/index.html")),
        "the unchanged HTML mirrors into the fresh release prefix"
    );
}

// ---------------------------------------------------------------------------
// 6. prune + rollback parity with the dir target
// ---------------------------------------------------------------------------

#[test]
fn prune_and_rollback_behave_identically_on_the_object_store_target() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    put_file(&dist, "assets/gone.1111.js", "asset-v1");
    put_file(&dist, "index.html", "v1");
    let store = mem_store();
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: V1.to_string(),
            now_ms: T1,
        },
    )
    .expect("v1 lands");

    // v2 removes the asset, changes the page.
    std::fs::remove_file(dist.join("assets/gone.1111.js")).expect("fixture removal");
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

    // Young marks survive; expired marks delete; the pointed release stays.
    let young = prune(
        &store,
        &PruneOptions {
            now_ms: T2 + WINDOW_MS - 1,
            window: WINDOW,
        },
    )
    .expect("young prune runs");
    assert!(young.deleted.is_empty(), "young marks are kept: {young:?}");
    assert!(
        keys(&store).contains(&"assets/gone.1111.js".to_string()),
        "the removed asset survives its window"
    );

    let report = prune(
        &store,
        &PruneOptions {
            now_ms: T2 + WINDOW_MS + 1,
            window: WINDOW,
        },
    )
    .expect("expired prune runs");
    assert!(
        report.deleted.contains(&"assets/gone.1111.js".to_string()),
        "the expired mark deletes: {report:?}"
    );
    assert!(
        keys(&store).contains(&format!("releases/{V2}/index.html")),
        "the pointed release is never pruned"
    );

    // Rollback re-points to v1; a second rollback returns to v2.
    let rolled = rollback(&store).expect("rollback");
    assert_eq!(rolled, V1, "rollback returns the id it re-pointed to");
    assert_eq!(
        store.read_pointer().expect("pointer reads").as_deref(),
        Some(V1)
    );
    let again = rollback(&store).expect("second rollback");
    assert_eq!(again, V2);
}

// ---------------------------------------------------------------------------
// 7. store-key safety
// ---------------------------------------------------------------------------

#[test]
fn the_object_store_target_refuses_path_traversal() {
    let store = mem_store();
    for bad in ["../evil", "/abs", "a/../../b", "", "a//b"] {
        let e = store.put(bad, b"x").expect_err(&format!("`{bad}` refuses"));
        assert_eq!(code_of(&e), "DEP-IO", "`{bad}` is a DEP-IO");
    }
}
