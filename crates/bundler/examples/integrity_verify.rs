//! Consumer example for the integrity verifier (the V2-E04 mismatch-case
//! tail): the `oxide verify` walk that recomputes every
//! `client.manifest.json` `integrity` digest over the bytes actually on
//! disk.
//!
//! Shows the seam [`oxide_fw_bundler::sri::verify_integrity_map`] exposes:
//! where [`oxide_fw_bundler::sri::integrity_map`] — the bundler-side
//! writer — is lenient (it only names files it just wrote), the verifier
//! is strict: a missing file is a finding, a digest that no longer matches
//! is a finding, and both are the operator's verdict. The CLI maps a dirty
//! report to the fail-closed `VRF-INTEGRITY` diagnostic (exit 1); this
//! example prints the same report it would produce.
//!
//! Run it:
//!
//! ```text
//! cargo run -p oxide-fw-bundler --example integrity_verify
//! ```

use oxide_fw_bundler::sri::{sri_digest, verify_integrity_map};
use std::collections::BTreeMap;

fn main() {
    // Three "emitted" files with their recorded digests — the shape
    // `dist/client/` + the manifest's `integrity` map has at verify time.
    let dir = tempfile::tempdir().expect("tempdir");
    let files = [
        ("index-a1b2.js", "export const a = 1;\n"),
        ("index-c3d4.js", "export const b = 2;\n"),
        ("index-e5f6.css", ".a { color: rebeccapurple; }\n"),
    ];
    let mut map = BTreeMap::new();
    for (name, body) in files {
        std::fs::write(dir.path().join(name), body).expect("write emitted file");
        map.insert(name.to_string(), sri_digest(body.as_bytes()));
    }

    // The clean walk:
    let report = verify_integrity_map(dir.path(), &map);
    println!(
        "intact dist:  checked {} files, clean = {}",
        report.checked,
        report.is_clean()
    );

    // Tamper with one chunk — the mismatch case the E04 record left open.
    std::fs::write(dir.path().join("index-a1b2.js"), "export const a = 9;\n").expect("tamper");
    // And delete one — a missing file is its own finding.
    std::fs::remove_file(dir.path().join("index-c3d4.js")).expect("remove");

    let report = verify_integrity_map(dir.path(), &map);
    println!(
        "tampered dist: checked {} files, clean = {}",
        report.checked,
        report.is_clean()
    );
    for file in &report.missing {
        println!("  MISSING {file}");
    }
    for f in &report.mismatched {
        println!(
            "  MISMATCH {} (expected {}, got {})",
            f.file,
            f.expected,
            f.actual.as_deref().unwrap_or("unreadable")
        );
    }
    println!("\n`oxide verify` maps this report to the VRF-INTEGRITY diagnostic and exit 1.");
}
