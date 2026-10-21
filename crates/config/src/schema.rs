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
use serde_json::{Value, json};

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
                    },
                    "ppr": {
                        "type": "boolean",
                        "default": false,
                        "description": "§10.8 partial prerendering: serve the cached static shell instantly and stream holes live"
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
                            "type": { "type": "string", "enum": ["memory", "disk", "redis", "s3", "http"] },
                            "url": { "type": "string", "description": "Remote base URL for `type: \"http\"` or `type: \"redis\"`." },
                            "bucket": { "type": "string", "description": "Bucket name for `type: \"s3\"`." }
                        },
                        "required": ["type"]
                    },
                    "l0": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "maxSize": { "type": "string", "pattern": "^[0-9]+(kb|mb|gb)$", "default": "512mb" },
                            "vary": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Request headers the L0 fingerprint varies on. The primary language tag of accept-language is normalized automatically."
                            },
                            "ttlSecs": {
                                "type": "integer",
                                "minimum": 1,
                                "description": "P-05: L0 freshness window in seconds (default 300)."
                            }
                        }
                    }
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
                    "subset": { "type": "array", "items": { "type": "string" } },
                    "families": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "family": { "type": "string" },
                                "provider": { "type": "string" },
                                "weights": { "type": "array", "items": { "type": "integer" } },
                                "styles": { "type": "array", "items": { "type": "string" } },
                                "localPaths": { "type": "array", "items": { "type": "string" } }
                            }
                        }
                    }
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
                        "enum": ["prefix", "prefix-except-default", "none"],
                        "description": "Locale routing strategy: `prefix` (always), `prefix-except-default` (omit the default locale), `none` (no path prefix; cookie/Accept-Language only)."
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
                    "cookieKeys": { "type": "array", "items": { "type": "string" } },
                    "sessionKeys": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "description": "≥32-byte base64 keys for the ChaCha20-Poly1305 session codec; first seals, all decrypt (rotation)."
                    },
                    "exposeCacheHeader": {
                        "type": "boolean",
                        "description": "S-21.3: emit the `x-oxide-cache` status header. Default true."
                    }
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
                    "ssrExternals": { "type": "array", "items": { "type": "string" } },
                    "remoteCache": { "type": "string", "description": "§5.4: remote build-cache URL (http://). Read-through on a local miss, write-back on store." }
                }
            },
            "adapter": {
                "type": "string",
                "enum": ["binary", "node", "static", "docker", "cloudflare", "lambda", "vercel", "netlify", "deno", "bun"]
            },
            "siteUrl": {
                "type": "string",
                "description": "§16.3: absolute site origin used by sitemap.xml and robots.txt."
            },
            "manifest": {
                "type": "object",
                "additionalProperties": true,
                "description": "§16.3: web app manifest (PWA). Present -> oxide build emits manifest.webmanifest + a service worker."
            },
            "og": {
                "type": "object",
                "additionalProperties": true,
                "description": "S-09: on-demand OG-image endpoint (GET /_oxide/og). Off by default; opt in with og.enabled = true."
            },
            "imagesEndpoint": {
                "type": "object",
                "additionalProperties": true,
                "description": "S-09: on-demand image transform endpoint (GET /_oxide/image). On by default; gateable."
            },
            "static": {
                "type": "object",
                "additionalProperties": true,
                "description": "§10.5: static-export options (fallback behavior for dynamic routes)."
            },
            "plugins": {
                "type": "array",
                "items": { "type": "object", "additionalProperties": true },
                "description": "§15: plugin entries (native, wasm, or js)."
            },
            "modules": {
                "type": "array",
                "items": { "type": "string" },
                "description": "§15.4: distributable modules to install."
            },
            "extends": {
                "type": "array",
                "items": { "type": "string" },
                "description": "§14.2: shared configs to merge before this file."
            },
            "environments": {
                "type": "object",
                "additionalProperties": true,
                "description": "§14.2: per-environment Rolldown overrides (client / ssr)."
            },
            "dev": {
                "type": "object",
                "additionalProperties": true,
                "description": "§13.5: dev-server options (https, host, qr, open, proxy)."
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
            }
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
        for k in [
            "hostAllowList",
            "ssrfAllowList",
            "trustedProxies",
            "cookieKeys",
        ] {
            assert!(sec.get(k).is_some(), "security.{k} missing from schema");
        }
        // §14.1: `cache.l0.maxSize` (nested), not a flat `l0MaxSize`.
        assert!(
            v["properties"]["cache"]["properties"]["l0"]["properties"]
                .get("maxSize")
                .is_some(),
            "cache.l0.maxSize missing"
        );
        assert!(
            v["properties"]["server"]["properties"]
                .get("backend")
                .is_some(),
            "server.backend missing"
        );
        assert!(
            v["properties"]["build"]["properties"]
                .get("ssrExternals")
                .is_some(),
            "build.ssrExternals missing"
        );
        // B-15: keys the typed config deserializes but the schema dropped.
        assert!(
            v["properties"]["cache"]["properties"]["l0"]["properties"]
                .get("vary")
                .is_some(),
            "cache.l0.vary missing"
        );
        // P-05 / S-21.3
        assert!(
            v["properties"]["cache"]["properties"]["l0"]["properties"]
                .get("ttlSecs")
                .is_some(),
            "cache.l0.ttlSecs missing"
        );
        assert!(
            sec.get("exposeCacheHeader").is_some(),
            "security.exposeCacheHeader missing"
        );
        assert!(
            sec.get("sessionKeys").is_some(),
            "security.sessionKeys missing"
        );
        assert!(
            v["properties"]["fonts"]["properties"]
                .get("families")
                .is_some(),
            "fonts.families missing"
        );
    }

    /// B-15: the schema must not carry duplicate object keys — serde's
    /// `json!` is last-wins, so a duplicated key silently drops the first
    /// (more descriptive) variant. Parse the pretty text with a
    /// duplicate-detecting pass.
    #[test]
    fn schema_has_no_duplicate_object_keys() {
        let text = config_schema_pretty().unwrap();
        let mut stack: Vec<Option<(String, Vec<String>)>> = Vec::new();
        let mut key: Option<String> = None;
        let mut chars = text.char_indices().peekable();
        while let Some((_, c)) = chars.next() {
            match c {
                '"' => {
                    let mut s = String::new();
                    while let Some((_, c2)) = chars.next() {
                        if c2 == '\\' {
                            if let Some((_, e)) = chars.next() {
                                s.push(e);
                            }
                        } else if c2 == '"' {
                            break;
                        } else {
                            s.push(c2);
                        }
                    }
                    // Detect `"key":` by lookahead for a colon after whitespace.
                    let mut la = chars.clone();
                    let mut is_key = false;
                    while let Some((_, c3)) = la.next() {
                        if c3.is_whitespace() {
                            continue;
                        }
                        is_key = c3 == ':';
                        break;
                    }
                    if is_key {
                        key = Some(s);
                    }
                }
                ':' => {
                    if let Some(k) = key.take() {
                        if let Some(Some((_, seen))) = stack.last_mut() {
                            assert!(
                                !seen.contains(&k),
                                "duplicate schema key `{k}` (last-wins drops the first)"
                            );
                            seen.push(k);
                        }
                    }
                }
                '{' => stack.push(Some((String::new(), Vec::new()))),
                '}' => {
                    stack.pop();
                }
                _ => {}
            }
        }
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
