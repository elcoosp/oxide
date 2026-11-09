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
pub const CONFIG_SCHEMA_URI: &str = "https://oxide.dev/schema/oxide-fw-config-v1.json";

/// Produce the JSON Schema document. Deterministic; identical bytes for a
/// given crate version (pinned by `tests/schema_stability.rs` and the
/// checked-in `schemas/oxide-fw-config.schema.json`).
#[must_use]
pub fn config_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": CONFIG_SCHEMA_URI,
        "title": "Oxide config",
        "description": "Typed configuration for an Oxide project (spec §14).",
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "framework": framework_schema(),
            "routes": routes_schema(),
            "server": server_schema(),
            "render": render_schema(),
            "cache": cache_schema(),
            "images": images_schema(),
            "fonts": fonts_schema(),
            "i18n": i18n_schema(),
            "security": security_schema(),
            "rules": rules_schema(),
            "budgets": budgets_schema(),
            "build": build_schema(),
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
            "sw": sw_schema(),
            "og": og_schema(),
            "imagesEndpoint": {
                "type": "object",
                "additionalProperties": true,
                "description": "S-09: on-demand image transform endpoint (GET /_oxide/image). On by default; gateable."
            },

            "experiments": experiments_schema(),
            "live": live_schema(),
            "stream": stream_schema(),
            "brownout": brownout_schema(),

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
            "content": content_schema(),
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

/// Schema fragment for the `framework` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn framework_schema() -> Value {
    json!({
    "type": "object",
    "additionalProperties": false,
    "properties": {
                "name": { "type": "string" },
                "adapter": {
                    "type": "string",
                    "enum": ["react", "vue", "svelte", "solid"]
                },
                "compiler": { "type": "boolean" },
                "islands": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "§10.3 islands registry: component name → source file. The CLI emits a synthetic __boot__ client entry that imports each entry; `<Island chunk=\"X\">` on a page resolves to X in this map."
                }
            }
        })
}

/// Schema fragment for the `routes` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn routes_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "dir": { "type": "string", "default": "src/routes" }
        }
    })
}

/// Schema fragment for the `server` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn server_schema() -> Value {
    json!({
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
            "h3Advertise": { "type": "boolean", "default": true, "description": "§12.1 / C3: emit the `Alt-Svc: h3` header on the TLS listener. Default true when http3 is enabled; set false to keep the QUIC listener silent while browsers are not ready for it." },
            "backend": { "type": "string", "enum": ["node", "v8"], "default": "node" }
        }
    })
}

/// Schema fragment for the `render` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn render_schema() -> Value {
    json!({
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
    })
}

/// Schema fragment for the `cache` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn cache_schema() -> Value {
    json!({
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
    })
}

/// Schema fragment for the `images` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn images_schema() -> Value {
    json!({
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
            },
            "maxConcurrency": {
                "type": "integer",
                "minimum": 1,
                "default": 4,
                "description": "§16.1: maximum concurrent rasterizations. Each transform is CPU-bound; unbounded concurrency lets a few clients saturate every core."
            },
            "maxDimension": {
                "type": "integer",
                "minimum": 1,
                "default": 8192,
                "description": "§16.1: largest accepted `w`/`h`. A user-supplied size is not trusted to allocate an arbitrarily large canvas."
            }
        }
    })
}

/// Schema fragment for the `fonts` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn fonts_schema() -> Value {
    json!({
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
    })
}

/// Schema fragment for the `i18n` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn i18n_schema() -> Value {
    json!({
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
    })
}

/// Schema fragment for the `security` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn security_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
                "sessionTtlSecs": { "type": "integer", "minimum": 60, "default": 1800, "description": "§16.6: session cookie lifetime in seconds." },
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
                "description": "S-21.3: emit the `x-oxide-fw-cache` status header. Default true."
            }
        }
    })
}

/// Schema fragment for the `rules` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn rules_schema() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "additionalProperties": true,
            "properties": {
                "source": { "type": "string" },
                "destination": { "type": "string" },
                "redirect": { "type": "string" },
                "status": { "type": "integer" },
                "priority": { "type": "string", "enum": ["low", "normal", "critical"], "default": "normal", "description": "V2-F12: the route's overload priority; low-priority routes shed at brownout level Shed, everything but critical at Emergency (DSP-113 rejects other spellings)." }
            }
        }
    })
}

/// Schema fragment for the `budgets` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn budgets_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "clientJs": { "type": "string" },
            "routeJs": { "type": "string" }
        }
    })
}

/// Schema fragment for the `build` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn build_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "target": { "type": "string" },
            "sourcemap": { "type": "string", "enum": ["hidden", "inline", "external", "off"] },
            "ssrExternals": { "type": "array", "items": { "type": "string" } },
            "edgeExternals": { "type": "array", "items": { "type": "string" }, "description": "§18 / D18: specifiers left external in the edge bundle. Empty by default — the Worker must be self-contained unless it declares a node-compat layer." },
            "remoteCache": { "type": "string", "description": "§5.4: remote build-cache URL (http://). Read-through on a local miss, write-back on store." },
            "writeBranches": { "type": "array", "items": { "type": "string" }, "description": "§5.4 / SPEC-5D: git branches trusted to write the shared remote cache. Default main/master + release/* when remoteCache is set." },
            "routeSearch": { "type": "boolean", "default": true, "description": "§16.5: emit the route-search index (dist/client/search-index.json). On by default when prerendered pages exist; set false for projects that do not ship a client-side search UI." }
        }
    })
}

/// Schema fragment for the `sw` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn sw_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "enabled": { "type": "boolean", "description": "§16.6: emit the service worker at build. Default true when `manifest` is present; an explicit `false` disables emission even with a manifest." },
            "offlineFallback": { "type": "string", "description": "§16.6: the offline-fallback route the worker serves for a navigation miss. Default `/offline` when the build produced that route; otherwise `/`." }
        },
        "description": "§16.6: service worker configuration. Present only when the project wants to control emission beyond the default (manifest presence)."
    })
}

/// Schema fragment for the `og` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn og_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
        "description": "S-09: on-demand OG-image endpoint (GET /_oxide/og). Off by default; opt in with og.enabled = true.",
        "properties": {
            "enabled": { "type": "boolean", "default": false },
            "rateLimit": {
                "type": "object",
                "additionalProperties": false,
                "description": "A3: per-peer token-bucket limit applied before rasterization.",
                "properties": {
                    "max": { "type": "integer", "minimum": 0, "default": 10 },
                    "windowSecs": { "type": "integer", "minimum": 0, "default": 60 }
                }
            }
        }
    })
}

/// Schema fragment for the `content` section (extracted so
/// [`config_schema`] stays legible; the JSON is identical to
/// the inline literal it replaced).
#[must_use]
fn content_schema() -> Value {
    json!({
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
    })
}

/// Schema fragment for the `experiments` section (V2-F04, §15.4 — the
/// oneOf mirror of the internally-tagged [`StickyConfig`](crate::StickyConfig)
/// serde shape).
#[must_use]
fn experiments_schema() -> Value {
    json!({
        "type": "array",
        "description": "V2-F04 / §15.4: A/B experiments evaluated natively per request before the L0 lookup, so an experiment is a cache-key dimension (the |exp: vary suffix) instead of forcing the route dynamic. Validation (DSP-110): traffic is a 0-100 percentage and each experiment needs at least two variants. Off by default - with no experiments configured every L0 key stays byte-identical to the pre-F04 baseline.",
        "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["id", "traffic", "sticky", "variants"],
            "properties": {
                "id": { "type": "string", "description": "The experiment id - also the locals key and the vary-suffix name." },
                "traffic": { "type": "integer", "minimum": 0, "maximum": 100, "description": "Percentage of eligible identities in the experiment (DSP-110)." },
                "sticky": {
                    "oneOf": [
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["sticky"],
                            "properties": {
                                "sticky": { "const": "cookie" },
                                "cookieName": { "type": "string", "description": "The cookie storage key, e.g. oxide_hero." }
                            }
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["sticky"],
                            "properties": {
                                "sticky": { "const": "header" },
                                "headerName": { "type": "string", "description": "The request header storage key, e.g. x-bucket." }
                            }
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["sticky"],
                            "properties": {
                                "sticky": { "const": "ip-hash" }
                            }
                        }
                    ]
                },
                "variants": { "type": "array", "minItems": 2, "items": { "type": "string" }, "description": "Variant names, in config order (DSP-110: at least two)." },
                "routes": { "type": "array", "items": { "type": "string" }, "description": "URL-pattern prefixes the experiment applies to; empty = all routes." }
            }
        }
    })
}

/// Schema fragment for the `live` section (V2-F05 — the live-loader
/// transport knobs: hub capacity, cadence, per-IP security cap, transport).
fn live_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "V2-F05: live-loader transport knobs (the SSE hub's capacity, cadence and transport). Defaults are inert - with no route opting in (page.server.ts live: true) nothing listens and no hub is constructed. Validation (DSP-111): keepaliveSeconds >= 1, maxSubscribers >= 1, maxRerunsPerSecond >= 1, perIpMax <= maxSubscribers.",
        "properties": {
            "maxSubscribers": { "type": "integer", "minimum": 1, "default": 5000, "description": "The hub's total subscriber capacity; the endpoint answers 503 + retry-after above it." },
            "debounceMs": { "type": "integer", "minimum": 0, "default": 50, "description": "Per-subscriber rerun debounce in ms." },
            "keepaliveSeconds": { "type": "integer", "minimum": 1, "default": 20, "description": "SSE keepalive ping period in seconds (DSP-111 rejects 0)." },
            "maxRerunsPerSecond": { "type": "integer", "minimum": 1, "default": 20, "description": "Per-subscriber rerun rate cap per second." },
            "perIpMax": { "type": "integer", "minimum": 1, "default": 20, "description": "Per-peer subscriber cap (security); cannot exceed maxSubscribers." },
            "transport": { "enum": ["sse", "poll"], "default": "sse", "description": "sse streams snapshots over GET /__oxide/live; poll is the serverless fallback." }
        }
    })
}

/// Schema fragment for the `stream` section (V2-F07 — the resumable-stream
/// bounds: ttl, per-stream cap, store-wide cap).
fn stream_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "V2-F07: resumable-stream bounds. A streamed response marked x-oxide-stream stays replayable for the ttl at GET /__oxide/stream/<id> with Range: bytes=<n>-. Defaults are inert - the store only exists while a resumable response is being teed. Size literals use the section-14 grammar (b|kb|mb|gb|tb, plain digits are bytes). Validation (DSP-112): ttlSecs >= 1, maxBytesEach >= 1 byte, maxBytesTotal >= maxBytesEach; a bad literal fails with CFG-SIZE.",
        "properties": {
            "resume": {
                "type": "object",
                "additionalProperties": false,
                "description": "The reattach/replay bounds.",
                "properties": {
                    "ttlSecs": { "type": "integer", "minimum": 1, "default": 60, "description": "How long a buffer stays reattachable, in seconds (DSP-112 rejects 0)." },
                    "maxBytesEach": { "type": "string", "default": "4mb", "description": "Per-stream replay cap; past it the buffer loses replay and the client falls back to a fresh fetch." },
                    "maxBytesTotal": { "type": "string", "default": "256mb", "description": "Store-wide cap; the accounting evicts the oldest completed buffer first, then the oldest buffer." }
                }
            }
        }
    })
}

/// Schema fragment for the `brownout` section (V2-F12 — the governor's
/// breach ladders, the stale ceiling and the opt-in hedger).
fn brownout_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "V2-F12: brownout mode. Under overload the governor degrades deliberately - stale content past the SWR window behind warning 110, low-priority routes shed with 503 + x-oxide-brownout, readiness flips to 503 at Emergency. Off by default - with enabled false no governor is constructed. Validation (DSP-113): ascending threshold ladders, a staleCeiling multiplier >= 1, hedge.afterFactor > 1.",
        "properties": {
            "enabled": { "type": "boolean", "default": false, "description": "Master switch; false keeps the request path at its pre-F12 shape." },
            "staleCeiling": { "type": "string", "default": "10x", "description": "How far past the TTL an L0 entry may serve at level Stale, as a multiplier: '10x', plain '10', or fractional '1.5x' (DSP-113 rejects < 1)." },
            "thresholds": {
                "type": "object",
                "additionalProperties": false,
                "description": "The three breach ladders; rung i is the level-(i+1) threshold, a signal at or above it breaches that level. Ladders must be ascending (DSP-113).",
                "properties": {
                    "queueWaitMs": { "type": "array", "items": { "type": "integer", "minimum": 0 }, "minItems": 3, "maxItems": 3, "default": [50, 200, 500], "description": "Pool queue-wait p99 rungs in ms (spec defaults)." },
                    "loopLagMs": { "type": "array", "items": { "type": "integer", "minimum": 0 }, "minItems": 3, "maxItems": 3, "default": [20, 100, 300], "description": "Event-loop lag rungs in ms (spec defaults)." },
                    "rssMb": { "type": "array", "items": { "type": "integer", "minimum": 0 }, "minItems": 3, "maxItems": 3, "default": [0, 0, 0], "description": "Resident-memory rungs in MiB; 0 disables the RSS gate (the spec default is never)." }
                }
            },
            "hedge": {
                "type": "object",
                "additionalProperties": false,
                "description": "The opt-in hedger: duplicate a still-running idempotent GET render after afterFactor x the route's recent p95; first response wins. Idempotency is the user's responsibility.",
                "properties": {
                    "enabled": { "type": "boolean", "default": false, "description": "Off by default." },
                    "afterFactor": { "type": "number", "exclusiveMinimum": 1, "default": 2.0, "description": "Hedge after this multiple of the recent p95 (DSP-113 rejects <= 1)." }
                }
            }
        }
    })
}

/// Serialize the schema as pretty JSON with a trailing newline, ready to
/// write to `.oxide/schema/config.json` or a project's committed schema file.
/// The output re-parses as JSON and is deterministic (pinned by
/// `tests/schema_stability.rs`).
///
/// # Errors
///
/// Fails with `CFG-SCHEMA` when the in-memory schema cannot be serialized
/// (`serde_json` value out of range — not reachable for this literal).
pub fn config_schema_pretty() -> Result<String> {
    let value = sort_schema_value(config_schema());
    let s = serde_json::to_string_pretty(&value)
        .map_err(|e| crate::err("CFG-SCHEMA", format!("schema serialization failed: {e}")))?;
    Ok(format!("{s}\n"))
}

/// Canonicalize a schema `Value`: every object's keys in sorted order.
///
/// `serde_json::Map` is order-flavored — with the default build objects
/// sort (`BTreeMap`), but a feature flip elsewhere in the graph (e.g.
/// `bundler`'s `--all-features` pulling `serde_json/preserve_order`) makes
/// every `json!` object in the workspace emit in insertion order instead,
/// which silently desyncs the committed artifact from the generator (the
/// byte gates in `crates/config/tests/schema_stability.rs` and
/// `crates/schema/tests/schema_tdd.rs`). Projecting through a `BTreeMap`
/// pins the emission to sorted keys under any feature unification; with
/// the default map flavor this is a no-op.
fn sort_schema_value(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .collect::<std::collections::BTreeMap<String, Value>>()
                .into_iter()
                .map(|(k, v)| (k, sort_schema_value(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(sort_schema_value).collect()),
        other => other,
    }
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
    // NOTE (r8): this test inspects the serialized JSON text for duplicate
    // keys. A duplicate introduced by the `json!` macro itself (two
    // `"static":` entries in the same object literal) is collapsed by
    // `serde_json::Value`'s last-wins map semantics *before* serialization
    // — this test cannot see it. The r8 audit found exactly that: two
    // `"static":` blocks (lines 248/300) and a duplicate-guard test that
    // could never fire on them. The real fix for that class of bug is a
    // `cargo expand` / macro-level lint, tracked as a follow-up. This test
    // still guards against duplicate keys in *constructed* JSON (the
    // `serde_json::json!` macro output is not the only source).
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
                    let la = chars.clone();
                    let mut is_key = false;
                    for (_, c3) in la {
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
                    if let Some(k) = key.take()
                        && let Some(Some((_, seen))) = stack.last_mut()
                    {
                        assert!(
                            !seen.contains(&k),
                            "duplicate schema key `{k}` (last-wins drops the first)"
                        );
                        seen.push(k);
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
