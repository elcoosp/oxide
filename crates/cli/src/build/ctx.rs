//! V2-A01 phase 2: the build-phase context (spec WP-A01 "Step 2" sketch).
//!
//! `cmd_build`'s ~2,300-line body shared its locals across the phase boundaries;
//! those locals are the [`BuildCtx`] fields here. The command arguments and the
//! canonical paths are plain fields set by [`phase_load`]; every later value is
//! an `Option` field that the phase which creates it fills in (the orchestrator
//! fixes the phase order, so a `None` read through a phase's `expect` is a
//! contract violation, never a run-time branch). The fields whose own value is
//! an `Option` (`boot_chunk_url`, `boot_script_url`, `boot_script_integrity`,
//! `head_producer`, `css_transform` rides `Some`, `transform_html_hook`) are
//! bare `Option<T>` so consumers never see `Option<Option<_>>`.

use crate::*;

/// The §6.1 entry set: route id → on-disk entry path (the alias keeps the
/// `BuildCtx` field declarations under clippy's type-complexity budget).
pub(crate) type EntryMap = std::collections::BTreeMap<String, std::path::PathBuf>;

/// One L0-seed row: (route_id, url_path, sentineled html, status, locale).
pub(crate) type SeedBody = (String, String, String, u16, String);

/// The per-build context threaded through the named phase functions.
pub(crate) struct BuildCtx<'w> {
    // ---- core: command arguments + canonical paths (set by phase_load) ----
    /// The canonicalized project root (CONVENTIONS: path canonicalization; the
    /// sketch calls it `root` — `rootp` is the function's own local name).
    pub(crate) rootp: std::path::PathBuf,
    /// The dist/ directory (the sketch's `out`).
    pub(crate) dist: std::path::PathBuf,
    pub(crate) w: Writer<'w>,
    pub(crate) r#static: bool,
    pub(crate) explain: bool,
    pub(crate) attest: bool,
    pub(crate) key_path: Option<&'w str>,
    // ---- phase_load outputs ----
    pub(crate) cfg: oxide_fw_config::OxideConfig,
    pub(crate) tree: oxide_fw_router::RouteTree,
    pub(crate) routes_root: std::path::PathBuf,
    pub(crate) stack: crate::framework_shim::FrameworkStack,
    // ---- phase_plan outputs ----
    pub(crate) entries: Option<EntryMap>,
    pub(crate) client_out: Option<std::path::PathBuf>,
    pub(crate) server_out: Option<std::path::PathBuf>,
    pub(crate) boot_chunk_url: Option<String>,
    pub(crate) server_precompiled: Option<std::collections::BTreeMap<std::path::PathBuf, String>>,
    pub(crate) build_secret_for_fp: Option<String>,
    pub(crate) css_transform: Option<oxide_fw_bundler::CssTransformHook>,
    pub(crate) client_opts: Option<oxide_fw_bundler::BundleOptions>,
    pub(crate) server_opts: Option<oxide_fw_bundler::BundleOptions>,
    pub(crate) rb: Option<oxide_fw_bundler::RolldownBundler>,
    pub(crate) transform_html_hook: Option<oxide_fw_bundler::TransformHtmlHook>,
    // ---- phase_bundle_client outputs ----
    pub(crate) client_outcome: Option<oxide_fw_bundler::BundleOutput>,
    // ---- phase_bundle_server outputs ----
    pub(crate) client_manifest: Option<serde_json::Value>,
    pub(crate) client_manifest_snapshot: Option<serde_json::Value>,
    pub(crate) boot_script_url: Option<String>,
    pub(crate) boot_script_integrity: Option<String>,
    pub(crate) manifest_integrity: Option<std::collections::BTreeMap<String, String>>,
    pub(crate) head_producer: Option<oxide_fw_bundler::head::ManifestHeadProducer>,
    pub(crate) server_manifest: Option<serde_json::Value>,
    pub(crate) build_id: Option<String>,
    // ---- phase_prerender outputs ----
    pub(crate) prerendered: Option<std::collections::BTreeMap<String, Vec<String>>>,
    pub(crate) seed_bodies: Option<Vec<SeedBody>>,
    pub(crate) static_override: Option<std::collections::BTreeSet<String>>,
    pub(crate) new_prerender_records: Option<Vec<crate::build_graph::RoutePrerenderRecord>>,
}
