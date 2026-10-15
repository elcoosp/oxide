//! §4.4: build a V8 startup snapshot from a **multi-file** server bundle.
//!
//! Ignored by default (boots V8 snapshot machinery) and needs a prior
//! `oxide build examples/smoke-app-react`. Run with `--ignored`.

use oxide_jshost::{snapshot_build_bundle, SNAPSHOT_FORMAT};

fn collect(root: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(&p, out);
            } else if p.extension().is_some_and(|x| x == "js") {
                if let Ok(s) = std::fs::read_to_string(&p) {
                    out.push((p, s));
                }
            }
        }
    }
}

#[test]
#[ignore = "boots V8 snapshot machinery; needs a prior `oxide build`; run with --ignored"]
fn snapshot_a_real_bundle_graph() {
    let server = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/smoke-app-react/dist/server");
    let entry = server.join("ssr/index.js");
    if !entry.is_file() {
        eprintln!("SKIP: run `oxide build examples/smoke-app-react` first");
        return;
    }
    let mut files = Vec::new();
    collect(&server, &mut files);
    assert!(files.len() >= 3, "expected a multi-file bundle, got {}", files.len());

    let snap = snapshot_build_bundle(&entry.to_string_lossy(), &files).unwrap();
    assert_eq!(snap.format, SNAPSHOT_FORMAT);
    assert!(
        snap.warnings.is_empty(),
        "snapshot should build cleanly: {:?}",
        snap.warnings
    );
    assert!(
        !snap.blob.is_empty(),
        "expected a real V8 startup blob, got an empty one"
    );
    // A populated snapshot of a real bundle is hundreds of KB.
    assert!(snap.blob.len() > 100_000, "blob too small: {}", snap.blob.len());
    assert_eq!(snap.source_hash.len(), 64);
}

#[test]
fn snapshot_bundle_handles_a_missing_entry_path() {
    // A non-file entry degrades to an empty snapshot with a warning, never a
    // panic — the caller falls back to no snapshot.
    let snap = snapshot_build_bundle("not-a-file", &[]).unwrap();
    assert!(snap.blob.is_empty());
    assert!(!snap.warnings.is_empty());
}