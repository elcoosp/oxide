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

/// Return the JSON Schema for `oxide.config.ts` as a pretty-printed
/// JSON string.
///
/// Delegates to [`oxide_fw_config::config_schema_pretty`] so there is
/// exactly one source of truth for the schema shape.
pub fn schema_pretty() -> Result<String, OxideError> {
    oxide_fw_config::config_schema_pretty()
        .map_err(|e| OxideError::new("SCHEMA-001", format!("config schema: {}", e.0.message)))
}

/// Return the JSON Schema as a [`serde_json::Value`].
pub fn schema_value() -> Result<serde_json::Value, OxideError> {
    let s = schema_pretty()?;
    serde_json::from_str(&s)
        .map_err(|e| OxideError::new("SCHEMA-002", format!("schema is not valid JSON: {e}")))
}

/// The `$schema` URL a config file should reference.
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
}
