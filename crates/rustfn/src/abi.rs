//! The C ABI the `*.server.rs` cdylibs speak (spec §V2-F08 verbatim):
//! the buffer, entry and table shapes, the ABI version the host gates
//! on and the shim statuses. No Rust types or allocators cross the
//! boundary — every payload is bytes, every allocation the host hands
//! back through the table's `free` (decision D33).

use std::ffi::c_char;

/// The ABI version the host gates on (`RFN-001` on mismatch). Bumped
/// only when a shape in this module changes.
pub const ABI_VERSION: u32 = 1;

/// The shim statuses: `0` ok, `1` the function returned `Err`, `2` the
/// function panicked (never unwound across the boundary).
pub const STATUS_OK: i32 = 0;
/// The shim status for a function-level `Err`.
pub const STATUS_ERROR: i32 = 1;
/// The shim status for a caught panic.
pub const STATUS_PANIC: i32 = 2;

/// The spec-verbatim output buffer (`#[repr(C)]`): the shim fills it
/// with one allocation; the host copies the bytes out and hands the
/// buffer back through the table's `free` so the *library's* allocator
/// always owns the release.
#[repr(C)]
pub struct RawBuf {
    /// The allocation's first byte; null only when `cap == 0`.
    pub ptr: *mut u8,
    /// The payload length in bytes.
    pub len: usize,
    /// The allocation's capacity in bytes.
    pub cap: usize,
}

impl RawBuf {
    /// The empty buffer a shim starts with (null-free: `len == 0` and
    /// the host copies nothing).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
            cap: 0,
        }
    }

    /// Allocate a buffer holding exactly `bytes` (the allocation the
    /// generated shims hand back; the table's `free` releases it).
    #[must_use]
    pub fn alloc(bytes: &[u8]) -> Self {
        if bytes.is_empty() {
            return Self::empty();
        }
        let mut owned = bytes.to_vec();
        let ptr = owned.as_mut_ptr();
        let len = owned.len();
        let cap = owned.capacity();
        std::mem::forget(owned);
        Self { ptr, len, cap }
    }

    /// The payload bytes. Zero-length buffers have no dereferenceable
    /// memory, so they yield an empty slice.
    ///
    /// # Safety
    ///
    /// The buffer must have come from [`RawBuf::alloc`] (or the
    /// library's equivalent) and must not have been released yet.
    #[must_use]
    pub unsafe fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            &[]
        } else {
            // SAFETY: the caller guarantees the allocation is live and
            // un-released (the contract on this method).
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    /// Release the buffer through the *library's* free function — the
    /// host never deallocates a shim allocation with its own
    /// allocator (the two sides may differ).
    ///
    /// # Safety
    ///
    /// The buffer must have come from the library whose `free` is
    /// passed here, and must be released exactly once.
    pub unsafe fn release_via(self, free: extern "C" fn(RawBuf)) {
        free(self);
    }
}

/// One registered function: its name (a NUL-terminated string that
/// lives as long as the library) and its C entry point.
#[repr(C)]
pub struct FnEntry {
    /// The NUL-terminated export name.
    pub name: *const c_char,
    /// The shim: input bytes in, the output buffer filled, the status
    /// returned ([`STATUS_OK`] / [`STATUS_ERROR`] / [`STATUS_PANIC`]).
    pub call: extern "C" fn(*const u8, usize, *mut RawBuf) -> i32,
}

impl FnEntry {
    /// The entry's name. The name pointer lives as long as the loaded
    /// library (a leaked literal the macros generated), so the borrow
    /// is tied to the caller's guarantee, not to `&self`.
    ///
    /// # Safety
    ///
    /// `name` must be a valid NUL-terminated string (the macros and
    /// [`leak_name`] guarantee it; a hand-built table must too).
    #[must_use]
    pub unsafe fn name_str(&self) -> String {
        // SAFETY: the caller guarantees `name` is NUL-terminated (the
        // contract on this method).
        unsafe { std::ffi::CStr::from_ptr(self.name) }
            .to_string_lossy()
            .into_owned()
    }
}

/// The spec-verbatim table the cdylib exports as `oxide_fn_table`:
/// the ABI version, the entry list and the `free` that releases shim
/// allocations.
#[repr(C)]
pub struct FnTable {
    /// The ABI this table speaks ([`ABI_VERSION`]); the host refuses a
    /// foreign version with `RFN-001`.
    pub abi_version: u32,
    /// How many entries `entries` points at.
    pub len: usize,
    /// The entry array (`len` elements).
    pub entries: *const FnEntry,
    /// Releases a buffer a shim allocated.
    pub free: extern "C" fn(RawBuf),
}

// SAFETY: the ABI shapes are immutable wire data — the names are
// process-lifetime strings, the call/free pointers the generated
// (or hand-built, same contract) shims. Sharing them across threads
// is the whole point of the table; concurrent calls are the same
// reentrancy contract any server handler has.
unsafe impl Send for RawBuf {}
unsafe impl Sync for RawBuf {}
unsafe impl Send for FnEntry {}
unsafe impl Sync for FnEntry {}
unsafe impl Send for FnTable {}
unsafe impl Sync for FnTable {}

/// Leak a NUL-terminated copy of `name` for a dynamically-built entry
/// (the generated statics use `c"…"` literals instead — const). The
/// leak is deliberate and process-lifetime, the macro tables'
/// convention: entry names are read for as long as the library stays
/// mounted.
#[must_use]
pub fn leak_name(name: &'static str) -> *const c_char {
    // An interior NUL cannot appear in a Rust identifier; the empty
    // name keeps a hand-built table from dereferencing garbage.
    match std::ffi::CString::new(name) {
        Ok(c) => c.into_raw().cast_const(),
        Err(_) => std::ffi::CString::default().into_raw().cast_const(),
    }
}
