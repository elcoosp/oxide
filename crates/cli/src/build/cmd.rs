//! V2-A01 phase 2: `cmd_build` is now the phase orchestrator.
//!
//! The spec's phase sketch (`phase_load` -> `phase_bundle_client` ->
//! `phase_bundle_server` -> `phase_prerender` -> `phase_seed_l0` ->
//! `phase_emit`) maps onto the tree as: [`phase_load`], [`phase_plan`] (the
//! entry-set/options front half of the bundle phases — split out to keep every
//! phase file under the 700-line budget), [`phase_bundle_client`],
//! [`phase_bundle_server`], [`phase_prerender`], [`phase_seed_l0`],
//! [`phase_ppr_seed`] (the §10.4/P0-4 build-time shell manifest — the
//! prerender walk's boundary-carrying pages, the l0-seed pattern),
//! [`phase_typegen`] (the §8.3/§16-D typegen tail — same budget reason) and
//! [`phase_emit`]. Bodies moved verbatim; the shared locals are the
//! `BuildCtx` fields; `scripts/split_selfcheck.py --phases` proves the
//! concatenation equals the pre-split `cmd_build` body line-for-line.

use super::*;
use crate::*;

/// The `oxide build` command (spec §10). Phase order is the pipeline contract:
/// every phase's `expect` on its inputs names the phase that must run before it.
pub(crate) async fn cmd_build(
    root: &str,
    r#static: bool,
    explain: bool,
    attest: bool,
    key_path: Option<&str>,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
    let Some(mut cx) = phase_load(root, r#static, explain, attest, key_path, w).await? else {
        // §5.1 early cutoff: up to date, nothing to do (the pre-split `return Ok(0)`).
        return Ok(0);
    };
    phase_plan(&mut cx).await?;
    // §V2-F08 finish: the native emission runs BEFORE the bundles — the
    // bundle-time `.server.rs` stub hooks read the payload it writes, and
    // the cdylib bytes it deploys join the A04 covered set.
    phase_native(&mut cx).await?;
    phase_bundle_client(&mut cx).await?;
    phase_bundle_server(&mut cx).await?;
    phase_prerender(&mut cx).await?;
    phase_seed_l0(&mut cx)?;
    phase_ppr_seed(&mut cx)?;
    phase_typegen(&mut cx)?;
    phase_emit(&mut cx).await?;
    // <<< phase-wrapper
    Ok(0)
}
