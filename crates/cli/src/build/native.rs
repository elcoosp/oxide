//! phase_native — the §V2-F08 finish's production emission: build the
//! native server-fn crate in release, deploy the cdylib under
//! `dist/native/` beside the types payload, and leave the mount copy under
//! `.oxide/rust/` for the serve boot walk. The A04 build id covers
//! `dist/native`, so a rebuilt library is a new build identity (the spec's
//! "records its hash in the build id inputs").
//!
//! Ordering: the phase runs AFTER `phase_plan` and BEFORE
//! `phase_bundle_client` — the bundle-time `.server.rs` stub hooks read
//! the payload the emission writes (`.oxide/rust/types.json`).

use crate::*;

// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
pub(crate) async fn phase_native(cx: &mut BuildCtx<'_>) -> anyhow::Result<()> {
    let rootp = &cx.rootp;
    let dist = &cx.dist;
    let w = &mut *cx.w;
    // The consistency guard first: `.server.rs` sources with no
    // discoverable cdylib crate refuse the build (`RFN-005`) — the stubs
    // would import ids nothing answers.
    crate::native_fns::ensure_native_crate_consistency(rootp)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    let Some(nc) = crate::native_fns::discover_native_crate(rootp) else {
        // The pre-finish default: no native crate, nothing to emit (the
        // build stays byte-identical for every non-native project).
        return Ok(());
    };
    // The secret's create-once side effect: the ids the consult derives at
    // serve time hash the same value the stub generator used.
    build_secret(rootp)?;
    let spec = oxide_fw_dev::server_lib::ServerLibSpec::new(nc.name.clone(), nc.dir.clone());
    let report =
        crate::native_fns::emit_native_lib(rootp, dist, &spec, &crate::native_fns::CargoEmitSource)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    outln!(
        w,
        "native fns: crate `{}` — {} export(s), {} bytes → dist/native (the build id covers the bytes)",
        nc.name,
        report.fn_count,
        report.bytes
    );
    Ok(())
}
// <<< phase-wrapper
