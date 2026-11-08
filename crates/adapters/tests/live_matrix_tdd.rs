//! TDD pins for the V2-F05 `Live` capability row (spec §V2-F05 step 9:
//! "Capability row `Live`: Full on Binary, Node, Docker; Degraded on
//! serverless (forces `transport: 'poll'`); Unsupported on Static").
//!
//! Written FIRST (red): failed to compile before `Capability::Live`
//! existed and every row was a 7-tuple.

use oxide_fw_adapters::{Capability, FeatureSupport, Target, capability_matrix, degradation};

/// The spec's §V2-F05 step 9 row, pinned per target.
#[test]
fn live_row_matches_the_spec() {
    let full = [Target::Binary, Target::Docker, Target::Node];
    for t in full {
        assert_eq!(
            capability_matrix(t).get(&Capability::Live),
            Some(&FeatureSupport::Full),
            "{t:?} holds long-lived SSE connections"
        );
    }
    let serverless = [
        Target::Lambda,
        Target::Vercel,
        Target::VercelFunctions,
        Target::Netlify,
    ];
    for t in serverless {
        assert_eq!(
            capability_matrix(t).get(&Capability::Live),
            Some(&FeatureSupport::Degraded),
            "{t:?} forces transport: 'poll'"
        );
        assert!(
            degradation(t, Capability::Live).is_some(),
            "the Degraded cell documents its reason for {t:?}"
        );
    }
    assert_eq!(
        capability_matrix(Target::Static).get(&Capability::Live),
        Some(&FeatureSupport::Unsupported),
        "Static has no server to hold a stream (the build refuses live: true, AST-LIV-001)"
    );
}

/// Deno and Bun are NodeHost-backed runtimes with full sockets (§18):
/// they hold SSE connections exactly like Node.
#[test]
fn node_host_runtimes_hold_sse_like_node() {
    for t in [Target::Deno, Target::Bun] {
        assert_eq!(
            capability_matrix(t).get(&Capability::Live),
            Some(&FeatureSupport::Full),
            "{t:?} matches Node's sockets row"
        );
    }
}

/// Cloudflare Workers can stream responses but do not guarantee long-lived
/// connections: Degraded with a documented poll fallback.
#[test]
fn edge_workers_degrade_to_poll() {
    for t in [Target::Cloudflare, Target::CloudflarePages] {
        assert_eq!(
            capability_matrix(t).get(&Capability::Live),
            Some(&FeatureSupport::Degraded),
            "{t:?} falls back to transport: 'poll'"
        );
        let reason = degradation(t, Capability::Live).expect("documented");
        assert!(
            reason.contains("poll"),
            "the degradation names the poll fallback: {reason}"
        );
    }
}

/// Every target's matrix row carries the new cell — no target silently
/// falls back to a missing key (the matrix is the machine-readable source
/// of truth for the refusal rule).
#[test]
fn every_target_declares_the_live_cell() {
    for t in Target::ALL {
        assert!(
            capability_matrix(*t).contains_key(&Capability::Live),
            "{t:?} must declare Live"
        );
    }
}
