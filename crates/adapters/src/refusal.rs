//! The §18 config-key refusal rule — V2-R02's config half (spec §V2-R02).
//!
//! The matrix says what each target class can honor; the server-manifest
//! refusal (`check_requirements`) covers what the build output REQUIRES;
//! this seam covers what the project's configuration and sources EXPOSE.
//! The config keys and project facts (`page.server.ts` `live: true`,
//! `*.server.rs` files, `fonts.subset: 'used'`, non-`static` routes) live
//! in the config/CLI layer — that layer reports them through
//! [`ConfigFeatureReport`], and this module turns the report into
//! refusals (errors) and degradation warnings against [`crate::support`].
//!
//! The seam consults the matrix — it never keeps a second opinion. Every
//! verdict is `support(target, capability)` re-stated as a `Diagnostic`,
//! so the matrix stays the single source of truth (the V2-R02 shape: the
//! map, the function and the refusal cannot disagree).

use crate::{Capability, FeatureSupport, Target, degradation, support};
use oxide_fw_diagnostics::Diagnostic;

/// The feature surface the §18 config-key refusal consults (V2-R02's
/// config half, spec §V2-R02).
///
/// Implemented by the config layer (which owns the config keys) over the
/// typed config plus the project facts only the driver can know — the
/// adapters crate never reads `oxide.config.ts` or walks a project (the
/// recorded V2-R02 deviation). A fixture implementing this trait is also
/// the seam's test vocabulary.
///
/// The method set is the config surface that exists TODAY, including F09's
/// `search` key: `search_configured` joined the set with its `AST-SRC-001`
/// refusal arm (additive, like a matrix row — D39's rule applied to the
/// refusal seam).
pub trait ConfigFeatureReport {
    /// A route opted into the live loaders (`page.server.ts` config
    /// `live: true` / `live: { debounceMs }`, V2-F05). The refusal maps
    /// this onto the `Live` capability.
    #[must_use]
    fn live_routes_present(&self) -> bool;

    /// Native Rust server functions (`*.server.rs`) are present in the
    /// project (V2-F08's surface). The refusal maps this onto the
    /// `RustFns` capability; a project carrying the files cannot deploy
    /// them to a target that cannot load a native binary.
    #[must_use]
    fn rust_fns_present(&self) -> bool;

    /// The built-in search is configured (`search.collections` carries at
    /// least one collection, V2-F09 / Appendix V2.L). The refusal maps
    /// this onto the `Search` capability: edge targets refuse
    /// (`AST-SRC-001`), Node/serverless/static ship degraded (static JSON).
    /// The §16.5 per-collection indexes are static JSON — the registry's
    /// own remedy for unsupported targets — so their presence must NOT set
    /// this; only the config key does.
    #[must_use]
    fn search_configured(&self) -> bool;

    /// `fonts.subset` carries the `'used'` glyph-usage spelling (C08:
    /// `subset: ['used']`). Only sound when every route is `static` — all
    /// text must be known at build time.
    #[must_use]
    fn font_subset_used(&self) -> bool;

    /// Any routable page renders non-`static` (§10.1: `dynamic`, `isr`,
    /// `spa`, `islands`, or a server module without an explicit mode).
    /// The second half of the [`ConfigFeatureReport::font_subset_used`]
    /// rule.
    #[must_use]
    fn non_static_routes_present(&self) -> bool;
}

/// The registry fix text for `AST-LIV-001` (Appendix V2.K).
const AST_LIV_001_FIX: &str = "Use `live.transport: 'poll'` or change target";
/// The registry fix text for `RFN-002` (Appendix V2.K).
const RFN_002_FIX: &str = "Use Binary or Docker, or remove the Rust functions";
/// The registry fix text for `AST-SRC-001` (Appendix V2.K).
const AST_SRC_001_FIX: &str = "Use static JSON search or a Binary or Docker target";
/// The registry fix text for `AST-FNT-001` (Appendix V2.K).
const AST_FNT_001_FIX: &str = "Use `latin` or `unicode-range`, or make all routes static";

/// The §18 config-key refusal: for each feature the report exposes, the
/// target's matrix cell decides the verdict (spec §V2-R02).
///
/// - `Unsupported` → an **error** diagnostic with the registry code
///   (`AST-LIV-001`, `RFN-002`, `AST-SRC-001`) and fix text — the build
///   refuses.
/// - `Degraded` → a **warning** diagnostic carrying the documented
///   degradation string (V2-K: "Degraded yields a warning with the
///   documented degradation string") — the build proceeds, degraded.
/// - `Full` → nothing.
///
/// The fonts rule (C08) is target-*independent*: glyph-usage subsetting is
/// refuted by the presence of any non-`static` route on every target.
///
/// Findings come back in a deterministic order (live, then rust fns, then
/// search — capability declaration order — then the target-independent
/// fonts rule), so callers print them the same way every run.
///
/// This is pure and infallible: no I/O, no config read — the report is the
/// input. Returns every finding (errors and warnings together, ordered);
/// callers split by [`oxide_fw_diagnostics::Severity::Error`] to refuse.
#[must_use]
pub fn check_config(report: &impl ConfigFeatureReport, target: Target) -> Vec<Diagnostic> {
    let mut findings = Vec::new();

    // Live first (capability declaration order): routes opted in via
    // `page.server.ts` `live: true` (V2-F05) against the `Live` row.
    if report.live_routes_present() {
        match support(target, Capability::Live) {
            FeatureSupport::Unsupported => findings.push(
                Diagnostic::error(
                    "AST-LIV-001",
                    format!(
                        "a route opts into `live: true` but target `{}` does not support \
                         live loaders (§18 capability matrix)",
                        target.as_str()
                    ),
                )
                .with_fix(AST_LIV_001_FIX),
            ),
            FeatureSupport::Degraded => {
                let reason = degradation(target, Capability::Live)
                    .unwrap_or("no documented degradation reason");
                findings.push(
                    Diagnostic::warning(
                        "AST-LIV-001",
                        format!(
                            "live routes on target `{}` degrade: {reason}",
                            target.as_str()
                        ),
                    )
                    .with_fix(AST_LIV_001_FIX),
                );
            }
            FeatureSupport::Full => {}
        }
    }

    // Rust fns second: `*.server.rs` sources (V2-F08) against the `RustFns`
    // row — the message names the adapter, per the F08 spec sentence.
    if report.rust_fns_present()
        && support(target, Capability::RustFns) == FeatureSupport::Unsupported
    {
        findings.push(
            Diagnostic::error(
                "RFN-002",
                format!(
                    "`*.server.rs` functions are present but target `{}` cannot load native \
                     functions (V2-F08)",
                    target.as_str()
                ),
            )
            .with_fix(RFN_002_FIX),
        );
    }

    // Search third (capability declaration order: RustFns < Search): the
    // `search.collections` key (V2-F09) against the `Search` row — edge
    // refuses (`AST-SRC-001`), Node/serverless/static warn with the
    // documented degradation string (the static-JSON fallback).
    if report.search_configured() {
        match support(target, Capability::Search) {
            FeatureSupport::Unsupported => findings.push(
                Diagnostic::error(
                    "AST-SRC-001",
                    format!(
                        "`search` is configured but target `{}` does not support the \
                         built-in search capability (§18 capability matrix)",
                        target.as_str()
                    ),
                )
                .with_fix(AST_SRC_001_FIX),
            ),
            FeatureSupport::Degraded => {
                let reason = degradation(target, Capability::Search)
                    .unwrap_or("no documented degradation reason");
                findings.push(
                    Diagnostic::warning(
                        "AST-SRC-001",
                        format!(
                            "`search` on target `{}` degrades: {reason}",
                            target.as_str()
                        ),
                    )
                    .with_fix(AST_SRC_001_FIX),
                );
            }
            FeatureSupport::Full => {}
        }
    }

    // Fonts last: the C08 glyph-usage rule is target-independent —
    // `fonts.subset: 'used'` is only sound when every route is `static`.
    if report.font_subset_used() && report.non_static_routes_present() {
        findings.push(
            Diagnostic::error(
                "AST-FNT-001",
                "`fonts.subset: 'used'` requires every route to be `static` (glyph-usage \
                 subsetting needs all text known at build), but the project has non-static \
                 routes",
            )
            .with_fix(AST_FNT_001_FIX),
        );
    }

    findings
}
