//! §V2-F08 finish — the CLI adoption legs the WP row names as remaining:
//! the consult adapter that holds the LIVE host (the dev loop swaps under
//! it), the production `dist/native/<lib>` emission, and the bundle-time
//! `.server.rs` import resolution (the stub generator's output rides the
//! bundler's resolve/load hooks).
//!
//! The substrate layers are done and pinned elsewhere: `oxide-rustfn`
//! (the C ABI host, the loader, the id-mapped registry), `oxide-fw-dev`
//! (the watch → build → load → swap loop over the injectable seams), the
//! server crate's consult seam ([`crate::oxide_fw_server::NativeFnSource`]
//! + `RustFirstDispatcher`). This module is the ONE definition site for
//! how the CLI wires them:
//!
//! - **Discovery** ([`discover_native_crate`]): the §V2-F08 crate layout —
//!   a `Cargo.toml` whose `[lib] crate-type` carries `"cdylib"` — at the
//!   app root or one level beside it. A dependency-tree crate is never a
//!   first-party fact.
//! - **The live consult** ([`swappable_native_fn_source`] /
//!   [`dev_native_consult_with`]): the consult holds the
//!   [`SwappableHost`] the loop swaps UNDER, so a rebuilt `*.server.rs`
//!   answers its new behavior on the next request without a restart.
//! - **Production emission** ([`emit_native_lib`]): `phase_native` builds
//!   the crate in release, copies the cdylib to `dist/native/` beside the
//!   types payload, and leaves the mount copy under `.oxide/rust/` for the
//!   serve boot walk. `crates/cache` covers `dist/native` in the A04
//!   build id, so a rebuilt library is a new build identity.
//! - **Bundle-time stubs** ([`server_rs_resolve_hook`] /
//!   [`server_rs_load_hook`]): `import … from './echo.server'` resolves to
//!   the module file and loads the generated stub — the same
//!   `rust_fn_stub_source` bytes the `.server.ts` transform produces, so
//!   the client cannot tell a native fn from a JS one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use oxide_fw_bundler::{LoadHook, ResolveHook};
use oxide_fw_compiler::{Env as CompilerEnv, rust_fn_stub_source};
use oxide_fw_dev::server_lib::{
    CargoLibBuild, CargoProfile, HostSwapper, LibBuild, LibLoad, ServerLibSpec,
};
use oxide_fw_diagnostics::OxideError;
use oxide_fw_server::NativeFnSource;
use oxide_rustfn::host::RustFnLibrary;
use oxide_rustfn::swap::SwappableHost;
use oxide_rustfn::types::TypesPayload;

use crate::build_secret;

/// The bundle-time hook arms apply to `*.server.rs` modules only.
const SERVER_RS_SUFFIX: &str = ".server.rs";

/// `RFN-TYPES`: the native crate's types payload did not parse.
const RFN_TYPES: &str = "RFN-TYPES";

/// `RFN-LOAD`: a native library failed to load or the secret is missing.
const RFN_LOAD: &str = "RFN-LOAD";

/// `RFN-005`: `.server.rs` sources are present but no cdylib crate is
/// discoverable — the functions would 404 at runtime, so the build
/// refuses instead of emitting stubs with no server behind them.
pub(crate) const RFN_NO_CRATE: &str = "RFN-005";

/// The `RFN-005` registry row (docs/errors.toml carries the message).
pub(crate) fn rfn_no_crate_error(detail: String) -> OxideError {
    OxideError::new(RFN_NO_CRATE, detail)
}

/// One discovered native server-fn crate: the `ServerLibSpec` inputs plus
/// the directory the bundle-time hooks map module paths against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCrate {
    /// The manifest's `[package] name` (the loop's log label).
    pub name: String,
    /// The crate root (the directory holding the `Cargo.toml`).
    pub dir: PathBuf,
}

/// The directories the discovery walk never enters (dependency and build
/// output trees — the same skip set `has_rust_fn_sources` applies).
const DISCOVERY_SKIP: [&str; 6] = ["node_modules", "target", "dist", ".git", ".oxide", "vendor"];

/// The first-party `*.server.rs` source a crate-less project carries, for
/// the [`RFN_NO_CRATE`] refusal's message. `None` when the walk finds none
/// (the same skip set the discovery walk applies).
pub(crate) fn first_server_rs_source(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let skip = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| DISCOVERY_SKIP.contains(&n));
                if !skip {
                    stack.push(p);
                }
            } else if p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(SERVER_RS_SUFFIX))
            {
                return Some(p);
            }
        }
    }
    None
}

/// Whether a `Cargo.toml` declares the §V2-F08 cdylib shape: a `[lib]`
/// section whose `crate-type` list carries `"cdylib"`. The scan is the
/// minimal line walk the two shapes need (the manifest is machine-written
/// by `cargo`/the scaffold, not a config surface).
fn manifest_declares_cdylib(text: &str) -> bool {
    let mut in_lib = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_lib = trimmed == "[lib]";
            continue;
        }
        if !in_lib {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("crate-type")
            && rest.trim_start().starts_with('=')
            && rest.contains("cdylib")
        {
            return true;
        }
    }
    false
}

/// The `[package] name` of a `Cargo.toml` (the discovery label), falling
/// back to the crate directory's file name.
fn manifest_package_name(text: &str, dir: &Path) -> String {
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("name")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            let name = value.trim().trim_matches('"').trim_matches('\'');
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    dir.file_name().map_or_else(
        || "native-fns".to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Discover the project's native server-fn crate: a `Cargo.toml` whose
/// `[lib] crate-type` carries `"cdylib"`, at the app root (the spec's
/// `src/thumb.server.rs` layout) or one level beside it (the scaffold's
/// `native/` sibling crate). Dependency and build-output trees are never
/// entered, so a vendored cdylib is not a first-party fact.
#[must_use]
pub fn discover_native_crate(root: &Path) -> Option<NativeCrate> {
    let mut candidates = vec![root.join("Cargo.toml")];
    if let Ok(rd) = std::fs::read_dir(root) {
        let mut subdirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| DISCOVERY_SKIP.contains(&n))
            })
            .collect();
        subdirs.sort();
        candidates.extend(subdirs.into_iter().map(|d| d.join("Cargo.toml")));
    }
    for manifest in candidates {
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        if manifest_declares_cdylib(&text) {
            let dir = manifest
                .parent()
                .map_or_else(|| root.to_path_buf(), Path::to_path_buf);
            return Some(NativeCrate {
                name: manifest_package_name(&text, &dir),
                dir,
            });
        }
    }
    None
}

/// The build-path consistency guard ([`RFN_NO_CRATE`]): `*.server.rs`
/// sources under the app root with no discoverable cdylib crate refuse
/// the build — the stubs would import ids nothing answers.
///
/// # Errors
///
/// [`RFN_NO_CRATE`] naming the first source, when the crate is missing.
pub fn ensure_native_crate_consistency(root: &Path) -> Result<(), OxideError> {
    if crate::has_rust_fn_sources(root) && discover_native_crate(root).is_none() {
        let source = first_server_rs_source(root).map_or_else(
            || "a `*.server.rs` module".to_string(),
            |p| p.display().to_string(),
        );
        return Err(rfn_no_crate_error(format!(
            "`{source}` is present but no cdylib crate is discoverable — scaffold the native crate (a `Cargo.toml` with `[lib] crate-type = [\"cdylib\"]`) beside the app or remove the server-fn sources"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The consult source
// ---------------------------------------------------------------------------

/// The consult's native half: an `oxide-rustfn` registry behind the server
/// crate's [`NativeFnSource`] seam (the registry error kinds map onto the
/// same envelope shapes the JS pool produces). The registry is a trait
/// object so the SAME source object answers across a dev-loop swap — the
/// dispatcher never rebuilds it.
pub(crate) struct RegistryFnSource {
    registry: Arc<dyn oxide_rustfn::RustFnRegistry>,
}

impl RegistryFnSource {
    /// Wrap any registry (the boot [`oxide_rustfn::RustFnHost`], the dev
    /// [`SwappableHost`]) into the consult seam.
    pub(crate) fn new(registry: Arc<dyn oxide_rustfn::RustFnRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl NativeFnSource for RegistryFnSource {
    fn has(&self, id: &str) -> bool {
        self.registry.has(id)
    }

    async fn call(
        &self,
        id: &str,
        args_json: &[u8],
    ) -> Result<Vec<u8>, oxide_fw_server::DispatchError> {
        self.registry
            .call(id, args_json)
            .await
            .map_err(|e| oxide_fw_server::DispatchError {
                status: e.status(),
                code: e.envelope_code().into(),
                message: e.message,
            })
    }
}

/// The consult source over a dev-loop host: the SAME `Arc` the
/// [`HostSwapper`] swaps under, so a swap lands without touching the
/// dispatcher. Unlike [`swappable_native_fn_source`] an empty host still
/// yields a source — the dev session starts before the first swap, and
/// `has()` answers `false` until the loop mounts (the JS-pool fallback
/// per id, the mount walk's registered-0 convention applied per id).
pub(crate) fn live_host_source(host: Arc<SwappableHost>) -> Arc<dyn NativeFnSource> {
    Arc::new(RegistryFnSource::new(host))
}

/// The consult source over a boot-mounted host. `None` when the host is
/// empty: the consult degrades to the pure JS pool (the boot mount walk's
/// registered-0 convention).
#[must_use]
pub fn swappable_native_fn_source(host: Arc<SwappableHost>) -> Option<Arc<dyn NativeFnSource>> {
    if host.is_empty() {
        None
    } else {
        Some(live_host_source(host))
    }
}

/// The boot mount walk (the pre-finish shape, unchanged): load every
/// library under `<root>/.oxide/rust` into a fresh [`RustFnHost`] and
/// reconstruct the §9.4 ids off each library's types payload. `None` when
/// the directory is absent, unreadable or mounts nothing.
pub(crate) fn boot_mounted_native_fn_source(rootp: &Path) -> Option<Arc<dyn NativeFnSource>> {
    let mut host = oxide_rustfn::RustFnHost::new();
    let mut registered = 0usize;
    if let Some(secret) = mount_secret(rootp) {
        let rust_dir = rootp.join(".oxide/rust");
        if rust_dir.is_dir() {
            register_dir_libraries(&mut host, &rust_dir, &secret, &mut registered);
        }
    }
    if registered == 0 {
        None
    } else {
        Some(Arc::new(RegistryFnSource::new(Arc::new(host))))
    }
}

/// The build secret the boot mount hashes ids with. A missing secret is a
/// loud warn (the mount walk degrades to the JS pool, `RFN-LOAD`), not a
/// refusal — a serve must start.
fn mount_secret(rootp: &Path) -> Option<String> {
    match build_secret(rootp) {
        Ok(secret) => Some(secret),
        Err(e) => {
            tracing::warn!(
                code = RFN_LOAD,
                error = %e,
                "the build secret is unavailable; the native server functions stay unmounted"
            );
            None
        }
    }
}

/// Mount every library file under `dir` into `host`, registering each
/// payload export under its §9.4 id. A broken library degrades
/// loud-but-not-fatal (the `RFN-LOAD` warn names it; the rest mount).
fn register_dir_libraries(
    host: &mut oxide_rustfn::RustFnHost,
    dir: &Path,
    secret: &str,
    registered: &mut usize,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                code = RFN_LOAD,
                dir = %dir.display(),
                error = %e,
                "the native function library directory is unreadable"
            );
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "so" | "dll" | "dylib") {
            continue;
        }
        mount_one_library(host, &path, dir, secret, registered);
    }
}

/// Mount one library and register its payload's exports.
fn mount_one_library(
    host: &mut oxide_rustfn::RustFnHost,
    path: &Path,
    rust_dir: &Path,
    secret: &str,
    registered: &mut usize,
) {
    let lib = match RustFnLibrary::load(path, rust_dir) {
        Ok(lib) => lib,
        Err(e) => {
            tracing::warn!(
                code = oxide_rustfn::host::RFN_LOAD,
                path = %path.display(),
                error = %e.0.message,
                "native function library failed to mount"
            );
            return;
        }
    };
    let index = host.mount(lib);
    let Some(types) = host.types_json() else {
        return;
    };
    match serde_json::from_str::<TypesPayload>(&types) {
        Ok(payload) => {
            for f in &payload.fns {
                let id = oxide_fw_compiler::server_fn_id(&f.file, &f.name, secret);
                host.register(id, index, &f.name);
                *registered += 1;
            }
        }
        Err(e) => {
            tracing::warn!(
                code = RFN_TYPES,
                path = %path.display(),
                error = %e,
                "the native function types payload did not parse; its exports stay unrouted"
            );
        }
    }
}

/// Build the server-fn dispatcher over the caller-held native source (the
/// dev loop's live host) or, when `None`, the boot mount walk — the
/// pre-finish shape. `None` overall when neither half mounts (the
/// pipeline answers `/_oxide/fn/*` with the clean `FN-NO-DISPATCHER` 404).
pub fn build_server_fn_dispatcher_with(
    rootp: &Path,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
    native: Option<Arc<dyn NativeFnSource>>,
) -> Option<Arc<dyn oxide_fw_server::ServerFnDispatcher>> {
    let native = native.or_else(|| boot_mounted_native_fn_source(rootp));
    let js = crate::build_js_server_fn_dispatcher(rootp, pool);
    if native.is_none() && js.is_none() {
        return None;
    }
    Some(Arc::new(oxide_fw_server::RustFirstDispatcher::new(
        native, js,
    )))
}

// ---------------------------------------------------------------------------
// The production emission
// ---------------------------------------------------------------------------

/// What one emission pass produced: the built cdylib (already copied into
/// the mount dir) and the crate's types payload.
pub struct NativeEmit {
    /// The mounted cdylib copy (the bytes `dist/native` deploys).
    pub cdylib: PathBuf,
    /// The library's `oxide_fn_types_json` payload.
    pub types_json: String,
}

/// The emission source seam: build the crate and hand back the mounted
/// cdylib + the types payload. [`CargoEmitSource`] is production (a real
/// release `cargo build --lib`); fakes serve the fast pins, the
/// `#[ignore]` real-cargo pins live with the dev crate's fixture.
pub trait NativeEmitSource: Send + Sync {
    /// Run the build-and-mount pass.
    ///
    /// # Errors
    ///
    /// The coded failures ride through: `DEV-RFN-001` (the build),
    /// `RFN-001`/`RFN-003` (the mount gates), `DEV-RFN-002` (no types
    /// side export).
    fn emit(&self, spec: &ServerLibSpec, rootp: &Path) -> Result<NativeEmit, OxideError>;
}

/// The production emission source: `cargo build --lib --release` in the
/// crate dir, then the host's copy-then-load into `.oxide/rust` (the
/// serve boot walk finds the copy), then the types payload read.
pub struct CargoEmitSource;

impl NativeEmitSource for CargoEmitSource {
    fn emit(&self, spec: &ServerLibSpec, rootp: &Path) -> Result<NativeEmit, OxideError> {
        let out = CargoLibBuild::for_profile(CargoProfile::Release).build(spec)?;
        let rust_dir = rootp.join(".oxide/rust");
        let lib = RustFnLibrary::load(&out.cdylib_path, &rust_dir)?;
        let Some(types) = lib.types_json().map(str::to_owned) else {
            return Err(OxideError::new(
                "DEV-RFN-002",
                format!(
                    "the built library of crate {:?} carries no types side export; rebuild with the `export_table!` macro's `oxide_fn_types_json` export",
                    spec.name
                ),
            ));
        };
        Ok(NativeEmit {
            cdylib: lib.path().to_path_buf(),
            types_json: types,
        })
    }
}

/// What one [`emit_native_lib`] wrote, for the build log and the pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitReport {
    /// The deployed cdylib (`dist/native/lib-1.<ext>`).
    pub lib: PathBuf,
    /// The exports the payload carries.
    pub fn_count: usize,
    /// The cdylib byte count (the build-id covered input).
    pub bytes: u64,
}

/// The production emission (§V2-F08): copy the built cdylib to
/// `dist/native/lib-1.<ext>` beside the types payload
/// (`dist/native/types.json`), and write the payload to
/// `.oxide/rust/types.json` for the bundle-time stub hooks. The mount
/// copy the source produced stays for the serve boot walk.
///
/// # Errors
///
/// The source's coded failures ride through; a write failure is an
/// `RFN-LOAD`-coded `OxideError` (the build fails, it never deploys a
/// half-written native dir).
pub fn emit_native_lib(
    rootp: &Path,
    dist: &Path,
    spec: &ServerLibSpec,
    src: &dyn NativeEmitSource,
) -> Result<EmitReport, OxideError> {
    let emitted = src.emit(spec, rootp)?;
    let ext = emitted
        .cdylib
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("so");
    let bytes = std::fs::read(&emitted.cdylib).map_err(|e| {
        OxideError::new(
            RFN_LOAD,
            format!(
                "cannot read the mounted cdylib {}: {e}",
                emitted.cdylib.display()
            ),
        )
    })?;
    let native_dir = dist.join("native");
    std::fs::create_dir_all(&native_dir).map_err(|e| {
        OxideError::new(
            RFN_LOAD,
            format!("cannot create {}: {e}", native_dir.display()),
        )
    })?;
    let lib = native_dir.join(format!("lib-1.{ext}"));
    std::fs::write(&lib, &bytes)
        .map_err(|e| OxideError::new(RFN_LOAD, format!("cannot write {}: {e}", lib.display())))?;
    let payload = emitted.types_json.as_bytes();
    write_payload(&native_dir.join("types.json"), payload)?;
    write_payload(&rootp.join(".oxide/rust/types.json"), payload)?;
    let fn_count = serde_json::from_str::<TypesPayload>(&emitted.types_json)
        .map(|p| p.fns.len())
        .unwrap_or(0);
    Ok(EmitReport {
        lib,
        fn_count,
        bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
    })
}

/// One payload write site (the two sites carry identical bytes; the
/// bundle-time hooks read the `.oxide/rust` one, the deploy artifact the
/// `dist/native` one).
fn write_payload(path: &Path, payload: &[u8]) -> Result<(), OxideError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            OxideError::new(RFN_LOAD, format!("cannot create {}: {e}", parent.display()))
        })?;
    }
    std::fs::write(path, payload)
        .map_err(|e| OxideError::new(RFN_LOAD, format!("cannot write {}: {e}", path.display())))
}

// ---------------------------------------------------------------------------
// The bundle-time stub hooks
// ---------------------------------------------------------------------------

/// The bundle-time resolve arm (§V2-F08): `./echo.server` (and the
/// explicit `./echo.server.rs` spelling) resolves to the module file, so
/// the built-in resolver's missing-`.rs` extension never fires. A
/// specifier that does not name a first-party `.server.rs` module falls
/// through (`None`).
#[must_use]
pub fn server_rs_resolve_hook(nc: &NativeCrate) -> Option<ResolveHook> {
    let crate_dir = nc.dir.clone();
    Some(ResolveHook::new(move |specifier: &str, importer: &str| {
        Ok(resolve_server_rs(specifier, importer, &crate_dir))
    }))
}

/// Resolve one specifier to an existing `*.server.rs` module file under
/// the crate, or `None` (fall through to the next hook in the chain).
fn resolve_server_rs(specifier: &str, importer: &str, crate_dir: &Path) -> Option<PathBuf> {
    if !specifier.starts_with('.') || importer.is_empty() {
        return None;
    }
    let target = if specifier.ends_with(SERVER_RS_SUFFIX) {
        specifier.to_string()
    } else if specifier.ends_with(".server") {
        format!("{specifier}.rs")
    } else {
        return None;
    };
    let importer_dir = Path::new(importer)
        .parent()
        .map_or_else(PathBuf::new, Path::to_path_buf);
    let candidate = normalize_path(&importer_dir.join(&target));
    if candidate.starts_with(crate_dir) && candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// Lexical normalization (no filesystem access): collapse `.` / `..` so a
/// specifier like `../native/src/echo.server.rs` lands inside the crate
/// dir check.
fn normalize_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The bundle-time load arm (§V2-F08): a `.server.rs` id loads the
/// generated stub — the exact [`rust_fn_stub_source`] bytes, so the §9.4
/// id the consult answers is what the client calls. The exports come from
/// the crate's payload (`dist/native/types.json` /
/// `.oxide/rust/types.json` — one write shape, read once per bundle); a
/// missing or unparsable payload degrades loud (`RFN-TYPES`) to the
/// header-only module, whose missing-import error names the real gap.
#[must_use]
pub fn server_rs_load_hook(
    rootp: &Path,
    nc: &NativeCrate,
    secret: &str,
    env: CompilerEnv,
) -> Option<LoadHook> {
    let crate_dir = nc.dir.clone();
    let secret = secret.to_string();
    let payload_path = payload_file(rootp);
    let payload: Arc<OnceLock<Option<String>>> = Arc::new(OnceLock::new());
    Some(LoadHook::new(move |id: &str| {
        if !id.ends_with(SERVER_RS_SUFFIX) {
            return Ok(None);
        }
        let path = Path::new(id);
        if !path.starts_with(&crate_dir) {
            return Ok(None);
        }
        let module_path = path
            .strip_prefix(&crate_dir)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let exports = read_exports(&payload_path, &payload, &module_path);
        let export_refs: Vec<&str> = exports.iter().map(String::as_str).collect();
        let (source, _stubs) = rust_fn_stub_source(&module_path, &export_refs, env, &secret);
        Ok(Some(source))
    }))
}

/// The payload file the build emission and the serve boot walk agree on.
fn payload_file(rootp: &Path) -> PathBuf {
    rootp.join(".oxide/rust/types.json")
}

/// The module's export names, read once per bundle (the memoized payload).
/// A payload failure is a loud warn and an empty export list — the
/// header-only module fails the bundle naming the import, never silently.
fn read_exports(
    payload_path: &Path,
    memo: &OnceLock<Option<String>>,
    module_path: &str,
) -> Vec<String> {
    let read = memo.get_or_init(|| match std::fs::read_to_string(payload_path) {
        Ok(text) => Some(text),
        Err(e) => {
            tracing::warn!(
                code = RFN_TYPES,
                path = %payload_path.display(),
                error = %e,
                "the native function types payload is unreadable; `.server.rs` imports load as header-only stubs"
            );
            None
        }
    });
    let Some(text) = read else {
        return Vec::new();
    };
    let payload = serde_json::from_str::<TypesPayload>(text).unwrap_or_else(|e| {
        tracing::warn!(
            code = RFN_TYPES,
            path = %payload_path.display(),
            error = %e,
            "the native function types payload did not parse; `.server.rs` imports load as header-only stubs"
        );
        TypesPayload { fns: Vec::new(), decls: std::collections::BTreeMap::new() }
    });
    payload
        .fns
        .iter()
        .filter(|f| f.file.replace('\\', "/") == module_path)
        .map(|f| f.name.clone())
        .collect()
}

/// Compose two load hooks: the first non-`None` answer wins (the
/// server-rs arm first, the JS-plugin arm second — the SPEC-15 order the
/// plan's transform composition documents).
#[must_use]
pub fn compose_load_hooks(first: Option<LoadHook>, second: Option<LoadHook>) -> Option<LoadHook> {
    match (first, second) {
        (Some(a), Some(b)) => Some(LoadHook::new(move |id: &str| {
            if let Some(code) = a.call(id)? {
                return Ok(Some(code));
            }
            b.call(id)
        })),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

/// Compose two resolve hooks (the same first-wins contract).
#[must_use]
pub fn compose_resolve_hooks(
    first: Option<ResolveHook>,
    second: Option<ResolveHook>,
) -> Option<ResolveHook> {
    match (first, second) {
        (Some(a), Some(b)) => Some(ResolveHook::new(move |specifier: &str, importer: &str| {
            if let Some(p) = a.call(specifier, importer)? {
                return Ok(Some(p));
            }
            b.call(specifier, importer)
        })),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

// ---------------------------------------------------------------------------
// The dev task (the cmd_dev adoption)
// ---------------------------------------------------------------------------

/// What the dev wiring hands `cmd_dev`: the consult source (the SAME
/// `Arc` the task swaps under) and the task's join handle.
pub struct DevNativeHandle {
    /// The consult source `start_dev_server_full` threads into the
    /// dispatcher.
    pub source: Arc<dyn NativeFnSource>,
    /// The loop task (`shutdown` aborts it — dev exits via Ctrl-C, this
    /// is the test hook).
    pub task: tokio::task::JoinHandle<()>,
}

impl DevNativeHandle {
    /// Stop the loop task (the pins' teardown hook).
    pub fn shutdown(&self) {
        self.task.abort();
    }
}

/// The `cmd_dev` wiring with injectable seams: discover the crate, mount
/// the newest existing generation into the swappable host (so a dev
/// server after a build serves the built library immediately), spawn the
/// watch → build → load → swap task, and return the live consult.
///
/// `None` when the project carries no native crate — the consult then
/// rides the boot mount walk (the pre-finish shape).
pub fn dev_native_consult_with(
    rootp: &Path,
    secret: &str,
    build: Box<dyn LibBuild>,
    load: Box<dyn LibLoad>,
    tick: Duration,
) -> Option<DevNativeHandle> {
    let nc = discover_native_crate(rootp)?;
    let host = Arc::new(SwappableHost::new());
    mount_existing_generation(&host, &nc, rootp, secret);
    let mut lp = oxide_fw_dev::server_lib::ServerLibLoop::new(
        ServerLibSpec::new(nc.name.clone(), nc.dir.clone()),
        build,
        Box::new(HostSwapper::new(Arc::clone(&host))),
    )
    .with_loader(load)
    .with_dest_dir(rootp.join(".oxide/rust"))
    .with_secret(secret);
    lp.seed();
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match lp.poll_once() {
                Ok(Some(report)) => {
                    tracing::info!(
                        generation = report.generation,
                        rebound = report.rebound.len(),
                        added = report.added.len(),
                        dropped = report.dropped.len(),
                        "the native fn crate reloaded; the consult serves the new generation"
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        code = %e.0.code,
                        msg = %e.0.message,
                        "the native fn rebuild failed; the previous mount keeps serving"
                    );
                }
            }
        }
    });
    Some(DevNativeHandle {
        source: live_host_source(host),
        task,
    })
}

/// Mount the newest existing generation (the highest `lib-<n>` the mount
/// dir holds) into the dev host, so a dev server after a build serves the
/// built library without waiting for the first edit. A broken or
/// payload-less library is a loud warn and an empty host — the first
/// swap fills it.
fn mount_existing_generation(host: &SwappableHost, nc: &NativeCrate, rootp: &Path, secret: &str) {
    let rust_dir = rootp.join(".oxide/rust");
    let Ok(entries) = std::fs::read_dir(&rust_dir) else {
        return;
    };
    let mut libs: Vec<(u64, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e, "so" | "dll" | "dylib"))
        })
        .map(|p| (generation_of(&p), p))
        .collect();
    libs.sort_by(|a, b| b.0.cmp(&a.0));
    let Some((_, newest)) = libs.into_iter().next() else {
        return;
    };
    let lib = match RustFnLibrary::load(&newest, &rust_dir) {
        Ok(lib) => lib,
        Err(e) => {
            tracing::warn!(
                code = oxide_rustfn::host::RFN_LOAD,
                crate = %nc.name,
                path = %newest.display(),
                error = %e.0.message,
                "the previous build's native library failed to mount; the first dev swap fills the host"
            );
            return;
        }
    };
    let Some(types) = lib.types_json() else {
        tracing::warn!(
            code = RFN_TYPES,
            crate = %nc.name,
            "the previous build's library carries no types payload; the first dev swap fills the host"
        );
        return;
    };
    match oxide_fw_dev::server_lib::bindings_from_types(types, secret) {
        Ok(bindings) => {
            if let Err(e) = host.swap(lib, &bindings) {
                tracing::warn!(
                    code = %e.0.code,
                    msg = %e.0.message,
                    "the previous build's mount failed; the first dev swap fills the host"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                code = %e.0.code,
                msg = %e.0.message,
                "the previous build's bindings did not re-derive; the first dev swap fills the host"
            );
        }
    }
}

/// The `lib-<n>` generation number (unparsable names sort oldest).
fn generation_of(path: &Path) -> u64 {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("lib-"))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

/// The production dev wiring: the real cargo build/load pair (the
/// `#[ignore]` real-cargo pins in the dev crate prove the cargo half).
pub(crate) fn dev_native_consult(rootp: &Path, secret: &str) -> Option<Arc<dyn NativeFnSource>> {
    let handle = dev_native_consult_with(
        rootp,
        secret,
        Box::new(CargoLibBuild::new()),
        Box::new(oxide_fw_dev::server_lib::CopyLoader),
        Duration::from_millis(300),
    )?;
    let DevNativeHandle { source, task } = handle;
    // Dropping the join handle detaches the task — it runs for the dev
    // session's lifetime and dies with the runtime.
    drop(task);
    Some(source)
}
