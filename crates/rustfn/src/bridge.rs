//! The ODF bridge (decision D33): "the calling convention is a C ABI
//! over ODF bytes". A typed payload crosses the boundary as ODF — the
//! shim decodes its input and encodes its result through these
//! helpers, and the host converts the JSON args the §9.4 route
//! receives before the call. `serde` rides the ODF JSON interop
//! ([`oxide_fw_odf::Value::from_json`] / [`oxide_fw_odf::Value::to_json`]),
//! so the wire stays the ODF1 envelope the browser decoder already
//! speaks.

use oxide_fw_diagnostics::OxideError;
use oxide_fw_odf::Value;
use serde::{Serialize, de::DeserializeOwned};

use crate::err;

/// The payload code (`RFN-004`): a value did not convert between its
/// Rust type and the ODF bytes — the message carries the serde/ODF
/// cause, the fix names the generated types.
pub const RFN_WIRE: &str = "RFN-004";

/// Encode `value` as the ODF bytes a shim (or the host) puts on the
/// boundary.
///
/// # Errors
///
/// [`RFN_WIRE`] when serde cannot serialize the value (an ODF encode
/// failure rides its own `ODF-*` code).
pub fn encode_odf<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, OxideError> {
    let json = serde_json::to_value(value).map_err(|e| {
        err(
            RFN_WIRE,
            format!("native function payload did not convert: {e}"),
        )
    })?;
    let v = Value::from_json(&json);
    oxide_fw_odf::encode(&v)
}

/// Decode the ODF bytes a shim (or the host) received into `T`.
///
/// # Errors
///
/// [`RFN_WIRE`] when the bytes are not ODF or the shape does not fit
/// `T` (the cause rides the message).
pub fn decode_odf<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, OxideError> {
    let v = oxide_fw_odf::decode(bytes).map_err(|e| {
        err(
            RFN_WIRE,
            format!("native function payload did not convert: {e}"),
        )
    })?;
    let json = v.to_json();
    serde_json::from_value(json).map_err(|e| {
        err(
            RFN_WIRE,
            format!("native function payload did not convert: {e}"),
        )
    })
}

/// Encode a function-level error message (status 1): an ODF string,
/// so the host's `decode_error_message` never guesses at a wire shape.
///
/// # Errors
///
/// An ODF encode failure rides its own `ODF-*` code.
pub fn encode_error_message(message: &str) -> Result<Vec<u8>, OxideError> {
    oxide_fw_odf::encode(&Value::String(message.to_string()))
}

/// Decode a status-1 payload into the error message.
///
/// # Errors
///
/// [`RFN_WIRE`] when the payload is not an ODF string (the host never
/// invents a message it did not receive).
pub fn decode_error_message(bytes: &[u8]) -> Result<String, OxideError> {
    match oxide_fw_odf::decode(bytes)? {
        Value::String(s) => Ok(s),
        other => Err(err(
            RFN_WIRE,
            format!(
                "native function error payload was {:?}, not a string",
                other.to_json()
            ),
        )),
    }
}
