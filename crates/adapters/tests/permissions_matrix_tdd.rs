//! V2-F19: the per-route capability permissions matrix truthfulness pin.
//!
//! The `Capability::Permissions` row is `Full` on `Native` (the binary
//! enforces its own grant table via the `PermGuard` + per-op boundary
//! checks, closed by the profile-keyed isolate pool — Model 2), `Degraded`
//! on `NodeLike` (the worker checks, the runtime cannot sandbox — the
//! `op_dispatch` Fetch arm emits a structured `PRM-001` warn, D35), and
//! `Unsupported` elsewhere. This pin holds the row truthful against the
//! landed enforcement — the preemptive `Full` cell (recorded before F19
//! landed) is now backed by the substrate.
//!
//! The drift guard between this Rust row and the TS conformance bridge
//! lives in `matrix_json.rs` (reload `conformance/matrix.json` and assert
//! every cell matches); this pin holds the Rust-side row's semantics.

use oxide_fw_adapters::{Capability, FeatureSupport, Target, degradation, support};

#[test]
fn permissions_is_full_on_native_backed_by_the_landed_enforcement() {
    // The Native class (Binary, Docker) — the binary enforces its own
    // grant table. The F19 enforcement landed (the `PermGuard` RAII, the
    // per-op boundary checks, the profile-keyed isolate pool); the
    // preemptive `Full` cell is now truthful.
    for t in [Target::Binary, Target::Docker] {
        assert_eq!(
            support(t, Capability::Permissions),
            FeatureSupport::Full,
            "{t:?} enforces the grant table (F19 landed)"
        );
    }
}

#[test]
fn permissions_is_degraded_on_node_like_with_the_advisory_reason() {
    // D35: the NodeLike runtime cannot sandbox a violation, so the
    // enforcement is advisory (a structured warn, never a hard block).
    for t in [Target::Node, Target::Deno, Target::Bun] {
        assert_eq!(
            support(t, Capability::Permissions),
            FeatureSupport::Degraded,
            "{t:?} is advisory (D35)"
        );
        let reason = degradation(t, Capability::Permissions);
        assert!(
            reason.is_some_and(|r| r.contains("grant table") && r.contains("cannot sandbox")),
            "{t:?} carries the advisory degradation reason (got {reason:?})"
        );
    }
}

#[test]
fn permissions_is_unsupported_where_no_server_runs() {
    for t in [
        Target::Static,
        Target::Cloudflare,
        Target::CloudflarePages,
        Target::Lambda,
        Target::Vercel,
        Target::VercelFunctions,
        Target::Netlify,
    ] {
        assert_eq!(
            support(t, Capability::Permissions),
            FeatureSupport::Unsupported,
            "{t:?} has no server to enforce against"
        );
        // Unsupported cells have no degradation reason.
        assert!(
            degradation(t, Capability::Permissions).is_none(),
            "{t:?} has no degradation reason (Unsupported)"
        );
    }
}

#[test]
fn permissions_row_is_in_the_matrix_and_the_all_list() {
    // The row is declared in `Capability::ALL` (the matrix folds over it)
    // and serialized as `"permissions"` — the stable spelling the doctor
    // column, the conformance bridge and the generated docs table print.
    assert!(Capability::ALL.contains(&Capability::Permissions));
    assert_eq!(Capability::Permissions.as_str(), "permissions");
}
