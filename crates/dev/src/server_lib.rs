//! The §V2-F08 dev-loop half (`feat/server-lib-dev-loop`): the watch →
//! build → load → swap loop the `oxide dev` task drives for a native
//! `*.server.rs` crate. The spec: "`oxide dev` watches `**/*.server.rs`
//! and `Cargo.toml`, runs `cargo build --lib` for the user crate … and
//! hot-swaps by loading the new copy and retiring the old `Arc<Library>`
//! after in-flight calls finish."
//!
//! The seams, each injectable so a host (and the tests) can run the full
//! story against fakes (the V2.0 item 3 test-placement rule):
//!
//! - [`LibBuild`] — the rebuild pass. [`CargoLibBuild`] runs the spec's
//!   `cargo build --lib` in the crate dir and refuses a broken crate with
//!   a coded `DEV-RFN-001` carrying cargo's stderr tail.
//! - [`LibLoad`] — the mount pass. [`CopyLoader`] is the host crate's
//!   [`RustFnLibrary::load`]: the cdylib is COPIED under the mount dir
//!   (`lib-<n>.<ext>`) and the copy is what loads — never the file cargo
//!   wrote (Windows locks it). The loader's gates (`RFN-001`/`RFN-003`)
//!   are the loop's gates.
//! - [`ServerLibSwap`] — the hand-off. [`HostSwapper`] drives the
//!   swappable host state ([`oxide_rustfn::swap::SwappableHost`]): the
//!   bindings re-point under the new mount, the removed exports drop to
//!   the JS-pool consult, and the old mount retires once no in-flight
//!   call holds it.
//!
//! The §9.4 id bindings ([`bindings_from_types`]) re-derive through the
//! ONE `server_fn_id` definition the compiler crate owns (§9.4, D6) —
//! the loop never re-implements the digest. A rebuild whose types side
//! export is missing or corrupt refuses `DEV-RFN-002`: without the
//! bindings the swap would either drop every id or guess — both are
//! silently wrong, so it is loud instead.
//!
//! [`ServerLibLoop`] is poll-shaped like the dev crate's watcher (§5.5):
//! one [`ServerLibLoop::poll_once`] per tick, no internal threads. A
//! failed step REFUSES the swap and keeps the previous mount serving — a
//! broken rebuild never takes the running dev server's native fns down.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxide_fw_compiler::server_fn_id;
use oxide_fw_diagnostics::OxideError;
use oxide_rustfn::host::RustFnLibrary;
use oxide_rustfn::swap::SwappableHost;
use oxide_rustfn::types::TypesPayload;

// The swap surface's types ride the host crate's definitions — ONE
// `SwapReport` shape for the loop, the host state and the consult.
pub use oxide_rustfn::swap::{FnBinding, SwapReport};

use crate::err;
use crate::{FileEvent, Watcher};

/// The rebuild refusal (`DEV-RFN-001`): `cargo build --lib` failed, or
/// built without producing the cdylib. The message names the crate, the
/// command and cargo's stderr tail — the fix is the compile error itself.
pub const DEV_RFN_001: &str = "DEV-RFN-001";
/// The bindings refusal (`DEV-RFN-002`): the rebuilt library's types side
/// export is missing or does not parse, so the §9.4 id bindings cannot be
/// re-derived. The swap refuses LOUD — a zero-binding swap would silently
/// drop every id, which is the one failure shape the outbox-era refusals
/// all rule out.
pub const DEV_RFN_002: &str = "DEV-RFN-002";

/// The native server-fn crate one loop watches and rebuilds: the name is
/// the `[lib] name` (the cdylib file is derived from it), `crate_dir` is
/// the crate root that owns `Cargo.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerLibSpec {
    /// The `[lib] name` of the native crate.
    pub name: String,
    /// The crate root (`Cargo.toml` lives here).
    pub crate_dir: PathBuf,
}

impl ServerLibSpec {
    /// A spec for one native crate.
    pub fn new(name: impl Into<String>, crate_dir: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            crate_dir: crate_dir.into(),
        }
    }
}

/// One rebuild's artifact: the cdylib path the loader mounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutput {
    /// The cdylib cargo wrote (the loader copies it under the mount dir —
    /// this path is never loaded from directly).
    pub cdylib_path: PathBuf,
}

/// The rebuild pass: turn a [`ServerLibSpec`] into the freshly built
/// cdylib. Fakes serve the loop's pins; [`CargoLibBuild`] is production.
pub trait LibBuild: Send + Sync {
    /// Run the rebuild.
    ///
    /// # Errors
    ///
    /// A coded failure (`DEV-RFN-001` for the production impl) — the loop
    /// refuses the step and the previous mount keeps serving.
    fn build(&self, spec: &ServerLibSpec) -> Result<BuildOutput, OxideError>;
}

/// The mount pass: load a rebuilt cdylib through the host's gates. The
/// production [`CopyLoader`] is [`RustFnLibrary::load`] (the copy
/// semantics); the embedded-host shape serves its own impl.
pub trait LibLoad: Send + Sync {
    /// Load the cdylib into `dest_dir` (the mount dir).
    ///
    /// # Errors
    ///
    /// The host's coded gates ride through (`RFN-001` for a foreign ABI,
    /// `RFN-003` for an unloadable file or duplicate entries).
    fn load(&self, cdylib: &Path, dest_dir: &Path) -> Result<RustFnLibrary, OxideError>;
}

/// The production mount pass: the host crate's copy-then-load ([`CargoLibBuild`]'s
/// output is cargo's own write — the copy is what loads).
pub struct CopyLoader;

impl LibLoad for CopyLoader {
    fn load(&self, cdylib: &Path, dest_dir: &Path) -> Result<RustFnLibrary, OxideError> {
        RustFnLibrary::load(cdylib, dest_dir)
    }
}

/// The hand-off seam: receive the freshly loaded library with its §9.4
/// bindings. [`HostSwapper`] is production (the swappable host state).
pub trait ServerLibSwap: Send + Sync {
    /// Swap the library in under the bindings.
    ///
    /// # Errors
    ///
    /// The host state's refusal (the poisoned-lock case).
    fn swap(
        &self,
        library: RustFnLibrary,
        bindings: &[oxide_rustfn::swap::FnBinding],
    ) -> Result<oxide_rustfn::swap::SwapReport, OxideError>;
}

/// The production hand-off: the swappable host state the dev task shares
/// with the consult adapter. `Arc`-shaped — the same handle the future
/// `NativeFnSource` adapter holds.
pub struct HostSwapper {
    host: Arc<SwappableHost>,
}

impl HostSwapper {
    /// Wrap one swappable host.
    #[must_use]
    pub fn new(host: Arc<SwappableHost>) -> Self {
        Self { host }
    }

    /// The wrapped host (the consult adapter's call surface).
    #[must_use]
    pub fn host(&self) -> &Arc<SwappableHost> {
        &self.host
    }
}

impl ServerLibSwap for HostSwapper {
    fn swap(
        &self,
        library: RustFnLibrary,
        bindings: &[oxide_rustfn::swap::FnBinding],
    ) -> Result<oxide_rustfn::swap::SwapReport, OxideError> {
        self.host.swap(library, bindings)
    }
}

/// Whether a watcher event is one of the loop's triggers — the spec's
/// watch set exactly: a `*.server.rs` source or the crate's `Cargo.toml`,
/// INSIDE the native crate dir, never under `target/` (the watcher skips
/// the directory, but the notify backend surfaces raw paths).
///
/// The `src/server.rs` spelling is NOT a trigger (the watch set is the
/// `.server.rs` suffix, not a file name); a `Removed` event triggers like
/// an edit (the rebuild re-derives the bindings, and a removed module
/// drops its exports).
#[must_use]
pub fn is_server_lib_event(ev: &FileEvent, spec: &ServerLibSpec) -> bool {
    let path = match ev {
        FileEvent::Modified(p) | FileEvent::Created(p) | FileEvent::Removed(p) => p,
    };
    is_server_lib_path(path, spec)
}

/// The path half of [`is_server_lib_event`] (the filter the notify-driven
/// host can also run over a raw path).
#[must_use]
pub fn is_server_lib_path(path: &Path, spec: &ServerLibSpec) -> bool {
    if !path.starts_with(&spec.crate_dir) {
        return false;
    }
    if path
        .components()
        .any(|c| c.as_os_str() == std::ffi::OsStr::new("target"))
    {
        return false;
    }
    let is_manifest = path
        .file_name()
        .is_some_and(|n| n == std::ffi::OsStr::new("Cargo.toml"));
    let is_server_module = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".server.rs"));
    is_manifest || is_server_module
}

/// Re-derive the §9.4 id bindings from the rebuilt library's types side
/// export and the build secret — the ONE `server_fn_id(file, export,
/// secret)` definition the compiler crate owns (§9.4, D6). Sorted by id,
/// so the swap's report is byte-stable.
///
/// # Errors
///
/// [`DEV_RFN_002`] when the payload is missing or does not parse: the
/// bindings cannot be re-derived, so the swap refuses (a zero-binding
/// swap would silently drop every id).
pub fn bindings_from_types(
    types_json: &str,
    secret: &str,
) -> Result<Vec<oxide_rustfn::swap::FnBinding>, OxideError> {
    let payload: TypesPayload = serde_json::from_str(types_json).map_err(|e| {
        err(
            DEV_RFN_002,
            format!("the rebuilt library's types payload does not parse (the §9.4 id bindings cannot be re-derived): {e}"),
        )
    })?;
    let mut bindings = Vec::with_capacity(payload.fns.len());
    for f in &payload.fns {
        bindings.push(oxide_rustfn::swap::FnBinding::new(
            server_fn_id(&f.file, &f.name, secret),
            &f.name,
        ));
    }
    bindings.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(bindings)
}

/// The watch → build → load → swap loop over one native crate. Poll-shaped
/// like the dev crate's watcher: one [`Self::poll_once`] per tick, no
/// internal threads (the dev task's interval drives it).
pub struct ServerLibLoop {
    spec: ServerLibSpec,
    watcher: Watcher,
    builder: Box<dyn LibBuild>,
    loader: Box<dyn LibLoad>,
    swap: Box<dyn ServerLibSwap>,
    dest_dir: PathBuf,
    secret: String,
    seen: BTreeSet<(PathBuf, String)>,
}

impl ServerLibLoop {
    /// A loop over one native crate: the poll-backend watcher rooted at
    /// the crate dir, the production [`CopyLoader`], the mount dir
    /// defaulting to `<crate_dir>/.oxide/rust` (the consult's mount walk)
    /// and an empty build secret (set [`Self::with_secret`] before the
    /// first poll — the ids are wrong without it).
    #[must_use]
    pub fn new(
        spec: ServerLibSpec,
        builder: Box<dyn LibBuild>,
        swap: Box<dyn ServerLibSwap>,
    ) -> Self {
        let dest_dir = spec.crate_dir.join(".oxide").join("rust");
        let watcher = Watcher::new(vec![spec.crate_dir.clone()]);
        Self {
            spec,
            watcher,
            builder,
            loader: Box::new(CopyLoader),
            swap,
            dest_dir,
            secret: String::new(),
            seen: BTreeSet::new(),
        }
    }

    /// Override the mount pass (the embedded-host shape, or a fake).
    #[must_use]
    pub fn with_loader(mut self, loader: Box<dyn LibLoad>) -> Self {
        self.loader = loader;
        self
    }

    /// Override the mount dir (the CLI adoption passes the app root's
    /// `.oxide/rust` — the consult's mount walk).
    #[must_use]
    pub fn with_dest_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dest_dir = dir.into();
        self
    }

    /// The build secret the §9.4 ids hash (the CLI adoption passes the
    /// app's build secret — the same one the stub generator used).
    #[must_use]
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        self.secret = secret.into();
        self
    }

    /// Seed the watcher's seen-set (the boot pass: no events for the
    /// current state — the first poll only reports REAL changes).
    pub fn seed(&mut self) {
        self.watcher.seed_seen(&mut self.seen);
    }

    /// One loop step: poll the watch set; on a `*.server.rs` / `Cargo.toml`
    /// change run build → load → bindings → swap. A poll with no relevant
    /// change is `Ok(None)`; a change runs exactly one step and hands back
    /// the host's swap report. A failed step refuses (the error rides out)
    /// and the previous mount keeps serving.
    ///
    /// # Errors
    ///
    /// The step's coded failures ride through: [`DEV_RFN_001`] (the
    /// build), the loader's gates (`RFN-001`/`RFN-003`), [`DEV_RFN_002`]
    /// (the bindings) — each warned at the refusal site first, so the
    /// tracing pipeline sees the story even when the driver treats the
    /// poll as fire-and-forget.
    pub fn poll_once(&mut self) -> Result<Option<oxide_rustfn::swap::SwapReport>, OxideError> {
        let events = self.watcher.poll(&mut self.seen)?;
        let relevant = events.iter().any(|ev| is_server_lib_event(ev, &self.spec));
        if !relevant {
            return Ok(None);
        }
        tracing::info!(
            crate = %self.spec.name,
            "the native fn crate changed; rebuilding the cdylib"
        );
        let out = self.builder.build(&self.spec).inspect_err(|e| {
            tracing::warn!(
                code = DEV_RFN_001,
                crate = %self.spec.name,
                error = %e,
                "the native fn crate failed to rebuild; the previous mount keeps serving"
            );
        })?;
        let lib = self
            .loader
            .load(&out.cdylib_path, &self.dest_dir)
            .inspect_err(|e| {
                tracing::warn!(
                    code = "RFN-LOAD",
                    crate = %self.spec.name,
                    path = %out.cdylib_path.display(),
                    error = %e,
                    "the rebuilt native fn library failed to load; the previous mount keeps serving"
                );
            })?;
        let Some(types) = lib.types_json().map(str::to_owned) else {
            let e = err(
                DEV_RFN_002,
                format!(
                    "the rebuilt library of crate {:?} carries no types side export; the §9.4 id bindings cannot be re-derived — rebuild with the `export_table!` macro's `oxide_fn_types_json` export",
                    self.spec.name
                ),
            );
            tracing::warn!(
                code = DEV_RFN_002,
                crate = %self.spec.name,
                error = %e,
                "the rebuilt library carries no types payload; the swap is refused"
            );
            return Err(e);
        };
        let bindings = bindings_from_types(&types, &self.secret)?;
        let report = self.swap.swap(lib, &bindings)?;
        tracing::info!(
            crate = %self.spec.name,
            generation = report.generation,
            rebound = report.rebound.len(),
            added = report.added.len(),
            dropped = report.dropped.len(),
            "the native fn crate reloaded"
        );
        Ok(Some(report))
    }
}

/// The production rebuild pass: `cargo build --lib` in the crate dir
/// (the spec's command; `--release` for the [`CargoProfile::Release`]
/// profile), then the cdylib path cargo wrote under `target/<profile>/`.
pub struct CargoLibBuild {
    profile: CargoProfile,
}

/// The cargo profile the loop builds (dev default; the CLI adoption may
/// pass release for a production-shaped dev loop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CargoProfile {
    /// `cargo build --lib` (the dev loop's default).
    #[default]
    Debug,
    /// `cargo build --lib --release`.
    Release,
}

impl CargoProfile {
    /// The `target/<dir>` directory name cargo writes under.
    #[must_use]
    pub fn dir(self) -> &'static str {
        match self {
            CargoProfile::Debug => "debug",
            CargoProfile::Release => "release",
        }
    }
}

impl CargoLibBuild {
    /// A dev-profile build pass.
    #[must_use]
    pub fn new() -> Self {
        Self {
            profile: CargoProfile::Debug,
        }
    }

    /// A build pass at an explicit profile.
    #[must_use]
    pub fn for_profile(profile: CargoProfile) -> Self {
        Self { profile }
    }
}

impl Default for CargoLibBuild {
    fn default() -> Self {
        Self::new()
    }
}

impl LibBuild for CargoLibBuild {
    fn build(&self, spec: &ServerLibSpec) -> Result<BuildOutput, OxideError> {
        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("build").arg("--lib");
        if matches!(self.profile, CargoProfile::Release) {
            cmd.arg("--release");
        }
        cmd.current_dir(&spec.crate_dir);
        let out = cmd.output().map_err(|e| {
            err(
                DEV_RFN_001,
                format!(
                    "the native fn crate {:?} failed to launch cargo (is cargo on the PATH?): {e}",
                    spec.name
                ),
            )
        })?;
        if !out.status.success() {
            return Err(err(
                DEV_RFN_001,
                format!(
                    "the native fn crate {:?} failed to build (cargo build --lib{}): {}",
                    spec.name,
                    if matches!(self.profile, CargoProfile::Release) {
                        " --release"
                    } else {
                        ""
                    },
                    stderr_tail(&out.stderr)
                ),
            ));
        }
        let cdylib = cdylib_path(spec, self.profile);
        if !cdylib.is_file() {
            return Err(err(
                DEV_RFN_001,
                format!(
                    "the native fn crate {:?} built but no cdylib was found at {} — check the [lib] crate-type (the dev loop loads a cdylib)",
                    spec.name,
                    cdylib.display()
                ),
            ));
        }
        Ok(BuildOutput {
            cdylib_path: cdylib,
        })
    }
}

/// The cdylib path cargo writes for `[lib] name = <spec.name>`: the
/// platform table (`lib<name>.so` / `lib<name>.dylib` / `<name>.dll`),
/// with `-` normalized to `_` (cargo's crate-name rule).
#[must_use]
pub fn cdylib_path(spec: &ServerLibSpec, profile: CargoProfile) -> PathBuf {
    let stem = spec.name.replace('-', "_");
    let file = if cfg!(target_os = "windows") {
        format!("{stem}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else {
        format!("lib{stem}.so")
    };
    spec.crate_dir.join("target").join(profile.dir()).join(file)
}

/// The last ~2000 chars of cargo's stderr (the errors are at the end; a
/// wall of compilation progress is not useful in a diagnostic).
fn stderr_tail(stderr: &[u8]) -> String {
    const TAIL: usize = 2000;
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim_end();
    let char_count = trimmed.chars().count();
    if char_count <= TAIL {
        return trimmed.to_string();
    }
    let skip = char_count - TAIL;
    trimmed.chars().skip(skip).collect()
}
