#![doc = include_str!("../README.md")]

//! oxide-fw-core-wasm — edge subset (spec §2 D18, §18 cloudflare target).
//!
//! Compiles the core features that must run where Rust cannot run V8: router, cache
//! keys, head merge, ODF encode/decode. WinterCG-compatible JS is emitted for the
//! remaining glue. This crate must stay dependency-light so it builds for wasm32.

pub use oxide_fw_edge_primitives::sse_frame;
use oxide_fw_edge_primitives::{KeyHash, NonceSplicer};
use oxide_fw_odf::Value;
use oxide_fw_router::{
    NativeRule, RouteEntry, RouteId, RouteKind, RouteTree, RuleOutcome, RulesEngine,
};

pub type Result<T> = oxide_fw_odf::Result<T>;

/// Compute a cache key for the edge (same canonicalization as oxide-fw-cache L0 keys,
/// minus build-id parts not available at the edge).
#[must_use]
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
        #[must_use]
        pub fn new() -> Self {
            Self(blake3::Hasher::new())
        }
        pub fn update(&mut self, data: &[u8]) {
            self.0.update(data);
        }
        #[must_use]
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

/// Head merge for the edge build (mirrors `render::HeadTags::merge` semantics).
///
/// Flat `(key, value)` model of the head: `"title"` is special — the inner title wins
/// when present, and keeps the outer title's position so document order is stable.
/// Every other key: an inner entry replaces the outer entry with the same key (the
/// outer entry is dropped and the inner entry is appended in inner order, after all
/// surviving outer entries — outer-first order preserved).
#[must_use]
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
///
/// # Errors
///
/// Passes through the encoder's own `ODF-*` errors (e.g. an invalid
/// `Value::Ref` index).
pub fn encode_odf(value: &Value) -> Result<Vec<u8>> {
    oxide_fw_odf::encode(value)
}

/// §9.5 / D18: decode ODF bytes back to a neutral value. The mirror of
/// [`encode_odf`], exposed for the edge build.
///
/// # Errors
///
/// Surfaces the decoder's own `ODF-*` codes (malformed magic, oversize,
/// bad refs).
pub fn decode_odf(bytes: &[u8]) -> Result<Value> {
    oxide_fw_odf::decode(bytes)
}

/// Extract the URL-addressable route patterns from a router typegen `routes.json`
/// document (§8.3, consumed by the edge bundle's router manifest, §2 D18).
///
/// Accepts either the top-level array of route rows or an object with a `routes`
/// array. Patterns keep document order (the router emits them sorted) and are
/// deduplicated (layout + page can share a pattern).
///
/// # Errors
///
/// `EDGE-101` when the manifest is neither an array of route rows nor an
/// object with a `routes` array (router typegen §8.3).
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
        if let Some(pattern) = row.get("pattern").and_then(|p| p.as_str())
            && seen.insert(pattern.to_string())
        {
            out.push(pattern.to_string());
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
#[must_use]
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
        if let Some(t) = obj.get("title")
            && !t.is_null()
        {
            title = t.clone();
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
            .map_or_default(std::string::ToString::to_string)
    });
    let links = dedupe_by(links, |t| {
        let a = t.as_array();
        let k = a
            .and_then(|a| a.first())
            .map_or_default(std::string::ToString::to_string);
        let href = a
            .and_then(|a| a.get(1))
            .map_or_default(std::string::ToString::to_string);
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

    /// §V2-E09: match `path` against a routes.json manifest. Returns the
    /// JSON `{"id","params","score"}` or `"null"`. See [`crate::match_route`].
    #[wasm_bindgen]
    pub fn match_route(routes_json: &str, path: &str) -> String {
        crate::match_route(routes_json, path).unwrap_or_else(|_| "null".to_string())
    }

    /// §V2-E09: evaluate the native rules engine. Returns the JSON outcome
    /// or `"null"`. See [`crate::apply_rules`].
    #[wasm_bindgen]
    pub fn apply_rules(rules_json: &str, path: &str, accept_language: &str) -> String {
        crate::apply_rules(rules_json, path, accept_language).unwrap_or_else(|_| "null".to_string())
    }

    /// §V2-E09: apply an `ODP1` patch to a base JSON value. Returns the
    /// patched JSON or `"null"`. See [`crate::odp1_apply`].
    #[wasm_bindgen]
    pub fn odp1_apply(base_json: &str, patch: &[u8]) -> String {
        crate::odp1_apply(base_json, patch).unwrap_or_else(|_| "null".to_string())
    }

    /// §V2-E09: frame one SSE event. See [`crate::sse_frame`].
    #[wasm_bindgen]
    pub fn sse_frame(event: &str, id: Option<usize>, data: &str) -> String {
        crate::sse_frame(event, id, data)
    }

    /// §V2-E09: the 32-hex cache key display form. See [`crate::key_str`].
    #[wasm_bindgen]
    pub fn key_str(layer: &str, route: &str, params: &str, vary: &str, build_id: &str) -> String {
        crate::key_str(layer, route, params, vary, build_id)
    }

    /// §V2-E09: splice `from` → `to` across `html`. See [`crate::nonce_splice`].
    #[wasm_bindgen]
    pub fn nonce_splice(html: &str, from: &str, to: &str) -> String {
        crate::nonce_splice(html, from, to)
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

// ============================================================ V2-E09 exports
//
// The edge Worker's pure ops over the EXISTING substrates. Each is a thin
// wrapper over the tested native implementation — router match/rules, ODF
// patch, edge-primitives key/sse/splice — so the wasm32 edge build and the
// native dispatcher share one matcher, one rules engine, one patch apply,
// one key derivation and one nonce splice. The wasm-bindgen bindings below
// re-export them for the Worker; the native-mirror tests in
// `e09_mirror_tests` reuse the SAME vectors as the substrate's own tests
// (router `rules.rs`, odf `patch_tdd.rs`, edge-primitives `key`/`sse`/`splice`).

/// Build a [`RouteTree`] from a router typegen `routes.json` document (§8.3)
/// — the array of route rows or `{"routes": [...]}`. Only URL-addressable
/// kinds (`page`, `not-found`, `endpoint`, `socket`) are inserted;
/// layouts/middleware/error/loading are not matchable and the matcher would
/// skip them anyway. `pattern_encoded` is `"/" + segments.join("/")`, so the
/// inverse is a `/` split with empties dropped — the exact segments the
/// matcher classifies.
fn route_tree_from_manifest(tree_json: &serde_json::Value) -> Result<RouteTree> {
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
    let mut tree = RouteTree::new();
    for row in rows {
        let Some(id) = row.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let pattern = row.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let kind = row.get("kind").and_then(|v| v.as_str()).unwrap_or("page");
        let parsed_kind = match kind {
            "page" => RouteKind::Page,
            "not-found" => RouteKind::NotFound,
            "endpoint" => RouteKind::Endpoint,
            "socket" => RouteKind::Socket,
            // layouts/middleware/error/loading/template are not URL-addressable.
            _ => continue,
        };
        let segments: Vec<String> = pattern
            .split('/')
            .filter(|s| !s.is_empty())
            .map(std::string::ToString::to_string)
            .collect();
        tree.insert(RouteEntry {
            id: RouteId(id.to_string()),
            kind: parsed_kind,
            segments,
            file: format!("{id}.tsx").into(),
            render_mode: None,
            tags: Vec::new(),
            has_server_module: false,
            needs_parent: false,
            live: None,
            priority: None,
            slot: None,
        });
    }
    Ok(tree)
}

/// Match `path` against a router typegen `routes.json` route tree (§8.3,
/// §2 D18). Reuses [`RouteTree::match_path`] — the public seam that runs the
/// B02 candidate index and the exact scorer — so the edge match is the same
/// match the native dispatcher produces (no parallel matcher). Returns
/// `null` when no route matches, otherwise `{"id","params","score"}`.
///
/// `params` is a JSON object `{name: value}`; `score` is the matcher's
/// tie-break score (higher wins).
///
/// # Errors
///
/// - `EDGE-103`: `routes_json` is not valid JSON.
/// - `EDGE-101`: the manifest is not a route-row array.
pub fn match_route(routes_json: &str, path: &str) -> Result<String> {
    let tree_value: serde_json::Value = serde_json::from_str(routes_json).map_err(|e| {
        oxide_fw_odf::err(
            "EDGE-103",
            format!("routes manifest is not valid JSON: {e}"),
        )
    })?;
    let tree = route_tree_from_manifest(&tree_value)?;
    let Some(m) = tree.match_path(path) else {
        return Ok("null".to_string());
    };
    let mut params = serde_json::Map::new();
    for (k, v) in &m.params {
        params.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    Ok(serde_json::to_string(&serde_json::json!({
        "id": m.route.id.0,
        "params": params,
        "score": m.score,
    }))
    .unwrap_or_else(|_| "null".to_string()))
}

/// Parse one JSON rule object into a [`NativeRule`] (the engine's compiled
/// shape). `type` maps to `rule_type`; `destination`/`status`/`headers`/
/// `basicAuth`/`rateLimit` are optional.
fn parse_native_rule(row: &serde_json::Value) -> Result<NativeRule> {
    let source = row
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| oxide_fw_odf::err("EDGE-104", "a rule is missing the `source` string"))?;
    let headers = row.get("headers").and_then(|v| v.as_object()).map(|o| {
        o.iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
            .collect::<std::collections::BTreeMap<String, String>>()
    });
    let basic_auth = row.get("basicAuth").and_then(|v| {
        let user = v.get("user").and_then(|x| x.as_str());
        let pass = v.get("pass").and_then(|x| x.as_str());
        match (user, pass) {
            (Some(u), Some(p)) => Some((u.to_string(), p.to_string())),
            _ => None,
        }
    });
    let rate_limit = row.get("rateLimit").and_then(|v| {
        let key = v.get("key").and_then(|x| x.as_str())?.to_string();
        let max = v
            .get("max")
            .and_then(serde_json::Value::as_i64)
            .and_then(|i| u32::try_from(i).ok())?;
        let window_secs = v
            .get("window_secs")
            .and_then(serde_json::Value::as_i64)
            .and_then(|i| u64::try_from(i).ok())?;
        Some(oxide_fw_router::RateLimit {
            key,
            max,
            window_secs,
        })
    });
    Ok(NativeRule {
        source: source.to_string(),
        destination: row
            .get("destination")
            .and_then(|v| v.as_str())
            .map(String::from),
        status: row
            .get("status")
            .and_then(serde_json::Value::as_i64)
            .and_then(|i| u16::try_from(i).ok()),
        rule_type: row.get("type").and_then(|v| v.as_str()).map(String::from),
        headers,
        basic_auth,
        rate_limit,
        priority: None,
    })
}

/// Render a [`RuleOutcome`] as the JSON the Worker consumes: `null` for no
/// outcome, otherwise `{"redirect","status"}` / `{"rewrite"}` /
/// `{"headers"}` / `{"authRequired"}` / `{"rateLimited","retryAfter"}`.
fn outcome_to_json(outcome: &RuleOutcome) -> serde_json::Value {
    match outcome {
        RuleOutcome::None => serde_json::Value::Null,
        RuleOutcome::Redirect {
            destination,
            status,
        } => {
            serde_json::json!({ "redirect": destination, "status": status })
        }
        RuleOutcome::Rewrite(dest) => serde_json::json!({ "rewrite": dest }),
        RuleOutcome::Headers(h) => {
            let mut m = serde_json::Map::new();
            for (k, v) in h {
                m.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            serde_json::json!({ "headers": m })
        }
        RuleOutcome::AuthRequired => serde_json::json!({ "authRequired": true }),
        RuleOutcome::RateLimited { retry_after_secs } => {
            serde_json::json!({ "rateLimited": true, "retryAfter": retry_after_secs })
        }
    }
}

/// Evaluate the native rules engine (§12.1, D13, §17.1) at the edge:
/// redirects, rewrites, headers, basic-auth and rate-limit decisions
/// evaluated in Rust before any JS. Reuses [`RulesEngine::compile`] +
/// [`RulesEngine::evaluate`] — the same engine the native dispatch runs.
///
/// `rules_json` is a JSON array of rule objects (`{source, destination?,
/// status?, type?, headers?, basicAuth?, rateLimit?}`). `accept_language` is
/// accepted for the v0.2 i18n rule shape (the negotiation is compiled to a
/// table at build; the edge applies the table's redirect — not yet wired
/// through the engine, recorded in `docs/deviations.md`). Returns `null` for
/// no outcome, otherwise the shape [`outcome_to_json`] emits.
///
/// # Errors
///
/// - `EDGE-104`: the rules table is not a JSON array, or a rule is malformed.
/// - `RTR-110`: a rule fails compile-time validation (passed through).
pub fn apply_rules(rules_json: &str, path: &str, _accept_language: &str) -> Result<String> {
    let rules_value: serde_json::Value = serde_json::from_str(rules_json).map_err(|e| {
        oxide_fw_odf::err("EDGE-104", format!("rules table is not valid JSON: {e}"))
    })?;
    let Some(rows) = rules_value.as_array() else {
        return Err(oxide_fw_odf::err(
            "EDGE-104",
            "rules table must be a JSON array of rule objects",
        ));
    };
    let mut rules = Vec::with_capacity(rows.len());
    for row in rows {
        rules.push(parse_native_rule(row)?);
    }
    let engine = RulesEngine::compile(rules)?;
    // The edge has no client IP at match time (rate-limit keys are resolved
    // against the build table) and no Authorization header to verify (basic
    // auth surfaces as `authRequired`, which the Worker turns into a 401).
    let outcome = engine.evaluate(path);
    Ok(outcome_to_json(&outcome).to_string())
}

/// Apply an `ODP1` patch (F06, §V2-F06 / D30) to a base JSON value at the
/// edge. Reuses [`oxide_fw_odf::patch::decode_patch`] +
/// [`oxide_fw_odf::patch::apply`] — the same substrate the live runner uses.
/// `base_json` is a JSON value; `patch_bytes` is the `ODP1`-prefixed payload.
/// Returns the patched value as JSON.
///
/// # Errors
///
/// - `EDGE-105`: `base_json` is not valid JSON or the result is not serializable.
/// - `ODP-*` / `ODF-*`: the patch payload is malformed or the apply fails
///   (passed through from the patch substrate).
pub fn odp1_apply(base_json: &str, patch_bytes: &[u8]) -> Result<String> {
    let base_value: serde_json::Value = serde_json::from_str(base_json)
        .map_err(|e| oxide_fw_odf::err("EDGE-105", format!("base value is not valid JSON: {e}")))?;
    let base = Value::from_json(&base_value);
    let (_base_id, ops) = oxide_fw_odf::patch::decode_patch(patch_bytes)?;
    let result = oxide_fw_odf::patch::apply(&base, &ops)?;
    serde_json::to_string(&result.to_json()).map_err(|e| {
        oxide_fw_odf::err(
            "EDGE-105",
            format!("patched value is not serializable: {e}"),
        )
    })
}

/// The 128-bit cache key display form (D24) for the edge — the 32-char hex
/// [`KeyHash`] the native L0 cache uses (`oxide_fw_cache::key_of`). Reuses
/// [`KeyHash::of_parts`] + [`KeyHash::hex`] (moved to
/// `oxide-fw-edge-primitives`), so the edge key is byte-identical to the
/// native L0 key (the wasm build links neither `redb` nor `tokio`).
#[must_use]
pub fn key_str(layer: &str, route: &str, params: &str, vary: &str, build_id: &str) -> String {
    KeyHash::of_parts(layer, route, params, vary, build_id)
        .hex()
        .as_str()
        .to_string()
}

/// Splice every `from` occurrence in `html` to `to` through the chunk-safe
/// [`NonceSplicer`] (§V2-B08) — one chunk = the whole document. Reuses the
/// edge-primitives splicer (moved from `oxide-fw-render::splice`); the nonce
/// is ASCII hex and the replacement is the ASCII sentinel, so a match can
/// never split a multi-byte UTF-8 sequence and the output is always valid
/// UTF-8.
///
/// # Panics
///
/// Panics if the spliced bytes are not valid UTF-8 — impossible when `from`
/// is ASCII (a CSP nonce is ASCII hex) and `to` is ASCII (the sentinel), so
/// no match can split a multi-byte sequence and the output is valid UTF-8.
#[must_use]
pub fn nonce_splice(html: &str, from: &str, to: &str) -> String {
    let mut splicer = NonceSplicer::new(from, to);
    let mut out = Vec::with_capacity(html.len());
    splicer.push(html.as_bytes(), &mut out);
    splicer.finish(&mut out);
    String::from_utf8(out).expect("nonce splice preserves UTF-8 (ascii needle + replacement)")
}

#[cfg(test)]
mod e09_mirror_tests {
    use super::*;
    use oxide_fw_odf::Value;
    use oxide_fw_odf::patch::{decode_patch, diff, encode_patch};
    use serde_json::json;

    // ----- match_route: mirrors router `rules.rs`/`index.rs` match vectors -----

    /// The routes.json the typegen emits for a `[slug]` page; the edge match
    /// must agree with `RouteTree::match_path` (the same seam the dispatcher
    /// uses). Mirrors the `blog/:slug` fixture from `index.rs`.
    fn blog_manifest() -> String {
        serde_json::to_string(&json!([
            { "id": "routes/page", "pattern": "/", "kind": "page" },
            { "id": "routes/blog/page", "pattern": "/blog", "kind": "page" },
            { "id": "routes/blog/[slug]/page", "pattern": "/blog/:slug", "kind": "page" },
            { "id": "routes/files/[...rest]/page", "pattern": "/files/*rest", "kind": "page" }
        ]))
        .unwrap()
    }

    #[test]
    fn match_route_captures_a_param_like_the_native_matcher() {
        let out = match_route(&blog_manifest(), "/blog/hello").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["id"], "routes/blog/[slug]/page");
        assert_eq!(v["params"]["slug"], "hello");
    }

    #[test]
    fn match_route_returns_null_for_no_match() {
        let out = match_route(&blog_manifest(), "/nope").unwrap();
        assert_eq!(out, "null");
    }

    #[test]
    fn match_route_root_and_static_match() {
        assert_eq!(
            match_route(&blog_manifest(), "/").unwrap(),
            serde_json::to_string(&json!({"id":"routes/page","params":{},"score":0}))
                .unwrap()
                .replace(' ', ""),
            "the root page matches with empty params"
        );
        // `/blog` matches the static `blog` page (no params), not the slug.
        let v: serde_json::Value =
            serde_json::from_str(&match_route(&blog_manifest(), "/blog").unwrap()).unwrap();
        assert_eq!(v["id"], "routes/blog/page");
        assert_eq!(v["params"].as_object().unwrap().len(), 0);
    }

    #[test]
    fn match_route_accepts_the_routes_object_shape() {
        let wrapped = format!("{{\"routes\":{}}}", blog_manifest());
        let v: serde_json::Value =
            serde_json::from_str(&match_route(&wrapped, "/blog/x").unwrap()).unwrap();
        assert_eq!(v["id"], "routes/blog/[slug]/page");
    }

    // ----- apply_rules: mirrors router `rules.rs` outcome vectors -----

    #[test]
    fn apply_rules_redirect_defaults_to_308_and_honors_status() {
        // Mirrors `redirect_defaults_to_308_and_honors_status`.
        let rules = json!([
            { "source": "/old", "destination": "/new" }
        ])
        .to_string();
        let v: serde_json::Value =
            serde_json::from_str(&apply_rules(&rules, "/old", "").unwrap()).unwrap();
        assert_eq!(v["redirect"], "/new");
        assert_eq!(v["status"], 308);
        assert_eq!(apply_rules(&rules, "/other", "").unwrap(), "null");
    }

    #[test]
    fn apply_rules_redirect_substitutes_path_params() {
        // Mirrors `redirect_substitutes_path_params`.
        let rules = json!([
            { "source": "/blog/:slug", "destination": "/posts/:slug" }
        ])
        .to_string();
        let v: serde_json::Value =
            serde_json::from_str(&apply_rules(&rules, "/blog/rust-is-nice", "").unwrap()).unwrap();
        assert_eq!(v["redirect"], "/posts/rust-is-nice");
        assert_eq!(v["status"], 308);
    }

    #[test]
    fn apply_rules_rewrite_and_headers_merge() {
        // Mirrors `rewrites_rewrite` + `headers_merge_across_matching_rules`.
        let rules = json!([
            { "source": "/shop/:item", "type": "rewrite", "destination": "/products/:item" }
        ])
        .to_string();
        let v: serde_json::Value =
            serde_json::from_str(&apply_rules(&rules, "/shop/42", "").unwrap()).unwrap();
        assert_eq!(v["rewrite"], "/products/42");

        let headers = json!([
            { "source": "/legacy/*", "headers": { "x-a": "1", "x-both": "a" } },
            { "source": "/legacy/:page", "headers": { "x-b": "2", "x-both": "b" } }
        ])
        .to_string();
        let v: serde_json::Value =
            serde_json::from_str(&apply_rules(&headers, "/legacy/home", "").unwrap()).unwrap();
        assert_eq!(v["headers"]["x-a"], "1");
        assert_eq!(v["headers"]["x-b"], "2");
        assert_eq!(v["headers"]["x-both"], "b", "later rules win per key");
    }

    #[test]
    fn apply_rules_returns_null_when_no_rule_matches() {
        let rules = json!([{ "source": "/old", "destination": "/new" }]).to_string();
        assert_eq!(apply_rules(&rules, "/unmatched", "").unwrap(), "null");
    }

    // ----- odp1_apply: mirrors odf `patch_tdd.rs` diff/apply vectors -----

    fn obj(pairs: &[(&str, Value)]) -> Value {
        let mut m = std::collections::BTreeMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), v.clone());
        }
        Value::Object(m)
    }

    /// Round-trip an ODP1 patch through [`odp1_apply`]: diff(base, new) →
    /// `encode_patch` → `odp1_apply(base_json`, patch) == `new_json`. Mirrors the
    /// `patch_tdd.rs` vectors (Set, Del, Splice append/prepend/middle/truncate).
    fn round_trip(base: &Value, new: &Value) {
        let ops = diff(base, new);
        let patch = encode_patch("base0", &ops).unwrap();
        let base_json = serde_json::to_string(&base.to_json()).unwrap();
        let result = odp1_apply(&base_json, &patch).unwrap();
        let result_value: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            result_value,
            new.to_json(),
            "odp1_apply must reproduce the diff target"
        );
    }

    #[test]
    fn odp1_apply_object_set_at_root_and_nested() {
        // Mirrors `diff_set_at_the_root_and_nested_objects`.
        round_trip(
            &obj(&[("a", Value::Number(1.0))]),
            &obj(&[("a", Value::Number(2.0))]),
        );
        let nested_old = obj(&[
            ("a", obj(&[("b", obj(&[("c", Value::Number(1.0))]))])),
            ("keep", Value::String("x".to_string())),
        ]);
        let nested_new = obj(&[
            ("a", obj(&[("b", obj(&[("c", Value::Number(2.0))]))])),
            ("keep", Value::String("x".to_string())),
        ]);
        round_trip(&nested_old, &nested_new);
    }

    #[test]
    fn odp1_apply_removed_key_is_a_del() {
        // Mirrors `diff_removed_key_is_a_del`.
        round_trip(
            &obj(&[
                ("a", Value::Number(1.0)),
                ("gone", Value::String("bye".to_string())),
            ]),
            &obj(&[("a", Value::Number(1.0))]),
        );
    }

    #[test]
    fn odp1_apply_array_splice_append_prepend_middle_truncate() {
        // Mirrors `diff_append_prepend_middle_and_truncate_are_one_splice_each`.
        let base = Value::Array(vec![
            Value::Number(1.0),
            Value::Number(2.0),
            Value::Number(3.0),
        ]);
        round_trip(&base, &Value::Array(base_array(vec![1.0, 2.0, 3.0, 4.0])));
        round_trip(&base, &Value::Array(base_array(vec![0.0, 1.0, 2.0, 3.0])));
        round_trip(&base, &Value::Array(base_array(vec![1.0, 9.0, 3.0])));
        round_trip(&base, &Value::Array(base_array(vec![1.0])));
    }

    fn base_array(nums: Vec<f64>) -> Vec<Value> {
        nums.into_iter().map(Value::Number).collect()
    }

    #[test]
    fn odp1_apply_round_trips_through_the_wire_bytes() {
        // decode_patch(odp1_apply's input) must agree with the ops the
        // substrate produced — the wire bytes are not a black box.
        let base = obj(&[("a", Value::Number(1.0))]);
        let new = obj(&[("a", Value::Number(2.0))]);
        let ops = diff(&base, &new);
        let patch = encode_patch("base0", &ops).unwrap();
        let (b, decoded) = decode_patch(&patch).unwrap();
        assert_eq!(b, "base0");
        assert_eq!(decoded, ops);
    }

    // ----- sse_frame: mirrors edge-primitives `sse` + server `live_tdd` -----

    #[test]
    fn sse_frame_multiline_data_matches_the_spec_pin() {
        assert_eq!(
            sse_frame("data", Some(7), "line1\nline2"),
            "event: data\nid: 7\ndata: line1\ndata: line2\n\n"
        );
        assert_eq!(sse_frame("ping", None, "1"), "event: ping\ndata: 1\n\n");
    }

    // ----- key_str: mirrors edge-primitives `key` / cache `keyhash` -----

    #[test]
    fn key_str_is_32_hex_and_matches_the_native_keyhash() {
        // Mirrors `key_str_is_32_hex_chars_and_raw_round_trips` +
        // `key_of_is_keyhash_hex`.
        let k = key_str("L0", "routes/blog/[slug]/page", "{}", "", "build");
        assert_eq!(k.len(), 32);
        assert_eq!(
            k,
            KeyHash::of_parts("L0", "routes/blog/[slug]/page", "{}", "", "build")
                .hex()
                .as_str()
        );
    }

    #[test]
    fn key_str_is_prefix_free_and_component_sensitive() {
        // Mirrors `prefix_free` + `every_component_participates`.
        assert_ne!(
            key_str("L0", "ab", "c", "", ""),
            key_str("L0", "a", "bc", "", "")
        );
        let base = key_str("L0", "r", "p", "v", "b");
        assert_ne!(base, key_str("L1", "r", "p", "v", "b"));
        assert_ne!(base, key_str("L0", "r2", "p", "v", "b"));
        assert_ne!(base, key_str("L0", "r", "p2", "v", "b"));
        assert_ne!(base, key_str("L0", "r", "p", "v2", "b"));
        assert_ne!(base, key_str("L0", "r", "p", "v", "b2"));
    }

    // ----- nonce_splice: mirrors render `splice` (byte-identical to replace) -----

    #[test]
    fn nonce_splice_matches_str_replace() {
        // Mirrors `splice_matches_str_replace` (the cli pin) + the render
        // `splits_at_every_position` byte-identical contract.
        let nonce = "nonce-abc123";
        let html = "<html><script nonce=\"nonce-abc123\"></script>\
                    <p>nonce-abc123</p><script nonce=\"nonce-abc123\"/></html>";
        assert_eq!(
            nonce_splice(html, nonce, "SENTINEL"),
            html.replace(nonce, "SENTINEL")
        );
    }

    #[test]
    fn nonce_splice_without_needle_is_identity() {
        let html = "<html><body>plain</body></html>";
        assert_eq!(nonce_splice(html, "nonce-xyz", "SENTINEL"), html);
    }

    #[test]
    fn nonce_splice_at_document_boundaries() {
        assert_eq!(nonce_splice("N", "N", "S"), "S");
        assert_eq!(nonce_splice("xNx", "N", "S"), "xSx");
        assert_eq!(nonce_splice("abN", "N", "S"), "abS");
    }
}
