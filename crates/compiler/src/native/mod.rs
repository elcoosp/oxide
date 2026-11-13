//! The native-compiler seam (spec v0.2, §V2-C01, D26/D40): any framework
//! compiles in-process when a native backend is compiled in and healthy,
//! the Node tooling pool stays the fallback, and an artifact is never
//! reused across implementations.
//!
//! # The seam
//!
//! [`NativeCompiler`] is a deliberately narrow contract — **string in,
//! string out**. No AST type may cross this boundary: each upstream
//! compiler crate pins its own `oxc` minor, and a shared AST type would
//! force one `oxc` version onto every backend (the C02 spike's reason for
//! vendoring). A backend names the framework it serves (`react | vue |
//! svelte | solid`), the extensions it handles, and the version string the
//! CAS key folds in ([`cas_tool_version`] — D40: switching
//! `compilerBackend` never reuses a stale artifact).
//!
//! [`NativeRegistry`] is the selection engine the precompile integration
//! consults per file:
//!
//! - [`Backend::Auto`] (the config default) picks a **healthy** compiled-in
//!   backend for `(framework, ext)` and answers [`None`] (use the Node
//!   pool) otherwise — a broken backend must never surface as a hard
//!   failure the config author did not ask for.
//! - [`Backend::Native`] is the hard request: no healthy backend refuses
//!   with `CMP-NAT-001` ([`native_err`]) instead of a silent fallback.
//! - [`Backend::Node`] always answers [`None`] — the one-release escape
//!   hatch (D26).
//!
//! The health probe ([`NativeCompiler::health_fixture`]) compiles a tiny
//! valid source once per backend and caches the verdict ([`OnceLock`]), so
//! a hot build loop never re-pays it; [`report`] is the `oxide doctor`
//! surface over the same verdicts.
//!
//! # What is compiled in today
//!
//! Nothing yet — by design. [`NativeRegistry::from_enabled_features`]
//! builds the registry from the backends compiled into the binary; the
//! real backends (Solid, Vue, Svelte) land with C02–C04, each behind its
//! own feature with its vendored upstream (D27), and register themselves
//! here. Until then every framework answers the Node pool under `auto`,
//! which is exactly the C01 contract the TDD pins hold
//! (`tests/native_registry_tdd.rs`, driven by an injected test backend —
//! the same shape a host crate uses). The integration (the CLI's
//! `compile_one` consulting `pick` before the pool, and `oxide doctor`
//! printing [`report`]) rides the A01-phase-3 CLI adoption, exactly like
//! the A02/A05/A06 substrates (see `docs/deviations.md`).

use crate::{OxideError, Result};
use std::path::Path;
use std::sync::OnceLock;

/// The compile target a request asks for (§V2-C01: the worker protocol's
/// `generate` argument — `dom` for the client bundle, `ssr` for the server
/// render module).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Generate {
    /// The client (DOM) bundle output.
    Dom,
    /// The server-render module output.
    Ssr,
}

/// One compile request: the source file verbatim plus the flags the
/// pipeline carries (§V2-C01 — synchronous, CPU bound; the caller wraps it
/// in `spawn_blocking`).
#[derive(Debug)]
pub struct CompileRequest<'a> {
    /// The source path (relative to the project root), for diagnostics and
    /// the output module's identity.
    pub path: &'a Path,
    /// The raw source text.
    pub source: &'a str,
    /// The compile target.
    pub generate: Generate,
    /// Dev mode (refresh boundaries, HMR ids) — the backend maps this onto
    /// its upstream's options.
    pub dev: bool,
}

/// One compile result: the emitted module text plus optional extracted CSS
/// and warnings (§V2-C01).
#[derive(Default, Debug)]
pub struct CompileOutput {
    /// The emitted JavaScript module.
    pub js: String,
    /// Extracted style block, when the framework's SFC format carries one.
    pub css: Option<String>,
    /// Non-fatal diagnostics (upstream warnings, passed through verbatim).
    pub warnings: Vec<String>,
}

/// A native framework compiler (§V2-C01): string in, string out.
///
/// Every upstream pins its own `oxc`, so no AST type crosses this
/// boundary — a backend parses and prints internally and hands back the
/// emitted module text. The trait is object safe: the registry stores
/// `Box<dyn NativeCompiler>`.
pub trait NativeCompiler: Send + Sync {
    /// The framework this backend serves: `react`, `vue`, `svelte` or
    /// `solid` (the `Framework::as_str` spelling).
    fn framework(&self) -> &'static str;
    /// The backend's stable id (`solid-native`, `vue-native`, ...) — the
    /// CAS key and the doctor line carry it.
    fn id(&self) -> &'static str;
    /// The upstream version or vendored revision — part of the CAS key
    /// (D40), so a backend upgrade invalidates its artifacts.
    fn version(&self) -> String;
    /// Whether this backend handles the file extension (`tsx`, `vue`, ...).
    fn handles(&self, ext: &str) -> bool;
    /// Compile one file. Synchronous and CPU bound — the caller owns the
    /// blocking-task placement.
    ///
    /// # Errors
    ///
    /// When the backend cannot compile the source (parse failure, an
    /// unsupported construct): the error carries the backend's diagnostic.
    fn compile(&self, req: &CompileRequest<'_>) -> Result<CompileOutput>;
    /// `(file name, tiny valid source)` the registry compiles once to
    /// probe backend health — a fixture that fails to produce non-empty
    /// output marks the backend unhealthy and [`Backend::Auto`] skips it.
    fn health_fixture(&self) -> (&'static str, &'static str);
}

/// The `framework.compilerBackend` selection mode (§V2-C01, D26). The
/// config crate carries the typed config key; this is the selection-side
/// copy the registry consumes — the two shapes are pinned by their own
/// crates and mapped at the call site (the recorded F04 ruling: config →
/// engine conversion is wiring, not a dependency).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Backend {
    /// Pick a healthy compiled-in native backend, else the Node pool (the
    /// config default).
    #[default]
    Auto,
    /// Require the native path — refuse with `CMP-NAT-001` when no healthy
    /// backend is available.
    Native,
    /// Always the Node tooling pool (the one-release escape hatch, D26).
    Node,
}

/// One `oxide doctor` row: the compiled-in backend's id, version and
/// health verdict (§V2-C01 — the doctor prints
/// `native compiler <id> <version> ok|unhealthy`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NativeReport {
    /// The backend id ([`NativeCompiler::id`]).
    pub id: String,
    /// The backend version ([`NativeCompiler::version`]).
    pub version: String,
    /// Whether the health fixture compiles (probed once, cached).
    pub healthy: bool,
}

/// One registered backend with its once-only health verdict.
struct Entry {
    backend: Box<dyn NativeCompiler>,
    healthy: OnceLock<bool>,
}

/// The native-compiler registry (§V2-C01): the compiled-in backends plus
/// the `auto`/`native`/`node` selection over them.
///
/// Backends enter through [`NativeRegistry::from_enabled_features`] (the
/// feature-gated compiled-in set) or [`NativeRegistry::register`] (the
/// injection seam a host crate or a test uses). Health is probed lazily —
/// at the first `auto`/`native` pick or [`NativeRegistry::report`] call —
/// and cached for the process lifetime.
pub struct NativeRegistry {
    items: Vec<Entry>,
}

impl NativeRegistry {
    /// The registry of backends compiled into this binary.
    ///
    /// No native backend is compiled in yet: C02–C04 land the real
    /// backends behind their own features (`native-solid`,
    /// `native-vue`, `native-svelte`) with their vendored upstreams
    /// (D27) and push them here. Until then the registry is empty —
    /// `auto` answers the Node pool for every framework and `native`
    /// refuses with [`native_err`], which is the C01 contract the TDD
    /// pins hold.
    #[must_use]
    pub fn from_enabled_features() -> Self {
        Self::empty()
    }

    /// An empty registry (the injection seam's starting point).
    #[must_use]
    pub fn empty() -> Self {
        Self { items: Vec::new() }
    }

    /// Register one backend (the injection seam a host crate — or the
    /// test fake — uses for a backend not gated behind a crate feature).
    pub fn register(&mut self, backend: Box<dyn NativeCompiler>) {
        self.items.push(Entry {
            backend,
            healthy: OnceLock::new(),
        });
    }

    /// The cached health verdict for backend `i` (compiled once).
    fn healthy(&self, i: usize) -> bool {
        let Some(entry) = self.items.get(i) else {
            return false;
        };
        *entry.healthy.get_or_init(|| {
            let (name, src) = entry.backend.health_fixture();
            let req = CompileRequest {
                path: Path::new(name),
                source: src,
                generate: Generate::Dom,
                dev: false,
            };
            matches!(entry.backend.compile(&req), Ok(o) if !o.js.is_empty())
        })
    }

    /// The selection engine (§V2-C01): `Ok(None)` means "use the Node
    /// pool", `Ok(Some)` means "compile in-process with this backend".
    ///
    /// `auto` consults the cached health verdict; `native` refuses when
    /// no healthy backend matches `(framework, ext)`; `node` never picks
    /// native.
    ///
    /// # Errors
    ///
    /// `CMP-NAT-001` ([`native_err`]) only for [`Backend::Native`] with no
    /// healthy matching backend — the config author's hard request cannot
    /// be honored.
    pub fn pick(
        &self,
        framework: &str,
        ext: &str,
        backend: Backend,
    ) -> Result<Option<&dyn NativeCompiler>> {
        if backend == Backend::Node {
            return Ok(None);
        }
        let found = self
            .items
            .iter()
            .position(|e| e.backend.framework() == framework && e.backend.handles(ext));
        let Some(i) = found else {
            return match backend {
                Backend::Native => Err(native_err(framework)),
                _ => Ok(None),
            };
        };
        if !self.healthy(i) {
            return match backend {
                Backend::Native => Err(native_err(framework)),
                _ => Ok(None),
            };
        }
        Ok(Some(self.items[i].backend.as_ref()))
    }

    /// The `oxide doctor` surface: one row per compiled-in backend
    /// (§V2-C01 — probing here warms the same cached verdicts `pick`
    /// reuses).
    #[must_use]
    pub fn report(&self) -> Vec<NativeReport> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, e)| NativeReport {
                id: e.backend.id().to_string(),
                version: e.backend.version(),
                healthy: self.healthy(i),
            })
            .collect()
    }

    /// How many backends are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether no backend is registered (the default build until C02–C04
    /// land their features).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The `CMP-NAT-001` refusal (§V2-C01): `compilerBackend: 'native'` but no
/// healthy native compiler for the framework. Public so the CLI adoption
/// surfaces the registry's identical refusal text and the errors registry
/// holds one row for one behavior.
///
/// # Examples
///
/// ```
/// use oxide_fw_compiler::native::native_err;
/// let e = native_err("vue");
/// assert_eq!(e.0.code, "CMP-NAT-001");
/// assert!(e.0.message.contains("vue"));
/// ```
#[must_use]
pub fn native_err(fw: &str) -> OxideError {
    let mut d = oxide_fw_diagnostics::Diagnostic::error(
        "CMP-NAT-001",
        format!(
            "framework.compilerBackend is 'native' but no healthy native compiler is available for {fw}"
        ),
    );
    d.fix = Some(format!(
        "build oxide-cli with --features native-{fw} or set framework.compilerBackend to 'auto' or 'node'"
    ));
    OxideError(Box::new(d))
}

/// The D40 CAS tool-version spelling for a native backend:
/// `native:{id}@{version}`. The precompile cache folds this into
/// `PrecompileKey::tool_version`, so switching `compilerBackend` (or
/// upgrading a backend) never reuses a stale artifact.
#[must_use]
pub fn cas_tool_version(compiler: &dyn NativeCompiler) -> String {
    format!("native:{}@{}", compiler.id(), compiler.version())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pong;
    impl NativeCompiler for Pong {
        fn framework(&self) -> &'static str {
            "vue"
        }
        fn id(&self) -> &'static str {
            "vue-native"
        }
        fn version(&self) -> String {
            "9.9.9".to_string()
        }
        fn handles(&self, ext: &str) -> bool {
            ext == "vue"
        }
        fn compile(&self, req: &CompileRequest<'_>) -> Result<CompileOutput> {
            Ok(CompileOutput {
                js: req.source.to_string(),
                css: None,
                warnings: Vec::new(),
            })
        }
        fn health_fixture(&self) -> (&'static str, &'static str) {
            ("fixture.vue", "<template />")
        }
    }

    #[test]
    fn empty_registry_is_the_default_build() {
        let reg = NativeRegistry::from_enabled_features();
        assert!(reg.is_empty());
        assert_eq!(reg.report(), Vec::new());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn register_then_pick_then_compile() {
        let mut reg = NativeRegistry::empty();
        reg.register(Box::new(Pong));
        let picked = reg.pick("vue", "vue", Backend::Auto).unwrap().unwrap();
        let out = picked
            .compile(&CompileRequest {
                path: Path::new("App.vue"),
                source: "// emitted",
                generate: Generate::Dom,
                dev: false,
            })
            .unwrap();
        assert_eq!(out.js, "// emitted");
    }

    #[test]
    fn node_beats_a_registered_backend() {
        let mut reg = NativeRegistry::empty();
        reg.register(Box::new(Pong));
        assert!(reg.pick("vue", "vue", Backend::Node).unwrap().is_none());
    }

    #[test]
    fn cas_version_shape() {
        assert_eq!(cas_tool_version(&Pong), "native:vue-native@9.9.9");
    }

    #[test]
    fn native_err_names_framework_and_fix() {
        let e = native_err("svelte");
        assert_eq!(e.0.code, "CMP-NAT-001");
        assert!(e.0.message.contains("svelte"));
        let fix = e.0.fix.clone().unwrap_or_default();
        assert!(fix.contains("--features native-svelte"));
        assert!(fix.contains("'auto'"));
    }
}
