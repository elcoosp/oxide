//! TDD pins for the V2-R02 capability-matrix v2 (spec §V2-R02: "Add
//! capabilities without ever editing existing rows (D39), keep the v0.1
//! rows byte-identical, and make every build refuse configurations a
//! target cannot honor").
//!
//! Written FIRST (red): failed to compile before the v0.2 `Capability`
//! variants existed and `support` replaced the fixed-size row tuples.
//!
//! The seam this pins: the matrix is a total function of
//! `(target class, capability)` — exhaustive over the pair space, so the
//! compiler proves every capability declares a cell for every target
//! class, and adding a capability means adding arms, never touching
//! others.

use oxide_fw_adapters::{
    BuildArtifact, Capability, FeatureSupport, Target, capability_matrix, degradation, emit,
    support,
};
use std::collections::BTreeMap;
use tempfile::TempDir;

/// The v0.1 golden row shape (one cell per v0.1 capability).
type GoldenRow = [(Capability, FeatureSupport); 7];
/// The v0.2 spec-check shape: capability → (target groups, expected cell).
type SpecCheck = (Capability, Vec<(Vec<Target>, FeatureSupport)>);

/// The v0.1 rows, cell by cell, exactly as the fixed-size `row([...])`
/// table spelled them before the v2 seam — the golden pin D39 demands
/// ("keep the v0.1 rows byte-identical").
#[test]
fn v01_rows_are_byte_identical_to_the_pre_v2_table() {
    use Capability as C;
    use FeatureSupport as S;
    // (target class, [(cap, cell); 7]) — the §18 table's original shape.
    let native: [(C, S); 7] = [
        (C::L0Cache, S::Full),
        (C::Isr, S::Full),
        (C::Sockets, S::Full),
        (C::ImageOptimization, S::Full),
        (C::NativeRules, S::Full),
        (C::Snapshots, S::Full),
        (C::Tls, S::Full),
    ];
    let node_like: [(C, S); 7] = [
        (C::L0Cache, S::Full),
        (C::Isr, S::Full),
        (C::Sockets, S::Full),
        (C::ImageOptimization, S::Full),
        (C::NativeRules, S::Full),
        (C::Snapshots, S::Degraded),
        (C::Tls, S::Full),
    ];
    let static_row: [(C, S); 7] = [
        (C::L0Cache, S::Degraded),
        (C::Isr, S::Unsupported),
        (C::Sockets, S::Unsupported),
        (C::ImageOptimization, S::Degraded),
        (C::NativeRules, S::Degraded),
        (C::Snapshots, S::Unsupported),
        (C::Tls, S::Unsupported),
    ];
    let edge: [(C, S); 7] = [
        (C::L0Cache, S::Degraded),
        (C::Isr, S::Degraded),
        (C::Sockets, S::Unsupported),
        (C::ImageOptimization, S::Unsupported),
        (C::NativeRules, S::Full),
        (C::Snapshots, S::Unsupported),
        (C::Tls, S::Full),
    ];
    let serverless: [(C, S); 7] = [
        (C::L0Cache, S::Degraded),
        (C::Isr, S::Degraded),
        (C::Sockets, S::Unsupported),
        (C::ImageOptimization, S::Degraded),
        (C::NativeRules, S::Full),
        (C::Snapshots, S::Degraded),
        (C::Tls, S::Unsupported),
    ];
    let classes: &[(Vec<Target>, GoldenRow)] = &[
        (vec![Target::Binary, Target::Docker], native),
        (vec![Target::Node, Target::Deno, Target::Bun], node_like),
        (vec![Target::Static], static_row),
        (vec![Target::Cloudflare, Target::CloudflarePages], edge),
        (
            vec![
                Target::Lambda,
                Target::Vercel,
                Target::VercelFunctions,
                Target::Netlify,
            ],
            serverless,
        ),
    ];
    for (targets, row) in classes {
        for t in targets {
            for (cap, cell) in row {
                assert_eq!(
                    support(*t, *cap),
                    *cell,
                    "{t:?} × {cap:?} drifted from the v0.1 table"
                );
            }
        }
    }
}

/// The v0.2 rows, per the spec's §V2-R02 `support` match. These are NEW
/// rows — written once here, never edited by a later capability.
#[test]
fn v02_rows_match_the_spec() {
    use Capability as C;
    use FeatureSupport as S;
    let native = [Target::Binary, Target::Docker];
    let node_like = [Target::Node, Target::Deno, Target::Bun];
    let edge = [Target::Cloudflare, Target::CloudflarePages];
    let serverless = [
        Target::Lambda,
        Target::Vercel,
        Target::VercelFunctions,
        Target::Netlify,
    ];
    let checks: &[SpecCheck] = &[
        (
            C::MixedIslands,
            vec![
                (native.to_vec(), S::Full),
                (node_like.to_vec(), S::Full),
                (vec![Target::Static], S::Full),
                (serverless.to_vec(), S::Full),
                (edge.to_vec(), S::Degraded),
            ],
        ),
        (
            C::Resume,
            vec![
                (native.to_vec(), S::Full),
                (node_like.to_vec(), S::Full),
                (vec![Target::Static], S::Unsupported),
                (edge.to_vec(), S::Unsupported),
                (serverless.to_vec(), S::Unsupported),
            ],
        ),
        (
            C::Skew,
            vec![
                (native.to_vec(), S::Full),
                (node_like.to_vec(), S::Degraded),
                (vec![Target::Static], S::Degraded),
                (edge.to_vec(), S::Degraded),
                (serverless.to_vec(), S::Degraded),
            ],
        ),
        (
            C::DeltaDeploy,
            vec![
                (native.to_vec(), S::Full),
                (vec![Target::Static], S::Full),
                (node_like.to_vec(), S::Degraded),
                (edge.to_vec(), S::Degraded),
                (serverless.to_vec(), S::Degraded),
            ],
        ),
        (
            C::RustFns,
            vec![
                (native.to_vec(), S::Full),
                (node_like.to_vec(), S::Unsupported),
                (vec![Target::Static], S::Unsupported),
                (edge.to_vec(), S::Unsupported),
                (serverless.to_vec(), S::Unsupported),
            ],
        ),
        (
            C::Search,
            vec![
                (native.to_vec(), S::Full),
                (node_like.to_vec(), S::Degraded),
                (vec![Target::Static], S::Degraded),
                (serverless.to_vec(), S::Degraded),
                (edge.to_vec(), S::Unsupported),
            ],
        ),
        (C::Offline, vec![(Target::ALL.to_vec(), S::Full)]),
        (
            C::Brownout,
            vec![
                (native.to_vec(), S::Full),
                (node_like.to_vec(), S::Degraded),
                (vec![Target::Static], S::Unsupported),
                (edge.to_vec(), S::Unsupported),
                (serverless.to_vec(), S::Unsupported),
            ],
        ),
        (
            C::Permissions,
            vec![
                (native.to_vec(), S::Full),
                (node_like.to_vec(), S::Degraded),
                (vec![Target::Static], S::Unsupported),
                (edge.to_vec(), S::Unsupported),
                (serverless.to_vec(), S::Unsupported),
            ],
        ),
        (C::Attestation, vec![(Target::ALL.to_vec(), S::Full)]),
    ];
    for (cap, cells) in checks {
        for (targets, cell) in cells {
            for t in targets {
                assert_eq!(support(*t, *cap), *cell, "{t:?} × {cap:?}");
            }
        }
    }
}

/// Every target's matrix carries ALL capabilities — no target falls back
/// to a missing key (the matrix is the machine-readable source of truth
/// for the refusal rule).
#[test]
fn every_target_declares_every_capability() {
    for t in Target::ALL {
        let matrix: BTreeMap<Capability, FeatureSupport> = capability_matrix(*t);
        assert_eq!(matrix.len(), Capability::ALL.len(), "{t:?} row is complete");
        for cap in Capability::ALL {
            assert!(
                matrix.contains_key(cap),
                "{t:?} is missing the {cap:?} cell"
            );
        }
    }
}

/// Every Degraded cell documents its reason — the new rows carry the same
/// obligation the F05 Live row set (a degradation string for every `D`).
#[test]
fn every_degraded_cell_documents_its_reason() {
    for t in Target::ALL {
        for (cap, cell) in capability_matrix(*t) {
            if cell == FeatureSupport::Degraded {
                let reason = degradation(*t, cap);
                assert!(
                    reason.is_some(),
                    "{t:?} × {cap:?} is Degraded with no documented reason"
                );
                assert_ne!(reason.unwrap_or_default(), "", "the reason is non-empty");
            }
        }
    }
}

/// `Capability::ALL` keeps the v0.1 declaration order first — the `BTreeMap`
/// iteration order is observable in docs and doctor output, and D39 says
/// existing rows are never reordered.
#[test]
fn capability_all_keeps_the_v01_order_first() {
    let all = Capability::ALL;
    let v01 = [
        "L0Cache",
        "Isr",
        "Sockets",
        "ImageOptimization",
        "NativeRules",
        "Snapshots",
        "Tls",
    ];
    for (i, name) in v01.iter().enumerate() {
        let cap = all[i];
        assert_eq!(
            format!("{cap:?}"),
            *name,
            "v0.1 order drifted at position {i}"
        );
    }
    assert!(all.len() > v01.len(), "the v0.2 rows are appended");
}

/// The refusal rule extends to the v0.2 `requires` keys: an Unsupported
/// capability refuses the build at emit time; a Degraded one does not (it
/// degrades, with the documented string).
#[test]
fn the_refusal_rule_covers_the_v02_requires_keys() {
    fn artifact_with(requires: &serde_json::Value) -> (TempDir, BuildArtifact) {
        let root = TempDir::new().unwrap();
        let artifact = BuildArtifact {
            out_dir: root.path().to_path_buf(),
            build_id: "test-build".into(),
            client_manifest: serde_json::json!({}),
            server_manifest: serde_json::json!({ "requires": requires }),
            prerendered: BTreeMap::new(),
            static_fallback: None,
            server_bundle: None,
        };
        (root, artifact)
    }
    // live on Static: Unsupported → the build refuses (the F05 shape, now
    // through the v2 seam).
    let (_d, a) = artifact_with(&serde_json::json!({ "live": true }));
    let out = TempDir::new().unwrap();
    let err = emit(Target::Static, &a, out.path()).expect_err("live on Static must refuse");
    assert_eq!(err.0.code, "ADP-CAP");
    // rustFns on Node: Unsupported → refuses.
    let (_d, a) = artifact_with(&serde_json::json!({ "rustFns": true }));
    let out = TempDir::new().unwrap();
    let err = emit(Target::Node, &a, out.path()).expect_err("rustFns on Node must refuse");
    assert_eq!(err.0.code, "ADP-CAP");
    // live on Lambda: Degraded → the build proceeds (poll fallback).
    let out = TempDir::new().unwrap();
    let (_d, a) = artifact_with(&serde_json::json!({ "live": true }));
    assert!(
        emit(Target::Lambda, &a, out.path()).is_ok(),
        "a Degraded capability degrades; it does not refuse"
    );
    // attestation everywhere: Full → proceeds.
    let out = TempDir::new().unwrap();
    let (_d, a) = artifact_with(&serde_json::json!({ "attestation": true }));
    assert!(emit(Target::Static, &a, out.path()).is_ok());
}
