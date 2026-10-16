//! JSON Schema for `oxide.config.{ts,json}` (spec §19.3: "JSON Schema for
//! config").
//!
//! The schema is generated from the typed `OxideConfig` surface, so editor
//! tooling (VS Code's JSON schema support) gets accurate completion and
//! validation without the user running any code. The schema is deliberately
//! hand-shaped rather than reflected from serde because the config is
//! camelCase-renamed and has nested types with distinct default semantics —
//! writing it explicitly keeps it legible and stable across crate versions.

use crate::Result;
use serde_json::{json, Value};

/// The `$schema` URI written into the config schema file. Editors recognise
/// the `$schema` field and use this URI to fetch the schema.
pub const CONFIG_SCHEMA_URI: &str = "https://oxide.dev/schema/oxide-config-v1.json";

/// Produce the JSON Schema document. Deterministic; identical bytes for a
/// given crate version, so it can be committed to a repo and diffed.
pub fn config_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": CONFIG_SCHEMA_URI,
        "title": "Oxide config",
        "description": "Typed configuration for an Oxide project (spec §14).",
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "framework": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string" },
                    "adapter": {
                        "type": "string",
                        "enum": ["react", "vue", "svelte", "solid"]
                    },
                    "compiler": { "type": "boolean" }
                }
            },
            "routes": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "dir": { "type": "string", "default": "src/routes" }
                }
            },
            "server": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "port": { "type": "integer", "minimum": 0, "maximum": 65535, "default": 3000 },
                    "host": { "type": "string", "default": "localhost" },
                    "compression": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["zstd", "br", "gzip"] }
                    },
                    "http3": { "type": "boolean", "default": false },
                    "backend": { "type": "string", "enum": ["node", "v8"], "default": "node" }
                }
            },
            "render": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "default": {
                        "type": "string",
                        "enum": ["auto", "static", "isr", "dynamic", "spa", "islands"]
                    }
                }
            },
            "cache": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "handler": {
                        "type": "object",
                        "properties": {
                            "type": { "type": "string", "enum": ["memory", "disk", "redis", "s3", "http"] }
                        }
                    },
                    "l0MaxSize": { "type": "string", "pattern": "^[0-9]+(kb|mb|gb)$", "default": "512mb" }
                }
            },
            "images": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "formats": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["avif", "webp", "jpeg", "png"] }
                    },
                    "remotePatterns": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": { "host": { "type": "string" } },
                            "required": ["host"]
                        }
                    }
                }
            },
            "fonts": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "providers": { "type": "array", "items": { "type": "string" } },
                    "subset": { "type": "array", "items": { "type": "string" } }
                }
            },
            "i18n": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "locales": { "type": "array", "items": { "type": "string" } },
                    "default": { "type": "string" },
                    "strategy": {
                        "type": "string",
                        "enum": ["prefix-except-default", "prefix-always", "prefix-never"]
                    }
                }
            },
            "security": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "csp": { "type": "string", "enum": ["strict", "hash", "off"] },
                    "csrf": { "type": "string", "enum": ["origin", "token", "off"] },
                    "hostAllowList": { "type": "array", "items": { "type": "string" } },
                    "ssrfAllowList": { "type": "array", "items": { "type": "string" } },
                    "trustedProxies": { "type": "array", "items": { "type": "string" } },
                    "cookieKeys": { "type": "array", "items": { "type": "string" } }
                }
            },
            "rules": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": true,
                    "properties": {
                        "source": { "type": "string" },
                        "destination": { "type": "string" },
                        "redirect": { "type": "string" },
                        "status": { "type": "integer" }
                    }
                }
            },
            "budgets": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "clientJs": { "type": "string" },
                    "routeJs": { "type": "string" }
                }
            },
            "build": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "target": { "type": "string" },
                    "sourcemap": { "type": "string", "enum": ["hidden", "inline", "external", "off"] },
                    "ssrExternals": { "type": "array", "items": { "type": "string" } }
                }
            },
            "adapter": {
                "type": "string",
                "enum": ["binary", "node", "static", "docker", "cloudflare", "lambda", "vercel", "netlify", "deno", "bun"]
            },
            "content": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "collections": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["name", "dir"],
                            "properties": {
                                "name": { "type": "string" },
                                "dir": { "type": "string" },
                                "indexedFields": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                }
                            }
                        }
                    }
                }
            },
            "static": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "fallback": {
                        "type": ["string", "null"],
                        "description": "Path to a fallback HTML file emitted for dynamic routes without prerendered output (§10.5)."
                    }
                }
            },
            "plugins": { "type": "array" },
            "modules": { "type": "array", "items": { "type": "string" } },
            "extends": { "type": "array", "items": { "type": "string" } },
            "environments": { "type": "object" }
        }
    })
}

/// Serialize the schema as pretty JSON with a trailing newline, ready to
/// write to `.oxide/schema/config.json` or a project's committed schema file.
pub fn config_schema_pretty() -> Result<String> {
    let value = config_schema();
    let s = serde_json::to_string_pretty(&value)
        .map_err(|e| crate::err("CFG-SCHEMA", format!("schema serialization failed: {e}")))?;
    Ok(format!("{s}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_valid_json_with_expected_top_level_keys() {
        let v = config_schema();
        assert_eq!(v["type"], "object");
        let props = v["properties"].as_object().unwrap();
        for expected in [
            "framework",
            "routes",
            "server",
            "render",
            "cache",
            "images",
            "fonts",
            "i18n",
            "security",
            "rules",
            "budgets",
            "build",
            "adapter",
            "static",
            "plugins",
            "modules",
        ] {
            assert!(props.contains_key(expected), "missing {expected}");
        }
    }

    /// BUG-11: guard against schema drift — the keys the typed config
    /// actually deserializes must appear in the schema, or editor tooling
    /// silently rejects a valid config.
    #[test]
    fn schema_declares_the_security_and_cache_keys() {
        let v = config_schema();
        let sec = &v["properties"]["security"]["properties"];
        for k in ["hostAllowList", "ssrfAllowList", "trustedProxies", "cookieKeys"] {
            assert!(sec.get(k).is_some(), "security.{k} missing from schema");
        }
        // `cache.l0MaxSize` is a top-level key on `cache`, not a nested `l0`.
        assert!(
            v["properties"]["cache"]["properties"]
                .get("l0MaxSize")
                .is_some(),
            "cache.l0MaxSize missing"
        );
        assert!(
            v["properties"]["server"]["properties"].get("backend").is_some(),
            "server.backend missing"
        );
        assert!(
            v["properties"]["build"]["properties"]
                .get("ssrExternals")
                .is_some(),
            "build.ssrExternals missing"
        );
    }

    #[test]
    fn schema_enum_contains_every_adapter() {
        let v = config_schema();
        let values: Vec<&str> = v["properties"]["adapter"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str())
            .collect();
        for expected in ["binary", "node", "static", "docker", "cloudflare"] {
            assert!(values.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn schema_is_deterministic_across_calls() {
        let a = config_schema_pretty().unwrap();
        let b = config_schema_pretty().unwrap();
        assert_eq!(a, b);
        assert!(a.ends_with('\n'));
    }

    #[test]
    fn schema_static_fallback_is_nullable_string() {
        let v = config_schema();
        let types = v["properties"]["static"]["properties"]["fallback"]["type"]
            .as_array()
            .unwrap();
        assert!(types.iter().any(|t| t == "string"));
        assert!(types.iter().any(|t| t == "null"));
    }
}
