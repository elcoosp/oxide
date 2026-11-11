//! TDD suite for the integrity verifier (the V2-E04 mismatch-case tail,
//! spec §V2-E04): the `oxide verify` walk that recomputes every
//! `client.manifest.json` `integrity` digest over the bytes actually on
//! disk. Where `integrity_map` (the bundler-side writer) is lenient by
//! design — it only names files it just wrote — the verifier is strict: a
//! missing file is a finding, a digest that no longer matches is a
//! finding, and both are the operator's verdict.

use oxide_fw_bundler::sri::{sri_digest, verify_integrity_map};
use std::collections::BTreeMap;
use std::path::Path;

/// Three emitted files under `out_dir`, with their recorded digests — the
/// shape `dist/client/` + `client.manifest.json.integrity` has at verify
/// time.
fn emitted_fixture(dir: &Path) -> BTreeMap<String, String> {
    let files = [
        ("index-a1b2.js", "export const a = 1;\n"),
        ("index-c3d4.js", "export const b = 2;\n"),
        ("index-e5f6.css", ".a { color: rebeccapurple; }\n"),
    ];
    let mut map = BTreeMap::new();
    for (name, body) in files {
        std::fs::write(dir.join(name), body).expect("write emitted file");
        map.insert(name.to_string(), sri_digest(body.as_bytes()));
    }
    map
}

#[test]
fn verify_integrity_map_reports_a_clean_walk() {
    let dir = tempfile::tempdir().unwrap();
    let map = emitted_fixture(dir.path());
    let report = verify_integrity_map(dir.path(), &map);
    assert!(report.is_clean(), "intact bytes verify clean");
    assert_eq!(report.checked, 3, "every recorded file was re-digested");
    assert_eq!(report.missing, Vec::<String>::new());
    assert_eq!(
        report.mismatched,
        Vec::<oxide_fw_bundler::sri::IntegrityFinding>::new()
    );
}

#[test]
fn verify_integrity_map_names_the_mismatched_file_and_the_actual_digest() {
    let dir = tempfile::tempdir().unwrap();
    let map = emitted_fixture(dir.path());
    // Tamper: the served chunk changed after the build recorded its
    // digest — the exact mismatch case the E04 record leaves open.
    std::fs::write(dir.path().join("index-a1b2.js"), "export const a = 9;\n").expect("tamper");
    let report = verify_integrity_map(dir.path(), &map);
    assert!(!report.is_clean());
    assert_eq!(report.checked, 3, "the tampered file was still read");
    assert_eq!(report.missing, Vec::<String>::new());
    assert_eq!(report.mismatched.len(), 1, "only the tampered file");
    let f = &report.mismatched[0];
    assert_eq!(f.file, "index-a1b2.js");
    assert_eq!(
        f.expected,
        sri_digest(b"export const a = 1;\n"),
        "expected is the recorded digest"
    );
    assert_eq!(
        f.actual.as_deref(),
        Some(sri_digest(b"export const a = 9;\n").as_str()),
        "actual is the digest of the bytes on disk"
    );
}

#[test]
fn verify_integrity_map_names_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let map = emitted_fixture(dir.path());
    std::fs::remove_file(dir.path().join("index-c3d4.js")).expect("remove");
    let report = verify_integrity_map(dir.path(), &map);
    assert!(!report.is_clean());
    assert_eq!(report.missing, vec!["index-c3d4.js".to_string()]);
    assert_eq!(report.checked, 2, "only the readable files count");
    assert_eq!(
        report.mismatched,
        Vec::<oxide_fw_bundler::sri::IntegrityFinding>::new(),
        "missing is not a mismatch"
    );
}

#[test]
fn verify_integrity_map_of_an_empty_map_is_clean() {
    // A pre-E04 manifest carries no integrity map; there is nothing to
    // verify, so the walk is trivially clean (the CLI skips loudly before
    // reaching this for the operator, but the seam itself stays total).
    let dir = tempfile::tempdir().unwrap();
    let report = verify_integrity_map(dir.path(), &BTreeMap::new());
    assert!(report.is_clean());
    assert_eq!(report.checked, 0);
}

#[test]
fn verify_integrity_map_is_deterministic_in_manifest_key_order() {
    // BTreeMap iteration is sorted, so the findings land in manifest key
    // order on every run — the same determinism the registry and the docs
    // pages pin.
    let dir = tempfile::tempdir().unwrap();
    let mut map = emitted_fixture(dir.path());
    map.insert("zzz-missing.js".to_string(), "sha384-ghost".to_string());
    std::fs::write(dir.path().join("index-a1b2.js"), "tampered").expect("tamper");
    let report = verify_integrity_map(dir.path(), &map);
    assert_eq!(report.missing, vec!["zzz-missing.js".to_string()]);
    assert_eq!(report.mismatched.len(), 1);
    assert_eq!(report.mismatched[0].file, "index-a1b2.js");
}
