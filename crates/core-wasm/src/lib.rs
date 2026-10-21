//! oxide-core-wasm — edge subset (spec §2 D18, §18 cloudflare target).
//!
//! Compiles the core features that must run where Rust cannot run V8: router, cache
//! keys, head merge, ODF encode/decode. WinterCG-compatible JS is emitted for the
//! remaining glue. This crate must stay dependency-light so it builds for wasm32.

use oxide_odf::Value;

pub type Result<T> = oxide_odf::Result<T>;

/// Compute a cache key for the edge (same canonicalization as oxide-cache L0 keys,
/// minus build-id parts not available at the edge).
pub fn edge_cache_key(route: &str, params: &str, vary: &str, build_id: &str) -> String {
    // blake3 is available for wasm32; mirror cache::CacheKey::canonical layout.
    let mut h = blake3_stub::Hasher::new();
    h.update(b"L0");
    h.update(b"\x00");
    h.update(route.as_bytes());
    h.update(b"\x00");
    h.update(params.as_bytes());
    h.update(b"\x00");
    h.update(vary.as_bytes());
    h.update(b"\x00");
    h.update(build_id.as_bytes());
    h.hex_digest()
}

/// SPEC-22: real blake3 for edge cache keys. `blake3` is pure Rust and
/// compiles to wasm32, so this crate no longer needs an FNV stub — the edge
/// keys are now byte-identical to the native L0 keys (§18/§11.2).
pub mod blake3_stub {
    /// A `blake3` hasher with the crate's `new`/`update`/`hex_digest` shape.
    pub struct Hasher(blake3::Hasher);
    impl Hasher {
        pub fn new() -> Self {
            Self(blake3::Hasher::new())
        }
        pub fn update(&mut self, data: &[u8]) {
            self.0.update(data);
        }
        pub fn hex_digest(&self) -> String {
            self.0.finalize().to_hex().to_string()
        }
    }
    impl Default for Hasher {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod spec22_tests {
    use super::blake3_stub::Hasher;

    /// SPEC-22: the edge key digest equals the native `blake3` digest byte for
    /// byte (the stub is gone — this is the real crate).
    #[test]
    fn edge_keys_match_native_blake3() {
        for msg in [b"".as_slice(), b"hello", b"routes/index\0get\0secret"] {
            let mut h = Hasher::new();
            h.update(msg);
            assert_eq!(h.hex_digest(), blake3::hash(msg).to_hex().to_string());
        }
    }

    /// The known BLAKE3 vector for "abc" (from the reference implementation).
    #[test]
    fn known_blake3_vector() {
        let mut h = Hasher::new();
        h.update(b"abc");
        assert_eq!(
            h.hex_digest(),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
    }
}

/// Head merge for the edge build (mirrors render::HeadTags::merge semantics).
///
/// Flat `(key, value)` model of the head: `"title"` is special — the inner title wins
/// when present, and keeps the outer title's position so document order is stable.
/// Every other key: an inner entry replaces the outer entry with the same key (the
/// outer entry is dropped and the inner entry is appended in inner order, after all
/// surviving outer entries — outer-first order preserved).
pub fn merge_head(
    outer: Vec<(String, String)>,
    inner: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let inner_title = inner
        .iter()
        .find(|(k, _)| k == "title")
        .map(|(_, v)| v.clone());
    let inner_keys: std::collections::BTreeSet<&str> =
        inner.iter().map(|(k, _)| k.as_str()).collect();
    let mut out: Vec<(String, String)> = Vec::with_capacity(outer.len() + inner.len());
    let mut title_seen = false;
    for (k, v) in outer {
        if k == "title" {
            match &inner_title {
                Some(t) => out.push(("title".to_string(), t.clone())),
                None => out.push((k, v)),
            }
            title_seen = true;
        } else if inner_keys.contains(k.as_str()) {
            // Overridden by an inner entry — dropped here, appended below.
        } else {
            out.push((k, v));
        }
    }
    for (k, v) in inner {
        if k == "title" {
            if !title_seen {
                out.push((k, v));
                title_seen = true;
            }
        } else {
            out.push((k, v));
        }
    }
    out
}

/// Encode an ODF value for edge embedding (re-export with edge defaults).
pub fn encode_odf(value: &Value) -> Result<Vec<u8>> {
    oxide_odf::encode(value)
}

/// §9.5 / D18: decode ODF bytes back to a neutral value. The mirror of
/// [`encode_odf`], exposed for the edge build. Errors surface as the
/// decoder's own `ODF-*` codes.
pub fn decode_odf(bytes: &[u8]) -> Result<Value> {
    oxide_odf::decode(bytes)
}

/// Extract the URL-addressable route patterns from a router typegen `routes.json`
/// document (§8.3, consumed by the edge bundle's router manifest, §2 D18).
///
/// Accepts either the top-level array of route rows or an object with a `routes`
/// array. Patterns keep document order (the router emits them sorted) and are
/// deduplicated (layout + page can share a pattern).
pub fn edge_manifest_routes(tree_json: &serde_json::Value) -> Result<Vec<String>> {
    let rows = match tree_json {
        serde_json::Value::Array(rows) => rows,
        v => match v.get("routes").and_then(|r| r.as_array()) {
            Some(rows) => rows,
            None => {
                return Err(oxide_odf::err(
                    "EDGE-101",
                    "routes manifest must be an array of route rows or {\"routes\": [...]} \
                     (router typegen §8.3)",
                ));
            }
        },
    };
    let mut out: Vec<String> = Vec::with_capacity(rows.len());
    let mut seen = std::collections::BTreeSet::new();
    for row in rows {
        if let Some(pattern) = row.get("pattern").and_then(|p| p.as_str()) {
            if seen.insert(pattern.to_string()) {
                out.push(pattern.to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

/// B-01: wasm-bindgen exports for the edge Worker. Thin wrappers over the
/// pure-Rust functions above (which remain the tested implementations).
#[cfg(target_arch = "wasm32")]
pub mod bindings {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    pub fn edge_cache_key(route: &str, params: &str, vary: &str, build_id: &str) -> String {
        crate::edge_cache_key(route, params, vary, build_id)
    }

    /// `shell`/`head` are JSON arrays of `[key, value]` pairs; returns the
    /// merged array as JSON (the edge handler parses it back).
    #[wasm_bindgen]
    pub fn merge_head(shell: &str, head: &str) -> String {
        let a: Vec<(String, String)> = serde_json::from_str(shell).unwrap_or_default();
        let b: Vec<(String, String)> = serde_json::from_str(head).unwrap_or_default();
        serde_json::to_string(&crate::merge_head(a, b)).unwrap_or_else(|_| "[]".to_string())
    }

    /// JSON value in, ODF bytes out (JS `Uint8Array`).
    #[wasm_bindgen]
    pub fn encode_odf(json: &[u8]) -> Vec<u8> {
        let value: serde_json::Value =
            serde_json::from_slice(json).unwrap_or(serde_json::Value::Null);
        crate::encode_odf(&value).unwrap_or_default()
    }

    /// ODF bytes in, JSON value bytes out.
    #[wasm_bindgen]
    pub fn decode_odf(odf: &[u8]) -> Result<Vec<u8>, JsValue> {
        let v = crate::decode_odf(odf)
            .map_err(|e| JsValue::from_str(&format!("edge decode_odf: {}", e.0.message)))?;
        serde_json::to_vec(&v).map_err(|e| JsValue::from_str(&format!("edge decode_odf: {e}")))
    }
}
