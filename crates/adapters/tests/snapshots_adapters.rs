//! §22: `insta` snapshots for the deploy-artifact emitters.
//!
//! The spec's requirement is "insta snapshots for HTML and manifests". The
//! manifest/artifact half lives here: `_headers`, `_redirects`, the Dockerfile,
//! and the Cloudflare entry are pure `&str`-in/`String`-out functions whose
//! byte shape is a contract with each hosting platform. A regression that
//! changes indentation or a status code breaks a deploy silently; a snapshot
//! makes it fail here instead.
//!
//! Update with `INSTA_UPDATE=always cargo test -p oxide-adapters --test snapshots_adapters`.

use std::collections::BTreeMap;

use oxide_adapters::*;

// ---------------------------------------------------------------- _headers

#[test]
fn snapshot_emit_headers() {
    let mut sec = BTreeMap::new();
    sec.insert("X-Frame-Options".to_string(), "DENY".to_string());
    sec.insert("X-Content-Type-Options".to_string(), "nosniff".to_string());
    let mut sec2 = BTreeMap::new();
    sec2.insert("Cache-Control".to_string(), "public, max-age=31536000".to_string());
    let rules = vec![
        ("/*".to_string(), sec),
        ("/assets/*".to_string(), sec2),
    ];
    insta::assert_snapshot!("emit_headers", emit_headers(&rules));
}

#[test]
fn snapshot_emit_headers_empty() {
    insta::assert_snapshot!("emit_headers_empty", emit_headers(&[]));
}

// ---------------------------------------------------------------- _redirects

#[test]
fn snapshot_emit_redirects() {
    let rules = vec![
        ("/old/:slug".to_string(), "/new/:slug".to_string(), 308u16),
        ("/legacy".to_string(), "/".to_string(), 0), // 0 -> 301 default
        ("/gone".to_string(), "/404".to_string(), 410),
    ];
    insta::assert_snapshot!("emit_redirects", emit_redirects(&rules));
}

#[test]
fn snapshot_emit_redirects_empty() {
    insta::assert_snapshot!("emit_redirects_empty", emit_redirects(&[]));
}

// ---------------------------------------------------------------- docker

#[test]
fn snapshot_dockerfile_embed_off() {
    insta::assert_snapshot!("dockerfile_embed_off", dockerfile(false));
}

#[test]
fn snapshot_dockerfile_embed_on() {
    insta::assert_snapshot!("dockerfile_embed_on", dockerfile(true));
}

// ---------------------------------------------------------------- cloudflare

#[test]
fn snapshot_cloudflare_worker_entry() {
    insta::assert_snapshot!(
        "cloudflare_worker_entry",
        cloudflare_worker_entry("build-abc123")
    );
}


// ---------------------------------------------------------------- node server entry (§18)

#[test]
fn snapshot_node_server_entry() {
    insta::assert_snapshot!("node_server_entry", node_server_entry("build-abc123"));
}

// ---------------------------------------------------------------- vercel functions manifest (§18)

/// Recursively sort object keys so the serialized form is deterministic.
///
/// `serde_json::Value`'s key order depends on the `preserve_order` feature,
/// which is **workspace-wide** (any crate that enables it flips it for all):
/// `syntect`'s `yaml-load` feature turns it on, so a `-p oxide-adapters` test
/// sees sorted keys while `--workspace` sees insertion order. A snapshot must
/// not depend on that — it would pass in isolation and fail in CI. Sorting
/// here makes the assertion feature-independent.
fn sorted_json(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                out.insert(k.clone(), sorted_json(&map[k]));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(sorted_json).collect())
        }
        other => other.clone(),
    }
}

#[test]
fn snapshot_vercel_functions_json() {
    let v = vercel_functions_json("build-abc123");
    // Sort so the snapshot is identical whether or not `serde_json`'s
    // `preserve_order` is unified in (see `sorted_json`'s doc comment).
    let normalized = sorted_json(&v);
    insta::assert_snapshot!(
        "vercel_functions_json",
        serde_json::to_string_pretty(&normalized).unwrap()
    );
}
