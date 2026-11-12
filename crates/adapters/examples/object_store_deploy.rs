//! The object-store deploy walkthrough (§V2-F11 remaining half): the same
//! delta-deploy story the `delta_deploy` example tells through `DirStore`,
//! now through the [`ObjectStoreStore`] seam — the target every `s3://`,
//! `r2://`, `file://` and `mem://` spelling lands through.
//!
//! The walkthrough runs against the network-free `mem://` backend. Pass a
//! real target URL as the first argument to walk the identical steps
//! against it (e.g. `s3://my-bucket/site`, or a `r2://` target with its
//! `endpoint=` query parameter) — credentials come from the standard
//! `AWS_*` environment, exactly like the cache crate's S3 handler.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-adapters --features s3 --example object_store_deploy
//! ```

use std::time::Duration;

use oxide_fw_adapters::{
    DEFAULT_SKEW_WINDOW, DeployOptions, DeployStore, ObjectStoreStore, PruneOptions, deploy_dir,
    prune, rollback,
};

/// Build a tiny `dist/` tree: hashed chunks under `assets/` (immutable),
/// HTML at the root (mirrored into the release prefix).
fn make_dist(dist: &std::path::Path) {
    std::fs::create_dir_all(dist.join("assets")).expect("dist mkdir");
    for i in 0..8 {
        std::fs::write(
            dist.join(format!("assets/chunk-{i}.deadbeef.js")),
            format!("console.log('chunk {i}');\n").repeat(64),
        )
        .expect("chunk write");
    }
    std::fs::write(dist.join("index.html"), "<html>v1</html>").expect("html write");
}

/// The first deploy: a fresh store uploads every object.
fn step1_full_deploy(dist: &std::path::Path, store: &ObjectStoreStore) {
    let first = deploy_dir(
        dist,
        store,
        &DeployOptions {
            build_id: "0123456789abcdef".to_string(),
            now_ms: 1_000,
        },
    )
    .expect("the first deploy");
    println!("deploy 1 (fresh object store)");
    println!(
        "  upload {} objects, {} bytes, keep {}",
        first.upload.len(),
        first.bytes_up,
        first.keep.len()
    );
    println!(
        "  pointer = {}",
        store.read_pointer().expect("pointer").unwrap_or_default()
    );
}

/// The one-line change: one hashed chunk flips — the delta is the
/// §V2-F11 acceptance case (<5% of the bytes).
fn step2_delta_deploy(dist: &std::path::Path, store: &ObjectStoreStore) {
    std::fs::write(
        dist.join("assets/chunk-3.deadbeef.js"),
        "console.log('chunk 3 patched');\n".repeat(64),
    )
    .expect("chunk rewrite");
    let second = deploy_dir(
        dist,
        store,
        &DeployOptions {
            build_id: "2222222222222222".to_string(),
            now_ms: 2_000,
        },
    )
    .expect("the delta deploy");
    println!("deploy 2 (one chunk changed)");
    println!(
        "  upload {:?}, {} bytes, keep {} objects",
        second.upload,
        second.bytes_up,
        second.keep.len()
    );
}

/// Rollback: `current` re-points to the previous build — one conditional
/// put against the version the pointer carries.
fn step3_rollback(store: &ObjectStoreStore) {
    let rolled = rollback(store).expect("rollback");
    println!("rollback -> current = {rolled}");
}

/// Prune past the window: the superseded v1 objects go; the release the
/// pointer names never deletes.
fn step4_prune(store: &ObjectStoreStore) {
    let window_ms =
        u64::try_from(DEFAULT_SKEW_WINDOW.as_millis()).expect("the window fits epoch millis");
    let report = prune(
        store,
        &PruneOptions {
            now_ms: 2_000 + window_ms + 1,
            window: Duration::from_secs(0),
        },
    )
    .expect("late prune");
    println!(
        "prune past the window: deleted {} objects, kept {}",
        report.deleted.len(),
        report.kept.len()
    );
}

/// The store layout after everything, listed the way a serving CDN sees
/// it: through `object_store`'s list API.
fn step5_print_layout(store: &ObjectStoreStore) {
    println!("store layout (via list_keys):");
    for key in store.list_keys().expect("the store lists") {
        println!("  {key}");
    }
}

fn main() {
    // Default: the network-free in-memory backend. A first argument swaps
    // in a real target URL (`s3://bucket/prefix`, `r2://bucket/prefix?endpoint=…`,
    // `file:///path`) — the parser is the same one `oxide deploy` will use.
    let args: Vec<String> = std::env::args().collect();
    let store = match args.get(1) {
        Some(url) => ObjectStoreStore::from_url(url).expect("the target URL parses"),
        None => ObjectStoreStore::new_in_memory().expect("the mem:// target constructs"),
    };
    let root = tempfile::tempdir().expect("the example's scratch dir");
    let dist = root.path().join("dist");
    make_dist(&dist);
    step1_full_deploy(&dist, &store);
    step2_delta_deploy(&dist, &store);
    step3_rollback(&store);
    step4_prune(&store);
    step5_print_layout(&store);
}
