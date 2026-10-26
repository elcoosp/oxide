//! SPEC-19.1 / §19-B: `oxide audit` — advisory scan over the project's
//! installed npm dependency tree.
//!
//! Reads `package-lock.json` (v2/v3 `packages` map), builds one OSV
//! `querybatch` request for the whole set, POSTs it to `api.osv.dev`, and
//! reports each advisory against its package + installed version.
//!
//! ## Offline + cache
//!
//! Every successful query is cached at `.oxide/cache/osv-advisories.json`
//! with a 24-hour TTL. `OXIDE_AUDIT_OFFLINE=1` forces the cache path: a
//! cache hit under 24 h is used; a miss (or an expired cache) is a loud
//! `AUDIT-OFFLINE` diagnostic, never a silent pass.
//!
//! ## Supported lockfile shape
//!
//! `package-lock.json` v2/v3 (the `packages` map with `node_modules/x`
//! paths). A project that ships only `pnpm-lock.yaml` or `yarn.lock` is a
//! documented limitation — `oxide audit` emits `AUDIT-LOCKFORMAT` naming
//! the unsupported file rather than guess. Generating a `package-lock.json`
//! alongside (`npm i --package-lock-only`) is a one-liner workaround.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const OSV_API: &str = "https://api.osv.dev/v1/querybatch";
const CACHE_TTL_HOURS: u64 = 24;

/// One installed npm package (name + exact installed version).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
}

/// One reported advisory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Advisory {
    pub package: String,
    pub version: String,
    pub id: String,
    pub summary: String,
    /// `"LOW"|"MODERATE"|"HIGH"|"CRITICAL"` (or empty when OSV gave no
    /// severity). Sourced from the first `affected[].ecosystem_specific
    /// .severity[]` entry, then from the `database_specific.severity`
    /// field — OSV's own precedence.
    pub severity: String,
}

/// The cache file shape.
#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    fetched_at: String,
    ecosystem: String,
    /// blake3 hex of the sorted `name@version` list — a mismatched query set
    /// invalidates the cache without a time check.
    queries_hash: String,
    advisories: Vec<Advisory>,
}

/// Read `package-lock.json` (v2/v3) and collect every `node_modules/x`
/// package. Skips the root package (`""` path) and any `file:` / `link:`
/// workspace entry (no `version` field, or an obvious path).
pub fn read_package_lock(root: &Path) -> anyhow::Result<Vec<InstalledPackage>> {
    let path = root.join("package-lock.json");
    if !path.is_file() {
        return Err(anyhow::anyhow!(
            "AUDIT-LOCKFORMAT: no `package-lock.json` at {}; \
             pnpm-lock.yaml / yarn.lock are not parsed by this build — run \
             `npm i --package-lock-only` to generate one",
            root.display(),
        ));
    }
    let bytes = std::fs::read(&path)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("AUDIT-LOCKFORMAT: parse {}: {e}", path.display()))?;
    let packages = v
        .get("packages")
        .and_then(|p| p.as_object())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "AUDIT-LOCKFORMAT: {} has no `packages` map (lockfileVersion < 2)",
                path.display(),
            )
        })?;
    let mut out = Vec::new();
    for (path_key, entry) in packages {
        if path_key.is_empty() || !path_key.starts_with("node_modules/") {
            continue;
        }
        // Skip nested duplicates like `node_modules/x/node_modules/y` — the
        // top-level entry (or the deepest non-collided one) is enough for a
        // security scan; OSV dedups by (name, version) anyway.
        let name = path_key.trim_start_matches("node_modules/").to_string();
        if name.contains("/node_modules/") {
            continue;
        }
        let Some(version) = entry.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        if version.starts_with("link:") || version.starts_with("file:") {
            continue;
        }
        // Skip the root workspace package that npm sometimes renames.
        if entry.get("link").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        out.push(InstalledPackage {
            name,
            version: version.to_string(),
        });
    }
    out.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    out.dedup();
    Ok(out)
}

fn queries_hash(pkgs: &[InstalledPackage]) -> String {
    let mut h = blake3::Hasher::new();
    for p in pkgs {
        h.update(p.name.as_bytes());
        h.update(b"@");
        h.update(p.version.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex().to_string()
}

fn cache_path(root: &Path) -> PathBuf {
    root.join(".oxide/cache/osv-advisories.json")
}

/// Try the cache: returns `Some(advisories)` on a fresh hit, `None` on
/// miss/expired/hash-mismatch.
fn read_cache(root: &Path, pkgs: &[InstalledPackage]) -> Option<Vec<Advisory>> {
    let bytes = std::fs::read(cache_path(root)).ok()?;
    let cache: CacheFile = serde_json::from_slice(&bytes).ok()?;
    if cache.ecosystem != "npm" {
        return None;
    }
    if cache.queries_hash != queries_hash(pkgs) {
        return None;
    }
    // Freshness: parse the ISO-8601 timestamp back to SystemTime. A parse
    // failure is a miss (corrupt cache).
    let fetched: chrono::DateTime<chrono::Utc> = cache.fetched_at.parse().ok()?;
    let fetched_ms = fetched.timestamp_millis().max(0) as u64;
    let now_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    if now_ms.saturating_sub(fetched_ms) > CACHE_TTL_HOURS * 3600 * 1000 {
        return None;
    }
    Some(cache.advisories)
}

fn write_cache(root: &Path, pkgs: &[InstalledPackage], advisories: &[Advisory]) {
    let path = cache_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let payload = CacheFile {
        fetched_at: chrono::Utc::now().to_rfc3339(),
        ecosystem: "npm".to_string(),
        queries_hash: queries_hash(pkgs),
        advisories: advisories.to_vec(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&payload) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// The OSV `querybatch` wire shape (v1). One `query` per installed package.
#[derive(Serialize)]
struct OsvBatchRequest<'a> {
    queries: Vec<OsvQuery<'a>>,
}

#[derive(Serialize)]
struct OsvQuery<'a> {
    package: OsvPackageRef<'a>,
    version: &'a str,
}

#[derive(Serialize)]
struct OsvPackageRef<'a> {
    name: &'a str,
    ecosystem: &'a str,
}

#[derive(Deserialize)]
struct OsvBatchResponse {
    results: Vec<OsvBatchResult>,
}

#[derive(Deserialize, Default)]
struct OsvBatchResult {
    #[serde(default)]
    vulns: Vec<OsvVuln>,
}

#[derive(Deserialize, Default)]
struct OsvVuln {
    id: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    affected: Vec<OsvAffected>,
    #[serde(default)]
    database_specific: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
struct OsvAffected {
    #[serde(default)]
    ecosystem_specific: Option<serde_json::Value>,
}

fn severity_of(v: &OsvVuln) -> String {
    // Priority 1: affected[].ecosystem_specific.severity[] (the canonical
    // place a CVE's severity lands for npm advisories).
    for a in &v.affected {
        if let Some(es) = &a.ecosystem_specific {
            if let Some(sev) = es.get("severity").and_then(|s| s.as_str()) {
                return sev.to_ascii_uppercase();
            }
            if let Some(arr) = es.get("severity").and_then(|s| s.as_array())
                && let Some(s) = arr.first().and_then(|v| v.as_str())
            {
                return s.to_ascii_uppercase();
            }
        }
    }
    // Priority 2: database_specific.severity.
    if let Some(db) = &v.database_specific
        && let Some(sev) = db.get("severity").and_then(|s| s.as_str())
    {
        return sev.to_ascii_uppercase();
    }
    String::new()
}

/// Query OSV for the given installed packages. `offline` disables the
/// network entirely (cache-only); with `offline`, a cache miss is an error.
pub async fn query_osv(
    root: &Path,
    pkgs: &[InstalledPackage],
    offline: bool,
) -> anyhow::Result<Vec<Advisory>> {
    // Cache hit wins regardless of `offline` — the freshness + hash checks
    // already gate it.
    if let Some(advisories) = read_cache(root, pkgs) {
        return Ok(advisories);
    }
    if offline {
        return Err(anyhow::anyhow!(
            "AUDIT-OFFLINE: no fresh OSV cache at {} and OXIDE_AUDIT_OFFLINE=1; \
             unset the env var or re-run with network",
            cache_path(root).display(),
        ));
    }
    if pkgs.is_empty() {
        // No packages → no vulns. Cache the empty result so a subsequent
        // offline run in the same session is a hit.
        write_cache(root, pkgs, &[]);
        return Ok(Vec::new());
    }

    // Chunk at 1000 — OSV's documented batch limit is 1000 queries.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(format!("oxide-fw-cli/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build reqwest client")?;
    let mut advisories = Vec::new();
    for chunk in pkgs.chunks(1000) {
        let req = OsvBatchRequest {
            queries: chunk
                .iter()
                .map(|p| OsvQuery {
                    package: OsvPackageRef {
                        name: &p.name,
                        ecosystem: "npm",
                    },
                    version: &p.version,
                })
                .collect(),
        };
        let resp = client
            .post(OSV_API)
            .json(&req)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("AUDIT-NETWORK: POST {OSV_API}: {e}"))?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!(
                "AUDIT-NETWORK: {OSV_API} returned {}",
                resp.status()
            ));
        }
        let body: OsvBatchResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("AUDIT-NETWORK: decode OSV response: {e}"))?;
        if body.results.len() != chunk.len() {
            return Err(anyhow::anyhow!(
                "AUDIT-PROTOCOL: OSV returned {} results for {} queries",
                body.results.len(),
                chunk.len()
            ));
        }
        for (pkg, result) in chunk.iter().zip(body.results.iter()) {
            for vuln in &result.vulns {
                advisories.push(Advisory {
                    package: pkg.name.clone(),
                    version: pkg.version.clone(),
                    id: vuln.id.clone(),
                    summary: vuln.summary.clone(),
                    severity: severity_of(vuln),
                });
            }
        }
    }
    advisories.sort_by(|a, b| (&a.package, &a.id).cmp(&(&b.package, &b.id)));
    write_cache(root, pkgs, &advisories);
    Ok(advisories)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn reads_package_lock_v3_packages_map() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("package-lock.json"),
            r#"{
              "lockfileVersion": 3,
              "packages": {
                "": {"name": "root", "version": "1.0.0"},
                "node_modules/lodash": {"version": "4.17.20"},
                "node_modules/react": {"version": "19.0.0"},
                "node_modules/react/node_modules/loose-envify": {"version": "1.0.0"},
                "node_modules/ws": {"link": true}
              }
            }"#,
        );
        let pkgs = read_package_lock(tmp.path()).unwrap();
        // lodash + react land; the nested loose-envify is dropped (path
        // contains /node_modules/), ws is dropped (link:true).
        assert_eq!(
            pkgs,
            vec![
                InstalledPackage {
                    name: "lodash".into(),
                    version: "4.17.20".into()
                },
                InstalledPackage {
                    name: "react".into(),
                    version: "19.0.0".into()
                },
            ]
        );
    }

    #[test]
    fn missing_lockfile_is_a_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        let e = read_package_lock(tmp.path()).unwrap_err();
        let msg = format!("{e:#}");
        assert!(msg.contains("AUDIT-LOCKFORMAT"), "{msg}");
        assert!(msg.contains("pnpm-lock.yaml"), "{msg}");
    }

    #[test]
    fn cache_roundtrips_within_ttl() {
        let tmp = tempfile::tempdir().unwrap();
        let pkgs = vec![InstalledPackage {
            name: "x".into(),
            version: "1.0.0".into(),
        }];
        let advisories = vec![Advisory {
            package: "x".into(),
            version: "1.0.0".into(),
            id: "OSV-1".into(),
            summary: "bad".into(),
            severity: "HIGH".into(),
        }];
        write_cache(tmp.path(), &pkgs, &advisories);
        let hit = read_cache(tmp.path(), &pkgs).expect("fresh hit");
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].id, "OSV-1");
    }

    #[test]
    fn cache_misses_on_query_hash_change() {
        let tmp = tempfile::tempdir().unwrap();
        let old_pkgs = vec![InstalledPackage {
            name: "x".into(),
            version: "1.0.0".into(),
        }];
        write_cache(tmp.path(), &old_pkgs, &[]);
        let new_pkgs = vec![InstalledPackage {
            name: "x".into(),
            version: "2.0.0".into(),
        }];
        assert!(read_cache(tmp.path(), &new_pkgs).is_none());
    }

    #[tokio::test]
    async fn offline_without_cache_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let pkgs = vec![InstalledPackage {
            name: "x".into(),
            version: "1.0.0".into(),
        }];
        let e = query_osv(tmp.path(), &pkgs, true).await.unwrap_err();
        let msg = format!("{e:#}");
        assert!(msg.contains("AUDIT-OFFLINE"), "{msg}");
    }

    #[tokio::test]
    async fn offline_with_fresh_cache_hits() {
        let tmp = tempfile::tempdir().unwrap();
        let pkgs = vec![InstalledPackage {
            name: "x".into(),
            version: "1.0.0".into(),
        }];
        write_cache(tmp.path(), &pkgs, &[]);
        let out = query_osv(tmp.path(), &pkgs, true).await.unwrap();
        assert!(out.is_empty(), "cached empty result: {out:?}");
    }
}
