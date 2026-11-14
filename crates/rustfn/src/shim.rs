//! The panic-safe shim every `#[server_fn]` generated entry delegates
//! to (spec §V2-F08: "Panics are caught with `catch_unwind` and mapped
//! to status 2 — never unwind across the boundary"). The closure the
//! macros generate does the typed work (decode the ODF input, call the
//! function, encode the result); this module owns the statuses, the
//! output buffer discipline and the loud panic record.

use crate::abi::{RawBuf, STATUS_ERROR, STATUS_OK, STATUS_PANIC};
use crate::bridge::encode_error_message;

/// The code the panic record carries (`RFN-PANIC`): a user function
/// panicked; the host maps the call to a 500 and the library never
/// unwinds across the C boundary.
pub const RFN_PANIC: &str = "RFN-PANIC";

/// Run one shim: `f` receives the raw input bytes and returns either
/// the already-encoded output bytes or the error message. The three
/// outcomes map to the ABI statuses — `Ok` → status 0 with the buffer
/// filled, `Err` → status 1 with an ODF-string payload, a panic →
/// status 2 with the buffer left empty (recorded loud, `RFN-PANIC`).
/// A payload-encode failure inside `Ok`/`Err` degrades to the panic
/// status: the host never sees a status without its buffer contract.
pub fn run_shim<F>(input: &[u8], out: &mut RawBuf, f: F) -> i32
where
    F: FnOnce(&[u8]) -> Result<Vec<u8>, String>,
{
    // AssertUnwindSafe: the closure borrows the user function's state;
    // a panic leaves nothing to poison (the whole call is one shot).
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(input))) {
        Ok(Ok(bytes)) => {
            *out = RawBuf::alloc(&bytes);
            STATUS_OK
        }
        Ok(Err(message)) => match encode_error_message(&message) {
            Ok(payload) => {
                *out = RawBuf::alloc(&payload);
                STATUS_ERROR
            }
            // The message itself failed to encode (a broken wire is a
            // host bug, not the user fn's): the panic status keeps the
            // status/buffer contract intact.
            Err(e) => {
                record_panic(&format!("error payload failed to encode: {e}"));
                STATUS_PANIC
            }
        },
        Err(panic) => {
            record_panic(&panic_message(&panic));
            STATUS_PANIC
        }
    }
}

/// The panic record (`RFN-PANIC`, the house warn convention): the
/// payload text is best-effort — a non-string panic payload has no
/// message to print.
fn record_panic(detail: &str) {
    tracing::warn!(
        code = RFN_PANIC,
        detail = %detail,
        "a native server function panicked; the host answers 500 and the library never unwinds across the boundary"
    );
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "the panic payload was not a string".to_string()
    }
}
