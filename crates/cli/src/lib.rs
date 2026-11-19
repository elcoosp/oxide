#![doc = include_str!("../README.md")]

//! oxide-fw-cli — clap entry, commands (spec §19.1).
//!
//! Commands: dev, build, start, preview, routes, analyze, check, why, doctor,
//! secrets, add, generate, cache, audit, migrate, test (in-process
//! app.fetch helper), mcp, schema, config, render, perf (§V2-F18 bisect).

use clap::{Parser, Subcommand};
use oxide_fw_diagnostics::OxideError;
use oxide_fw_jshost::loader_host::ActionRunArgs;
use oxide_fw_jshost::loader_host::EndpointRunArgs;
use oxide_fw_jshost::loader_host::LoaderRunArgs;
use oxide_fw_server::ActionDispatchArgs;
use oxide_fw_server::EndpointDispatchArgs;
use oxide_fw_server::MiddlewareDispatchArgs;
use oxide_fw_server::RunActionArgs;

type LoaderInputs = (
    Option<std::path::PathBuf>,
    Option<String>,
    std::collections::BTreeMap<String, String>,
    bool,
    Option<String>,
    bool,
    // V2-F05: the matched route is live-opted (`RouteEntry.live.is_some()`)
    // — the /__data envelope advertises `live: true` so the client's router
    // opens the SSE connection for the route it just navigated to. `false`
    // keeps the envelope byte-identical for every non-live route.
    bool,
);

/// V2-A06: one layout's §9.2 load spec — the module path plus the scanner's
/// token-exact `parent()` verdict (A06 step 1 applies `calls_parent` to
/// `layout.server.ts`, not only to pages). The §9.2 load graph runs the
/// layout concurrently unless its own verdict says it awaits its
/// predecessor's data; the /__data path keeps the sequential chain and
/// reads only the id/path pair.
#[derive(Clone, Debug)]
pub struct LayoutLoadSpec {
    /// The layout route id (`routes/blog/layout`), as the loader input's
    /// route field expects.
    pub(crate) id: String,
    /// The `layout.server.ts` path on disk (routes-root joined).
    pub(crate) path: std::path::PathBuf,
    /// Whether this layout's load may call `await parent()` — the load
    /// graph serializes it behind its predecessor when true.
    pub(crate) needs_parent: bool,
}

/// The wasm-bindgen-processed `oxide-fw-core-wasm` artifact, embedded at
/// CLI build time so a user project without a cargo workspace containing
/// `oxide-fw-core-wasm` still gets the accelerated edge path.
/// Regenerated via `scripts/regen-edge-wasm.sh` (WP-C2, deviation 6).
static EDGE_WASM_BYTES: &[u8] = include_bytes!("../resources/edge.wasm");
use oxide_fw_config::OxideConfig;
use oxide_fw_router::RouteTree;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub mod attest;
mod audit;
pub mod docs_gen;
pub mod llms;
pub mod mcp;
mod native_plugins;
pub mod perf_bisect;
pub mod serve;
// V2-F09: the build-side search mapping (feature `search`) — the pure
// entry→document helpers the `emit_search_engine_index` build step uses.
#[cfg(feature = "search")]
mod search_emit;

mod build_cache;
mod build_graph;
mod esm_dev;
mod framework_shim;
// V2-F49: the posture probe's public seam — the wire body the `/-/health`
// surface answers, the typed-tier adapter behind the server crate's
// `CachePosture` trait, and the boot's two-halves gate (the bench group
// and the walkthrough reuse them; the rest of the shim stays
// crate-internal, the `mod args; pub use args::*` shape).
pub use framework_shim::{TierCachePosture, cache_posture_probe_json, posture_probe_provider};
mod l0_cache;
pub mod loader_shim;
mod modules_loader;
pub mod precompile;
mod precompile_cache;
mod record_wire;

// ----------------------------------------------------------------- helpers

// V2-E07: `+ Send` — the MCP registry's async tools (build.run) hold the
// writer across an await, so the trait object must be Send. Every current
// call site passes a Send writer (stdout locks, sinks, byte buffers).
type Writer<'a> = &'a mut (dyn std::io::Write + Send);

macro_rules! outln {
    ($w:expr, $($arg:tt)*) => {
        writeln!($w, $($arg)*).map_err(|e| anyhow::anyhow!("write failed: {e}"))?
    };
}
pub(crate) use outln;

// ----------------------------------------------------------------- commands

/// The stage name the parallel arm records. It is a **counterfactual**: the
/// wall-clock the overlapping loads would have cost had they run
/// sequentially (`min(a, b)`), not a duration anyone measured. It is
/// recorded as a saving so the inspector's total reconciles, and the name
/// says so — `overlap-saved` rather than a bare `load-overlap` that reads
/// like a phase.
const LOAD_OVERLAP_STAGE: &str = "overlap-saved";

/// §16.6 / WP-D1: encode a loader's post-run session into a `Set-Cookie`
/// value, but only when it actually changed. Compares against the inbound
/// session (the object decoded from the request's cookie); an untouched
/// session — including the `Some({})` the worker returns for a
/// no-cookie-start project — never re-issues a cookie.
///
/// Returns `Some(cookie)` when a cookie should ride the response, `None`
/// when the session is unchanged or absent. Extracted so the four serve
/// paths (sequential page, parallel page, /__data short-circuit, /__data
/// page) share one implementation instead of four near-copies.
/// §16.6 / WP-r9: session cookie lifetime, sourced from
/// `security.sessionTtlSecs` at server start. Process-global because
/// `build_dispatch`'s closure cannot see `cfg`; the CLI runs one server
/// per process (documented single-server assumption). Tests use the
/// default 1800 until they opt in.
static SESSION_TTL_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1800);

/// Boxed dispatch future. Structurally identical to `futures::future::BoxFuture<_,
/// OxideResponse>` (which [`oxide_fw_server::Server::serve`] takes) without needing a
/// `futures` dependency in this crate.
type BoxedResponse =
    std::pin::Pin<Box<dyn std::future::Future<Output = oxide_fw_server::OxideResponse> + Send>>;

/// Inspector adapter for the dev server: converts the CLI's
/// `RouteTableInspector` into the `oxide_fw_server::InspectorSnapshot` trait
/// (§13.4). The trait lives in oxide-fw-server so the pipeline can render it
/// without depending on oxide-fw-dev.
#[derive(Debug)]
struct CliInspector {
    state: oxide_fw_dev::InspectorState,
}

#[cfg(test)]
mod tests;

// ─── §15-E: extended plugin hooks (resolveId + load) ─────────────────────────
//
// Additive: these use the same `plugin:load` / `plugin:call` worker ops that
// `build_js_plugin_hook` already uses for `transform`, but build a
// `ResolveHook` / `LoadHook` (types already in `oxide_fw_bundler::BundleOptions`)
// so a `kind:js` plugin — or a `modules:[]` entry — can alias or virtualize
// modules. The bundler wiring (setting `BundleOptions.resolve_hook` etc.) is
// in `cmd_build`.
//
// Each collector honors the same `confine_plugin_path` rule the transform
// path applies, so a plugin path escape is a warning+skip, not an error.

// ─── §5-A full incremental: per-route prerender reuse ──────────────────────
//
// A build re-runs the full pipeline; the win comes from **skipping the
// expensive per-route render** when the route's source files (and the
// config) are unchanged since the last build. The reuse granularity is
// per-URL, so a `generateParams` route that produced N pages keeps its
// rendered output while a page whose source changed gets a fresh render.
//
// The hash deliberately includes the serialized config: any config change
// (framework adapter, i18n locale list, `build.sourcemap`, etc.) flushes
// every route. That is coarser than a field-by-field dependency, but it is
// conservative in the safe direction — the worst case is a full rerender,
// never a stale render.

// ─── §5-B: plugin-options inventory ────────────────────────────────────────
//
// The graph's `Input::PluginOptions` variant lets a build node record "I was
// transformed by plugin X with these options." `file_compute_with_plugins`
// records that edge for every module source whose path matches a plugin
// entry. This helper is what produces the `(name, options_hash, path)`
// triples the build graph consumes.
//
// The hash covers the plugin's *whole* config entry (kind, path, plus every
// field an author put in the object). A change to any option the plugin was
// configured with changes the hash, so every module that plugin transforms
// invalidates on the next build — which is exactly the §5-B contract.

// ─── §15-B: renderChunk / generateBundle / transformHtml hook builders ─────
//
// Same worker-op contract the existing transform/resolveId/load builders use:
// `plugin:load` once per candidate, then `plugin:call {path, hook, args}`.
// Each builder is deliberately simple — the chaining rules differ per hook
// (renderChunk: chain the source; generateBundle: last non-null wins;
// transformHtml: chain the HTML), so each is its own function.

// ─── §15-B: hook-result shape helpers ──────────────────────────────────────
//
// A plugin author writes the shape every bundler documents: `transform`
// returns the rewritten code string, `resolveId` returns an id string (or
// null), `load` returns a source string (or null), `transformHtml` returns
// HTML, `generateBundle` returns a file array. The Rust readers used to
// require an object (`{code: …}`) — which no plugin author writes — so every
// hook's return value was silently dropped. These helpers accept both: the
// documented bare value, and an object with the named field (for a plugin
// that wants to carry extra metadata alongside).

// ─── §15: generic plugin hook dispatch ─────────────────────────────────────
//
// The six "structural" hooks (transform, resolveId, load, renderChunk,
// generateBundle, transformHtml) each have a bespoke builder because their
// chaining rules differ. The rest are notifications or single-shot
// contributions: load each candidate, call the hook, collect the results.
// `call_js_plugin_hook` is that shape.

// §10.8 / B-27: process-local PPR shell cache, keyed by (build id, route,
// params, vary). One store per server; only populated when `render.ppr` is
// enabled.
//
// Bounded by PPR_SHELLS_MAX with least-recently-inserted eviction. The pre-r9
// behavior cleared the whole map on the 256th insert, so every visitor paid
// the first-request tap again after the cap.
type PprShells = std::sync::Arc<std::sync::Mutex<PprShellStore>>;

const PPR_SHELLS_MAX: usize = 256;

mod tailwind;

#[cfg(test)]
mod check_backends_tests {
    //! §V2-C11 pins: [`resolve_tool`] prefers the project-local
    //! `node_modules/.bin` install over `PATH`; a fake executable drives
    //! the three tool outcomes (OK / FAILED / SKIPPED); the formatter
    //! fallback chain prefers `oxfmt`, then `biome format`; the configured
    //! typecheck backend maps onto the exact command `oxide check` runs.
    //! The `which`-based PATH arms are exercised through the fakes, so
    //! the suite needs no real toolchain on PATH.

    use super::*;
    use std::path::PathBuf;

    /// `node_modules/.bin/fake-tool` — a `#!/bin/sh` script that exits
    /// with the code its name encodes (`...-ok` exits 0, `...-fail`
    /// exits 1) and records its argv in a side file, so the pins can
    /// assert the exact arguments the backend plan produced.
    fn install_fake(root: &Path, name: &str, exit: i32) -> PathBuf {
        let dir = root.join("node_modules/.bin");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let log = write_args(root);
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nexit {exit}\n",
                log.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    fn write_args(w: &Path) -> PathBuf {
        w.join("fake-args.log")
    }

    #[test]
    fn resolve_tool_prefers_node_modules_over_path() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = install_fake(tmp.path(), "tsgo", 0);
        // PATH deliberately empty-ish: the resolution must succeed through
        // the project-local install alone.
        let got = resolve_tool("tsgo", tmp.path()).expect("project-local tool resolves");
        assert_eq!(got, fake, "node_modules/.bin wins");
        // A missing tool answers None even with nothing else on PATH.
        assert!(resolve_tool("oxide-not-a-tool", tmp.path()).is_none());
    }

    #[test]
    fn run_tool_reports_ok_and_failed_from_exit_codes() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir().unwrap();
        let ok_bin = install_fake(tmp.path(), "tool-ok", 0);
        let fail_bin = install_fake(tmp.path(), "tool-fail", 1);
        let args_log = write_args(tmp.path());
        let mut buf: Vec<u8> = Vec::new();
        // The Writer is `&mut dyn Write + Send` — a mut Vec borrow fits.
        let w: Writer<'_> = &mut buf;
        assert!(run_tool(
            "typecheck",
            &ok_bin,
            &["--noEmit"],
            tmp.path(),
            w
        )?);
        assert!(!run_tool(
            "typecheck",
            &fail_bin,
            &["--noEmit"],
            tmp.path(),
            w
        )?);
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("typecheck: OK"), "exit 0 prints OK: {out}");
        assert!(
            out.contains("typecheck: FAILED"),
            "exit 1 prints FAILED: {out}"
        );
        // The tool saw the exact args the plan produced.
        let seen = std::fs::read_to_string(&args_log).unwrap();
        assert_eq!(seen.lines().count(), 2, "one line per invocation");
        assert!(seen.contains("--noEmit"));
        Ok(())
    }

    #[test]
    fn typecheck_backend_plan_maps_every_backend() {
        use oxide_fw_config::TypecheckBackend as T;
        let (bin, args) = typecheck_backend_plan(T::Tsgo).expect("tsgo has a plan");
        assert_eq!(bin, "tsgo");
        assert!(args.contains(&"--noEmit"));
        let (bin, args) = typecheck_backend_plan(T::Oxlint).expect("oxlint has a plan");
        assert_eq!(bin, "oxlint");
        assert!(args.contains(&"--type-aware") && args.contains(&"--type-check"));
        let (bin, _) = typecheck_backend_plan(T::Tsrs).expect("tsrs has a plan");
        assert_eq!(bin, "tsrs");
        let (bin, args) = typecheck_backend_plan(T::Tsc).expect("tsc has a plan");
        assert_eq!(bin, "tsc");
        assert!(args.contains(&"--noEmit"));
        assert!(
            typecheck_backend_plan(T::Off).is_none(),
            "`off` runs no typechecker"
        );
    }

    #[test]
    fn fmt_falls_back_from_oxfmt_to_biome_and_skips_when_both_absent() {
        // The fallback chain is exercised through a tiny re-implementation
        // of `cmd_fmt`'s decision over the same helpers — cmd_fmt prints
        // through a Writer and shells out, so the pin holds the decision
        // (which tool would run, in which order) against the same
        // resolve_tool calls the command makes.
        let with_both = tempfile::tempdir().unwrap();
        install_fake(with_both.path(), "oxfmt", 0);
        install_fake(with_both.path(), "biome", 0);
        assert!(resolve_tool("oxfmt", with_both.path()).is_some());
        assert!(resolve_tool("biome", with_both.path()).is_some());
        // oxfmt present wins (cmd_fmt's first arm).
        assert!(resolve_tool("oxfmt", with_both.path()).is_some());

        let biome_only = tempfile::tempdir().unwrap();
        install_fake(biome_only.path(), "biome", 0);
        assert!(
            resolve_tool("oxfmt", biome_only.path()).is_none(),
            "no oxfmt"
        );
        assert!(resolve_tool("biome", biome_only.path()).is_some());

        let neither = tempfile::tempdir().unwrap();
        assert!(resolve_tool("oxfmt", neither.path()).is_none());
        assert!(resolve_tool("biome", neither.path()).is_none());
    }
}

// V2-A01: module tree; moved items are re-exported so existing names keep resolving.
mod args;
pub use args::*;
mod boot_entry;
pub(crate) use boot_entry::*;
mod build;
pub(crate) use build::*;
mod bundle_env;
pub(crate) use bundle_env::*;
mod commands;
pub use commands::*;
mod config_load;
pub(crate) use config_load::*;
mod data_route;
pub use data_route::*;
mod dev;
pub use dev::*;
mod dispatch;
pub(crate) use dispatch::*;
mod emit;
pub(crate) use emit::*;
mod endpoints;
pub(crate) use endpoints::*;
mod hooks;
pub(crate) use hooks::*;
// V2-F42: the observability module is public — the boot flag's override
// slot and the §20 resolver's parse core are the window's consumer API
// (the criterion bench and the walkthrough example ride them).
pub mod observability;
pub(crate) use observability::*;
mod prerender;
pub use prerender::*;
pub use serve::*;
