//! The loader and the id-mapped registry (spec §V2-F08 "Host"):
//!
//! > `RustFnHost::load(path)`: copy the library to `.oxide/rust/
//! > lib-<n>.<ext>` (never load the file cargo writes; Windows locks
//! > it), `libloading::Library::new`, read `oxide_fn_table`, check
//! > `abi_version == ABI_VERSION` (`RFN-001` on mismatch), index
//! > entries by name. A call runs on `spawn_blocking` with a
//! > per-function timeout (default 30 s) and a bounded semaphore.
//!
//! The registry is keyed by the §9.4 id — the same `server_fn_id`
//! triple the client stubs call — so the dispatcher consult (the
//! server crate's `RustFirstDispatcher`) cannot tell a native fn from
//! a JS one. Hot-swap is mount + re-register: the old library's
//! `Arc` retires when the last in-flight call finishes.

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::ffi::c_char;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use libloading::Library;
use oxide_fw_diagnostics::OxideError;
use tokio::sync::Semaphore;

use crate::abi::{ABI_VERSION, FnTable, RawBuf, STATUS_ERROR, STATUS_OK, STATUS_PANIC};
use crate::bridge::{RFN_WIRE, decode_error_message};
use crate::err;

/// The ABI-mismatch code (spec V2.K): the library was built against a
/// different `oxide-rustfn` ABI than the host speaks.
pub const RFN_ABI_MISMATCH: &str = "RFN-001";
/// The load-failure code: a library that does not load, does not
/// carry `oxide_fn_table`, or indexes two entries under one name.
pub const RFN_LOAD: &str = "RFN-003";

/// The spec's per-function call budget (§V2-F08: "a per-function
/// timeout (default 30 s)").
pub const RUSTFN_TIMEOUT_SECS: u64 = 30;

/// The default bounded-semaphore lanes (§V2-F08: "a bounded
/// semaphore" — the native calls are blocking work, so the default is
/// the conservative `available_parallelism`-shaped 16; the host with
/// `with_lanes` sizes it).
const DEFAULT_LANES: usize = 16;

/// A loaded native-function library: the table, the name index and
/// (when the crate exports it) the types JSON. Construct with
/// [`RustFnLibrary::load`] (the `libloading` path) or
/// [`RustFnLibrary::from_static`] (the in-process seam embedded hosts
/// and tests drive — the same gates, no dlopen).
pub struct RustFnLibrary {
    /// The path the library was loaded from (the versioned copy, not
    /// cargo's file) — `in-process` for a static table.
    path: PathBuf,
    /// The dlopen handle kept alive for the table's lifetime; `None`
    /// for an in-process table. Kept (never read) so the library
    /// outlives every call through `table`.
    lib: Option<Library>,
    /// The table pointer (`&'static` for the static seam; valid while
    /// `lib` holds the library).
    table: *const FnTable,
    /// The export name → entry-index map (`BTreeMap`: `names()` is
    /// sorted, duplicate names refuse at load).
    index: BTreeMap<String, usize>,
    /// The optional `oxide_fn_types_json` payload (absent → `None`).
    types: Option<String>,
}

// SAFETY: the table's data is immutable library memory for the handle's
// lifetime (`lib` lives as long as the struct), the fn pointers it
// carries are the user crate's `extern "C"` shims (thread-safe by the
// same contract any server handler has — calls may run concurrently),
// and `libloading::Library` is Send + Sync.
unsafe impl Send for RustFnLibrary {}
// SAFETY: see the Send impl — shared access only reads immutable table
// data and `call_raw` hands every buffer back through the table's own
// `free`.
unsafe impl Sync for RustFnLibrary {}

impl std::fmt::Debug for RustFnLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The table pointer and the dlopen handle are not printable —
        // the debug shape is the loader-relevant view (path, ABI,
        // indexed names, types presence).
        f.debug_struct("RustFnLibrary")
            .field("path", &self.path)
            .field("abi_version", &self.abi_version())
            .field("names", &self.names())
            .field("types", &self.types.is_some())
            .field("table", &self.table)
            .field("index", &self.index.len())
            .field("lib", &self.lib.is_some())
            .finish()
    }
}

/// The failure of one native call, before the envelope mapping
/// ([`RegistryError`] carries the wire shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawCallError {
    /// The name is not in the table's index (a registry/id mismatch).
    NotFound(String),
    /// The fn returned `Err` — the decoded message rides along.
    FnError(String),
    /// The fn panicked (status 2; the payload is the shim's record).
    Panic,
}

impl std::fmt::Display for RawCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "no native function named {name:?}"),
            Self::FnError(message) => write!(f, "{message}"),
            Self::Panic => write!(f, "the native function panicked"),
        }
    }
}

impl RustFnLibrary {
    /// Copy `source` to `dest_dir/lib-<n>.<ext>` (the smallest free
    /// index, or the existing copy when the bytes are identical — a
    /// rebuild without a code change does not re-copy) and load it.
    ///
    /// # Errors
    ///
    /// [`RFN_LOAD`] when the copy or the load fails (a missing file,
    /// a non-library image, a table that does not parse, duplicate
    /// names); [`RFN_ABI_MISMATCH`] when the table speaks a foreign
    /// ABI version.
    pub fn load(source: &Path, dest_dir: &Path) -> Result<Self, OxideError> {
        let dest = copy_to_versioned(source, dest_dir)?;
        // SAFETY: `Library::new` loads the (already copied) image; the
        // handle keeps it alive in the returned struct.
        let lib = unsafe { Library::new(&dest) }.map_err(|e| {
            err(
                RFN_LOAD,
                format!(
                    "native function library {} failed to load: {e}",
                    dest.display()
                ),
            )
        })?;
        let types = read_types_export(&lib);
        unsafe { Self::from_parts(dest, lib, types) }
    }

    /// The in-process seam: an embedded host (or a test) hands over a
    /// static table and the same ABI/duplicate gates run without
    /// dlopen. `types` carries the crate's `oxide_fn_types_json`
    /// payload when it has one.
    ///
    /// # Errors
    ///
    /// Same gates as [`RustFnLibrary::load`] (no copy, no dlopen).
    pub fn from_static(
        table: &'static FnTable,
        types: Option<&'static str>,
    ) -> Result<Self, OxideError> {
        let index = Self::gate_and_index(table)?;
        Ok(Self {
            path: PathBuf::from("in-process"),
            lib: None,
            table,
            index,
            types: types.map(str::to_string),
        })
    }

    /// The libloading path's tail: the table pointer is read from the
    /// export and the gates run once, here.
    ///
    /// # Safety
    ///
    /// `lib` must own the memory `table_ptr`'s export points at.
    unsafe fn from_parts(
        path: PathBuf,
        lib: Library,
        types: Option<String>,
    ) -> Result<Self, OxideError> {
        // SAFETY: the caller guarantees the library owns the export.
        // `oxide_fn_table` is a FUNCTION returning the pointer (the
        // spec's export shape), so the symbol type is the fn signature —
        // reading the symbol as data would decode machine code.
        let sym: libloading::Symbol<'_, extern "C" fn() -> *const FnTable> =
            unsafe { lib.get(b"oxide_fn_table") }.map_err(|e| {
                err(
                    RFN_LOAD,
                    format!("native function library has no oxide_fn_table export: {e}"),
                )
            })?;
        let table_ptr: *const FnTable = sym();
        let index = Self::gate_and_index(table_ptr)?;
        let _ = sym;
        Ok(Self {
            path,
            lib: Some(lib),
            table: table_ptr,
            index,
            types,
        })
    }

    /// The load-time gates: the ABI version first (the `RFN-001`
    /// message names saw-and-expected), then the name index (a
    /// duplicate name is the same refusal class as a broken table).
    fn gate_and_index(table: *const FnTable) -> Result<BTreeMap<String, usize>, OxideError> {
        // SAFETY: the caller guarantees the table pointer is live.
        let t = unsafe { &*table };
        if t.abi_version != ABI_VERSION {
            return Err(err(
                RFN_ABI_MISMATCH,
                format!(
                    "native function library speaks ABI {saw}, the host speaks ABI {want}; rebuild the user crate with the matching oxide-rustfn version",
                    saw = t.abi_version,
                    want = ABI_VERSION
                ),
            ));
        }
        let mut index = BTreeMap::new();
        for i in 0..t.len {
            // SAFETY: `entries` points at `len` contiguous FnEntries.
            let entry = unsafe { &*t.entries.add(i) };
            // SAFETY: entry names are NUL-terminated strings the
            // macros (or the hand-built table's contract) leaked.
            let name = unsafe { CStr::from_ptr(entry.name) }
                .to_string_lossy()
                .into_owned();
            if index.insert(name, i).is_some() {
                return Err(err(
                    RFN_LOAD,
                    "native function library indexes two entries under one name",
                ));
            }
        }
        Ok(index)
    }

    /// The sorted export names.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.index.keys().cloned().collect()
    }

    /// The ABI version the table speaks (the post-gate value is always
    /// [`ABI_VERSION`]; kept for diagnostics).
    #[must_use]
    pub fn abi_version(&self) -> u32 {
        // SAFETY: the table is live for the struct's lifetime.
        unsafe { (*self.table).abi_version }
    }

    /// The optional types payload (the `oxide_fn_types_json` side
    /// export; `None` when the crate did not export one).
    #[must_use]
    pub fn types_json(&self) -> Option<&str> {
        self.types.as_deref()
    }

    /// The path the library was loaded from (the versioned copy).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// One blocking call: encode-free (the caller hands ODF bytes in),
    /// the status mapped, the buffer released through the table's own
    /// `free` — the host never deallocates with a foreign allocator.
    ///
    /// # Errors
    ///
    /// [`RawCallError::NotFound`] for a name outside the index;
    /// [`RawCallError::FnError`] for status 1 (the decoded message);
    /// [`RawCallError::Panic`] for status 2.
    pub fn call_raw(&self, name: &str, input_odf: &[u8]) -> Result<Vec<u8>, RawCallError> {
        let entry_index = *self
            .index
            .get(name)
            .ok_or_else(|| RawCallError::NotFound(name.to_string()))?;
        // SAFETY: the table is live and `entry_index < len`.
        let entry = unsafe { &*(*self.table).entries.add(entry_index) };
        let (ptr, len) = if input_odf.is_empty() {
            (std::ptr::null(), 0usize)
        } else {
            (input_odf.as_ptr(), input_odf.len())
        };
        let mut out = RawBuf::empty();
        // SAFETY: the table is live for the struct's lifetime and the
        // entry pointer is within its bounds; the callee is the ABI
        // signature the shims implement.
        let status = (entry.call)(ptr, len, &raw mut out);
        let bytes: Vec<u8> = if out.len == 0 {
            Vec::new()
        } else {
            // SAFETY: the shim allocated exactly this buffer.
            unsafe { out.as_slice() }.to_vec()
        };
        // The library's allocator releases its own allocation.
        // SAFETY: the buffer came from this table's shims.
        unsafe { ((*self.table).free)(out) };
        match status {
            STATUS_OK => Ok(bytes),
            STATUS_ERROR => {
                let message = decode_error_message(&bytes)
                    .unwrap_or_else(|e| format!("the native fn error payload did not decode: {e}"));
                Err(RawCallError::FnError(message))
            }
            STATUS_PANIC => Err(RawCallError::Panic),
            other => Err(RawCallError::FnError(format!(
                "the native fn shim returned the unknown status {other}"
            ))),
        }
    }
}

/// Copy `source` beside itself as `lib-<n>.<ext>` (never load the
/// file cargo wrote; Windows locks it). A byte-identical copy reuses
/// its existing slot.
fn copy_to_versioned(source: &Path, dest_dir: &Path) -> Result<PathBuf, OxideError> {
    let bytes = std::fs::read(source)
        .map_err(|e| err(RFN_LOAD, format!("cannot read {}: {e}", source.display())))?;
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        err(
            RFN_LOAD,
            format!("cannot create {}: {e}", dest_dir.display()),
        )
    })?;
    let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("so");
    let mut first_free: Option<usize> = None;
    let entries = std::fs::read_dir(dest_dir)
        .map_err(|e| err(RFN_LOAD, format!("cannot scan {}: {e}", dest_dir.display())))?;
    let mut used: Vec<usize> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let candidate = dest_dir.join(entry.file_name());
        if candidate.is_file() && std::fs::read(&candidate).is_ok_and(|b| b == bytes) {
            return Ok(candidate);
        }
        if let Some(rest) = name
            .strip_prefix("lib-")
            .and_then(|r| r.strip_suffix(&format!(".{ext}")))
            && let Ok(n) = rest.parse::<usize>()
        {
            used.push(n);
        }
    }
    used.sort_unstable();
    for (i, n) in used.iter().enumerate() {
        if *n != i {
            first_free = Some(i);
            break;
        }
    }
    let n = first_free.unwrap_or(used.len());
    let dest = dest_dir.join(format!("lib-{n}.{ext}"));
    std::fs::write(&dest, &bytes)
        .map_err(|e| err(RFN_LOAD, format!("cannot write {}: {e}", dest.display())))?;
    Ok(dest)
}

/// Read the optional `oxide_fn_types_json` side export (a function
/// returning the NUL-terminated pointer; a missing export or a null
/// read → `None`).
fn read_types_export(lib: &Library) -> Option<String> {
    // SAFETY: `get` resolves the export in the caller's loaded library.
    let sym: libloading::Symbol<'_, extern "C" fn() -> *const c_char> =
        unsafe { lib.get(b"oxide_fn_types_json") }.ok()?;
    let ptr = sym();
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the export is a NUL-terminated static string the crate
    // generated.
    let s = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    if s.is_empty() { None } else { Some(s) }
}

/// The registry error the wire sees — the envelope codes mirror the
/// JS-pool dispatcher's shapes (`FN-NOT-FOUND`, `FN-THREW`, …), so the
/// HTTP route cannot tell a native fn from a JS one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError {
    /// The failure class (drives the HTTP status).
    pub kind: RegistryErrorKind,
    /// The human message (the envelope's message field).
    pub message: String,
}

/// The failure classes and their wire mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryErrorKind {
    /// No fn registered under the id → 404 `FN-NOT-FOUND`.
    NotFound,
    /// The args JSON did not convert → 400 `FN-BAD-REQUEST`.
    Wire,
    /// The fn returned `Err` → 400 `FN-THREW`.
    FnError,
    /// The fn panicked → 500 `FN-INTERNAL`.
    Panic,
    /// The call exceeded its budget → 504 `FN-TIMEOUT`.
    Timeout,
    /// The host itself failed (a closed semaphore, a dead task) →
    /// 500 `FN-INTERNAL`.
    Internal,
}

impl RegistryError {
    fn new(kind: RegistryErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// The HTTP status the pipeline maps the error to.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self.kind {
            RegistryErrorKind::NotFound => 404,
            RegistryErrorKind::Wire | RegistryErrorKind::FnError => 400,
            RegistryErrorKind::Panic | RegistryErrorKind::Internal => 500,
            RegistryErrorKind::Timeout => 504,
        }
    }

    /// The envelope code the pipeline maps the error to.
    #[must_use]
    pub fn envelope_code(&self) -> &'static str {
        match self.kind {
            RegistryErrorKind::NotFound => "FN-NOT-FOUND",
            RegistryErrorKind::Wire => "FN-BAD-REQUEST",
            RegistryErrorKind::FnError => "FN-THREW",
            RegistryErrorKind::Panic | RegistryErrorKind::Internal => "FN-INTERNAL",
            RegistryErrorKind::Timeout => "FN-TIMEOUT",
        }
    }
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.envelope_code(), self.message)
    }
}

impl std::error::Error for RegistryError {}

impl From<RawCallError> for RegistryError {
    fn from(e: RawCallError) -> Self {
        match e {
            RawCallError::NotFound(name) => Self::new(
                RegistryErrorKind::NotFound,
                format!("no native function named {name:?}"),
            ),
            RawCallError::FnError(message) => Self::new(RegistryErrorKind::FnError, message),
            RawCallError::Panic => Self::new(
                RegistryErrorKind::Panic,
                "the native function panicked (the library never unwinds across the boundary)",
            ),
        }
    }
}

/// The object-safe registry seam the dispatcher consult wires: `has`
/// answers before a JS dispatch, `call` runs the native fn with the
/// §9.4 route's args JSON and `types_json` feeds the `.d.ts` emitter.
#[async_trait::async_trait]
pub trait RustFnRegistry: Send + Sync {
    /// Whether the id is native-owned.
    fn has(&self, id: &str) -> bool;
    /// Run the fn: args JSON in, the fn's encoded output out.
    ///
    /// # Errors
    ///
    /// [`RegistryError`] — the kind carries the wire mapping.
    async fn call(&self, id: &str, args_json: &[u8]) -> Result<Vec<u8>, RegistryError>;
    /// The types payload of the first mounted library that has one.
    fn types_json(&self) -> Option<String>;
}

/// The host: the mounted libraries, the id → (library, entry) map and
/// the bounded blocking-call lane.
pub struct RustFnHost {
    libraries: Vec<Arc<RustFnLibrary>>,
    ids: BTreeMap<String, (usize, String)>,
    permits: Arc<Semaphore>,
    timeout: Duration,
}

impl Default for RustFnHost {
    fn default() -> Self {
        Self::new()
    }
}

impl RustFnHost {
    /// A host with the default lane count and the spec's 30 s budget.
    #[must_use]
    pub fn new() -> Self {
        Self::with_lanes(DEFAULT_LANES)
    }

    /// A host with `lanes` concurrent native calls (at least one).
    #[must_use]
    pub fn with_lanes(lanes: usize) -> Self {
        Self {
            libraries: Vec::new(),
            ids: BTreeMap::new(),
            permits: Arc::new(Semaphore::new(lanes.max(1))),
            timeout: Duration::from_secs(RUSTFN_TIMEOUT_SECS),
        }
    }

    /// Override the per-function call budget (the substrate's seam for
    /// a future config key; the spec default stays [`RUSTFN_TIMEOUT_SECS`]).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Mount a loaded library; returns its index for [`Self::register`].
    pub fn mount(&mut self, library: RustFnLibrary) -> usize {
        self.libraries.push(Arc::new(library));
        self.libraries.len() - 1
    }

    /// Point the §9.4 id at `(library index, entry name)`. The id is
    /// the same `server_fn_id(file, export, secret)` triple the client
    /// stubs call — re-registering an id under a newly mounted library
    /// is the hot-swap.
    pub fn register(
        &mut self,
        id: impl Into<String>,
        library_index: usize,
        fn_name: impl Into<String>,
    ) {
        self.ids.insert(id.into(), (library_index, fn_name.into()));
    }

    /// Whether the id is native-owned.
    #[must_use]
    pub fn has(&self, id: &str) -> bool {
        self.ids.contains_key(id)
    }

    /// The registered ids (sorted).
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.ids.keys().cloned().collect()
    }

    /// Whether anything is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The types payload of the first mounted library that exports one.
    #[must_use]
    pub fn types_json(&self) -> Option<String> {
        self.libraries
            .iter()
            .find_map(|lib| lib.types_json().map(str::to_owned))
    }

    /// The blocking call path (`call` wraps it in the permit +
    /// `spawn_blocking` + timeout trio; tests and the hot-swap pin use
    /// it directly).
    ///
    /// # Errors
    ///
    /// [`RegistryError`] with the wire mapping.
    pub fn call_now(&self, id: &str, args_json: &[u8]) -> Result<Vec<u8>, RegistryError> {
        let (library_index, name) = self
            .ids
            .get(id)
            .ok_or_else(|| {
                RegistryError::new(
                    RegistryErrorKind::NotFound,
                    format!("no native function registered under id {id:?}"),
                )
            })?
            .clone();
        let lib = self
            .libraries
            .get(library_index)
            .ok_or_else(|| {
                RegistryError::new(
                    RegistryErrorKind::NotFound,
                    format!("no native function registered under id {id:?}"),
                )
            })?
            .clone();
        let input = args_to_odf(args_json).map_err(|e| RegistryError {
            kind: RegistryErrorKind::Wire,
            message: e.to_string(),
        })?;
        lib.call_raw(&name, &input).map_err(RegistryError::from)
    }
}

/// Convert the §9.4 route's args JSON into the ODF bytes the boundary
/// carries (the JSON → ODF hop the host owns, so the client stub and
/// the C ABI both stay untouched).
fn args_to_odf(args_json: &[u8]) -> Result<Vec<u8>, OxideError> {
    let json: serde_json::Value = serde_json::from_slice(args_json).map_err(|e| {
        err(
            RFN_WIRE,
            format!("the native fn args did not parse as JSON: {e}"),
        )
    })?;
    let v = oxide_fw_odf::Value::from_json(&json);
    oxide_fw_odf::encode(&v)
}

#[async_trait::async_trait]
impl RustFnRegistry for RustFnHost {
    fn has(&self, id: &str) -> bool {
        RustFnHost::has(self, id)
    }

    fn types_json(&self) -> Option<String> {
        RustFnHost::types_json(self)
    }

    async fn call(&self, id: &str, args_json: &[u8]) -> Result<Vec<u8>, RegistryError> {
        // The permit bounds the blocking lanes; a wire error refuses
        // before it queues.
        let lookup = self.ids.get(id).cloned().ok_or_else(|| {
            RegistryError::new(
                RegistryErrorKind::NotFound,
                format!("no native function registered under id {id:?}"),
            )
        })?;
        let lib = self.libraries.get(lookup.0).cloned().ok_or_else(|| {
            RegistryError::new(
                RegistryErrorKind::NotFound,
                format!("no native function registered under id {id:?}"),
            )
        })?;
        let input = args_to_odf(args_json).map_err(|e| RegistryError {
            kind: RegistryErrorKind::Wire,
            message: e.to_string(),
        })?;
        let name = lookup.1.clone();
        let _permit = self.permits.clone().acquire_owned().await.map_err(|_| {
            RegistryError::new(RegistryErrorKind::Internal, "the native call lane closed")
        })?;
        let handle = tokio::task::spawn_blocking(move || lib.call_raw(&name, &input));
        match tokio::time::timeout(self.timeout, handle).await {
            Ok(Ok(result)) => result.map_err(RegistryError::from),
            Ok(Err(join)) => Err(RegistryError::new(
                RegistryErrorKind::Panic,
                format!("the native call task died: {join}"),
            )),
            Err(_elapsed) => Err(RegistryError::new(
                RegistryErrorKind::Timeout,
                format!(
                    "the native fn `{}` exceeded its {:.0}s budget",
                    lookup.1,
                    self.timeout.as_secs_f32()
                ),
            )),
        }
    }
}
