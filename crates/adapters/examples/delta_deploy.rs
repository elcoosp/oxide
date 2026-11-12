//! The delta-deploy walkthrough (§V2-F11): a fresh store takes a full
//! deploy, a one-line change takes a delta orders of magnitude smaller,
//! a rollback re-points `current` in one small write, and a prune removes
//! only what has served its skew window.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-adapters --example delta_deploy
//! ```

use std::collections::BTreeMap;
use std::time::Duration;

use oxide_fw_adapters::{
    DEFAULT_SKEW_WINDOW, DeployOptions, DeployStore, DirStore, PruneOptions, deploy_dir, prune,
    rollback,
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

/// The first deploy: a fresh store uploads everything.
fn step1_full_deploy(dist: &std::path::Path, store: &DirStore) {
    let first = deploy_dir(
        dist,
        store,
        &DeployOptions {
            build_id: "0123456789abcdef".to_string(),
            now_ms: 1_000,
        },
    )
    .expect("the first deploy");
    println!("deploy 1 (fresh store)");
    println!(
        "  upload {} files, {} bytes, keep {}",
        first.upload.len(),
        first.bytes_up,
        first.keep.len()
    );
    println!(
        "  pointer = {}",
        store.read_pointer().expect("pointer").unwrap_or_default()
    );
}

/// The one-line change: one hashed chunk flips, everything else is a
/// keep — the delta is the §V2-F11 acceptance case (< 5% of the bytes).
fn step2_delta_deploy(dist: &std::path::Path, store: &DirStore) {
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
        "  upload {:?}, {} bytes, keep {} files",
        second.upload,
        second.bytes_up,
        second.keep.len()
    );
    let previous = store
        .load_manifest()
        .expect("manifest")
        .expect("exists")
        .previous_build_id
        .expect("recorded");
    println!("  previous = {previous}");
}

/// Rollback: `current` re-points to the previous build in one small write.
fn step3_rollback(store: &DirStore) {
    let rolled = rollback(store).expect("rollback");
    println!("rollback -> current = {rolled}");
}

/// Prune inside the window: the young marks survive.
fn step4_prune_young(store: &DirStore) {
    let report = prune(
        store,
        &PruneOptions {
            now_ms: 3_000,
            window: DEFAULT_SKEW_WINDOW,
        },
    )
    .expect("prune");
    println!(
        "prune at t=3s, window {}s: deleted {}, kept {}",
        DEFAULT_SKEW_WINDOW.as_secs(),
        report.deleted.len(),
        report.kept.len()
    );
}

/// A prune past the window: the v1 release copies go. The pointer names
/// v2 again (step 5 re-deploys it), so nothing protects the v1 prefix.
fn step5_prune_late(store: &DirStore, dist: &std::path::Path) {
    deploy_dir(
        dist,
        store,
        &DeployOptions {
            build_id: "2222222222222222".to_string(),
            now_ms: 4_000,
        },
    )
    .expect("re-deploy v2 (the pointer returns to v2)");
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
        "prune past the window: deleted {} objects:",
        report.deleted.len()
    );
    for path in &report.deleted {
        println!("  - {path}");
    }
}

/// The store layout after everything: immutable objects at rel paths, one
/// release prefix per build, the pointer, the ledger.
fn print_layout(store_root: &std::path::Path) {
    let mut layout = BTreeMap::new();
    collect(store_root, "", &mut layout);
    println!("store layout:");
    for (rel, kind) in &layout {
        println!("  {rel}  ({kind})");
    }
}

fn main() {
    let root = tempfile::tempdir().expect("the example's scratch dir");
    let dist = root.path().join("dist");
    let store_root = root.path().join("store");
    make_dist(&dist);
    let store = DirStore::new(&store_root);
    step1_full_deploy(&dist, &store);
    step2_delta_deploy(&dist, &store);
    step3_rollback(&store);
    step4_prune_young(&store);
    step5_prune_late(&store, &dist);
    print_layout(&store_root);
}

fn collect(dir: &std::path::Path, prefix: &str, out: &mut BTreeMap<String, &'static str>) {
    for entry in std::fs::read_dir(dir).expect("walk") {
        let entry = entry.expect("entry");
        let rel = format!("{prefix}{}", entry.file_name().to_string_lossy());
        if entry.file_type().expect("type").is_dir() {
            collect(&entry.path(), &format!("{rel}/"), out);
        } else {
            let kind = match rel.as_str() {
                "current" => "pointer",
                "manifest.json" => "ledger",
                _ => "object",
            };
            out.insert(rel, kind);
        }
    }
}
