//! oxide-fw-core-wasm — edge subset (spec §2 D18, §18 cloudflare target).
//!
//! Compiles the core features that must run where Rust cannot run V8: router, cache
//! keys, head merge, ODF encode/decode. WinterCG-compatible JS is emitted for the
//! remaining glue. This crate must stay dependency-light so it builds for wasm32.

use oxide_fw_odf::Value;

pub type Result<T> = oxide_fw_odf::Result<T>;

/// Compute a cache key for the edge (same canonicalization as oxide-fw-cache L0 keys,
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
    oxide_fw_odf::encode(value)
}

/// §9.5 / D18: decode ODF bytes back to a neutral value. The mirror of
/// [`encode_odf`], exposed for the edge build. Errors surface as the
/// decoder's own `ODF-*` codes.
pub fn decode_odf(bytes: &[u8]) -> Result<Value> {
    oxide_fw_odf::decode(bytes)
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
                return Err(oxide_fw_odf::err(
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

/// §18 / D18: the edge head model, merged structurally.
///
/// The edge handler merges two head models of shape
/// `{ title, meta: [[k, v], ...], links: [[k, v], ...], json_ld: [...] }`.
/// This is **not** the flat `Vec<(String, String)>` `merge_head` above
/// merges — that ABI mismatch is why the wasm path silently returned `[]`
/// for every edge response (P0-1a). This function takes and returns the
/// structured shape.
///
/// Semantics match the JS implementation the handler falls back to:
/// later sources win on `title`; `meta` dedupes by the first pair element;
/// `links` dedupe by `(rel, href)`; `json_ld` concatenates. Dedupe keeps
/// the **last** occurrence, which is what `Map.set` in the JS loop does.
pub fn merge_head_structured(
    base: &serde_json::Value,
    over: &serde_json::Value,
) -> serde_json::Value {
    let mut title = serde_json::Value::Null;
    let mut meta: Vec<serde_json::Value> = Vec::new();
    let mut links: Vec<serde_json::Value> = Vec::new();
    let mut json_ld: Vec<serde_json::Value> = Vec::new();

    for src in [base, over] {
        let Some(obj) = src.as_object() else { continue };
        if let Some(t) = obj.get("title") {
            if !t.is_null() {
                title = t.clone();
            }
        }
        if let Some(arr) = obj.get("meta").and_then(|v| v.as_array()) {
            meta.extend(arr.iter().cloned());
        }
        if let Some(arr) = obj.get("links").and_then(|v| v.as_array()) {
            links.extend(arr.iter().cloned());
        }
        if let Some(arr) = obj.get("json_ld").and_then(|v| v.as_array()) {
            json_ld.extend(arr.iter().cloned());
        }
    }

    // Dedupe keeping the LAST occurrence: a `BTreeMap` from key → value,
    // then the values in insertion order of their final occurrence. The JS
    // implementation achieves the same with `Map.set` (which replaces the
    // value but keeps the original insertion position); here the key order
    // is rebuilt from the surviving entries' positions.
    let dedupe_by = |items: Vec<serde_json::Value>, key: fn(&serde_json::Value) -> String| {
        let mut last_at: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for (i, item) in items.iter().enumerate() {
            last_at.insert(key(item), i);
        }
        // Keep items whose index is their key's last occurrence, in order.
        let live: std::collections::BTreeSet<usize> = last_at.values().copied().collect();
        items
            .into_iter()
            .enumerate()
            .filter(|(i, _)| live.contains(i))
            .map(|(_, v)| v)
            .collect::<Vec<_>>()
    };

    let meta = dedupe_by(meta, |t| {
        t.as_array()
            .and_then(|a| a.first())
            .map(|v| v.to_string())
            .unwrap_or_default()
    });
    let links = dedupe_by(links, |t| {
        let a = t.as_array();
        let k = a
            .and_then(|a| a.first())
            .map(|v| v.to_string())
            .unwrap_or_default();
        let href = a
            .and_then(|a| a.get(1))
            .map(|v| v.to_string())
            .unwrap_or_default();
        format!("{k}:{href}")
    });

    serde_json::json!({
        "title": title,
        "meta": meta,
        "links": links,
        "json_ld": json_ld,
    })
}

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

    /// §18 / D18: the structured head merge. `shell`/`head` are JSON
    /// objects of shape `{ title, meta, links, json_ld }`; returns the
    /// merged object as JSON.
    ///
    /// Distinct from `merge_head` above, which merges flat
    /// `Vec<(String, String)>` pairs — the ABI mismatch that made the old
    /// wasm path return `[]` for every edge response (P0-1a).
    #[wasm_bindgen]
    pub fn merge_head_structured(shell: &str, head: &str) -> String {
        let a: serde_json::Value = serde_json::from_str(shell).unwrap_or(serde_json::Value::Null);
        let b: serde_json::Value = serde_json::from_str(head).unwrap_or(serde_json::Value::Null);
        serde_json::to_string(&crate::merge_head_structured(&a, &b))
            .unwrap_or_else(|_| "{}".to_string())
    }

    /// JSON value in, ODF bytes out (JS `Uint8Array`).
    #[wasm_bindgen]
    pub fn encode_odf(json: &[u8]) -> Vec<u8> {
        // `oxide_fw_odf::Value` is the ODF model; `serde_json::Value` is the wire
        // form the caller passes. `Value::from_json` is the bridge (the same
        // one the native encoder uses at the boundary).
        let json_value: serde_json::Value =
            serde_json::from_slice(json).unwrap_or(serde_json::Value::Null);
        let value = crate::Value::from_json(&json_value);
        crate::encode_odf(&value).unwrap_or_default()
    }

    /// ODF bytes in, JSON value bytes out.
    #[wasm_bindgen]
    pub fn decode_odf(odf: &[u8]) -> Result<Vec<u8>, JsValue> {
        let v = crate::decode_odf(odf)
            .map_err(|e| JsValue::from_str(&format!("edge decode_odf: {}", e.0.message)))?;
        // `Value::to_json` is the inverse of `from_json` above (a lossy but
        // total projection: Ref → null, Deferred → {"__deferred": id}, etc).
        let json_value = v.to_json();
        serde_json::to_vec(&json_value)
            .map_err(|e| JsValue::from_str(&format!("edge decode_odf: {e}")))
    }
}

#[cfg(test)]
mod merge_head_structured_tests {
    use super::merge_head_structured;
    use serde_json::json;

    /// Later source wins on `title`, matching the JS `for (const src of [a, b])`.
    #[test]
    fn inner_title_wins() {
        let out = merge_head_structured(
            &json!({"title": "outer", "meta": [], "links": [], "json_ld": []}),
            &json!({"title": "inner", "meta": [], "links": [], "json_ld": []}),
        );
        assert_eq!(out["title"], "inner");
    }

    /// A `null` title in the inner model does not clobber the outer one —
    /// the JS guard is `if (src.title)`, which is falsy for null.
    #[test]
    fn null_inner_title_preserves_outer() {
        let out = merge_head_structured(
            &json!({"title": "outer", "meta": [], "links": [], "json_ld": []}),
            &json!({"title": null, "meta": [], "links": [], "json_ld": []}),
        );
        assert_eq!(out["title"], "outer");
    }

    /// `meta` dedupes by the first pair element, keeping the LAST occurrence
    /// (the JS `Map.set` semantics).
    #[test]
    fn meta_dedupes_by_key_keeping_last() {
        let out = merge_head_structured(
            &json!({"meta": [["description", "outer"], ["author", "ada"]]}),
            &json!({"meta": [["description", "inner"]]}),
        );
        let meta = out["meta"].as_array().unwrap();
        assert_eq!(meta.len(), 2, "two distinct keys survive: {meta:?}");
        // The description pair is the inner value.
        let desc = meta
            .iter()
            .find(|p| p[0] == "description")
            .expect("description present");
        assert_eq!(desc[1], "inner");
    }

    /// `links` dedupe by `(rel, href)`.
    #[test]
    fn links_dedupe_by_rel_and_href() {
        let out = merge_head_structured(
            &json!({"links": [["canonical", "/a"], ["preload", "/x"]]}),
            &json!({"links": [["canonical", "/a"], ["canonical", "/b"]]}),
        );
        let links = out["links"].as_array().unwrap();
        // `(canonical, /a)` collapses to one; `(canonical, /b)` and
        // `(preload, /x)` are distinct.
        assert_eq!(links.len(), 3, "got {links:?}");
    }

    /// `json_ld` concatenates without dedupe — the JS pushes both.
    #[test]
    fn json_ld_concatenates() {
        let out = merge_head_structured(
            &json!({"json_ld": [{"@type": "Article"}]}),
            &json!({"json_ld": [{"@type": "Breadcrumb"}]}),
        );
        assert_eq!(out["json_ld"].as_array().unwrap().len(), 2);
    }

    /// A missing field on one side is treated as empty, not as a panic —
    /// the edge handler passes whatever the layout and page produced.
    #[test]
    fn missing_fields_are_empty() {
        let out = merge_head_structured(&json!({}), &json!({"title": "only"}));
        assert_eq!(out["title"], "only");
        assert_eq!(out["meta"].as_array().unwrap().len(), 0);
        assert_eq!(out["links"].as_array().unwrap().len(), 0);
    }
}
