#![doc = include_str!("../README.md")]

//! JSON Schema generation for `oxide.config.ts`.
//!
//! The schema this crate emits is the contract between a user's
//! `oxide.config.ts` and their editor: the `$schema` field points at
//! the schema URL, and the editor validates the config against it
//! in real time. It is also what `oxide schema` prints on stdout.

use oxide_fw_diagnostics::OxideError;

/// The canonical schema URL for `oxide.config.ts`.
pub const SCHEMA_URL: &str = "https://oxide.dev/config.schema.json";

/// The error codes this crate surfaces. `SCHEMA-001` wraps a config-crate
/// failure; `SCHEMA-002` means the generated schema was not parseable JSON
/// (a serde bug class, not user input).
pub const CODE_CONFIG_FAILURE: &str = "SCHEMA-001";
pub const CODE_NOT_JSON: &str = "SCHEMA-002";

/// Wrap a config-crate failure into this crate's user-visible error shape.
/// Extracted so the mapping (code + context) is testable without forcing the
/// infallible happy path to fail.
fn wrap_config_error(e: &OxideError) -> OxideError {
    OxideError::new(
        CODE_CONFIG_FAILURE,
        format!("config schema: {}", e.0.message),
    )
}

/// Return the JSON Schema for `oxide.config.ts` as a pretty-printed
/// JSON string.
///
/// Delegates to [`oxide_fw_config::config_schema_pretty`] so there is
/// exactly one source of truth for the schema shape.
///
/// # Errors
///
/// `SCHEMA-001` when the config crate cannot serialize its schema
/// (a build/dependency bug class, not user input).
pub fn schema_pretty() -> Result<String, OxideError> {
    oxide_fw_config::config_schema_pretty().map_err(|e| wrap_config_error(&e))
}

/// Return the JSON Schema as a [`serde_json::Value`].
///
/// # Errors
///
/// `SCHEMA-002` when the generated schema is not parseable JSON.
pub fn schema_value() -> Result<serde_json::Value, OxideError> {
    let s = schema_pretty()?;
    serde_json::from_str(&s)
        .map_err(|e| OxideError::new(CODE_NOT_JSON, format!("schema is not valid JSON: {e}")))
}

/// The `$schema` URL a config file should reference.
#[must_use]
pub fn schema_url() -> &'static str {
    SCHEMA_URL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_valid_json() {
        let v = schema_value().expect("schema_value");
        assert!(v.is_object());
        assert!(v.get("properties").is_some());
    }

    #[test]
    fn schema_pretty_roundtrips() {
        let s = schema_pretty().expect("schema_pretty");
        let _: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
    }

    #[test]
    fn schema_url_is_a_url() {
        assert!(schema_url().starts_with("https://"));
    }

    #[test]
    fn config_failures_wrap_as_schema_001_with_context() {
        let e = wrap_config_error(&OxideError::new(
            "CFG-SCHEMA",
            "schema serialization failed: x",
        ));
        assert_eq!(e.0.code, CODE_CONFIG_FAILURE);
        assert_eq!(e.0.message, "config schema: schema serialization failed: x");
    }
}
