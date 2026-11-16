//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// `oxide secrets rotate` (§9.4 / BUG-05): delete `.oxide/secret`.
pub(crate) async fn cmd_secrets(root: &str, op: SecretsOp, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    match op {
        SecretsOp::Rotate => {
            let f = rootp.join(".oxide/secret");
            match std::fs::remove_file(&f) {
                Ok(()) => outln!(
                    w,
                    "[ok] removed {} — next build mints a new secret",
                    f.display()
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    outln!(w, "[--] no secret at {} (nothing to rotate)", f.display())
                }
                Err(e) => anyhow::bail!("cannot remove {}: {e}", f.display()),
            }
        }
    }
    Ok(0)
}

/// `oxide cache <inspect|clear|stats>` (§19.1): operates directly on the
/// effective cache root — the `OXIDE_CACHE_DIR` override or the historical
/// `<root>/.oxide/cache` (§V2-F18).
pub(crate) async fn cmd_cache(root: &str, op: CacheOp, w: Writer<'_>) -> anyhow::Result<i32> {
    let dir = perf_bisect::cache_dir_for(Path::new(root));
    let mut files: Vec<PathBuf> = Vec::new();
    walk_files(&dir, &mut files);
    match op {
        CacheOp::Inspect => {
            if files.is_empty() {
                outln!(w, "cache empty ({})", dir.display());
            }
            for f in &files {
                outln!(w, "{}", f.display());
            }
        }
        CacheOp::Clear => {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            outln!(w, "[ok] cache cleared ({})", dir.display());
        }
        CacheOp::Stats => {
            let total: u64 = files
                .iter()
                .filter_map(|f| std::fs::metadata(f).ok())
                .map(|m| m.len())
                .sum();
            outln!(w, "files: {}", files.len());
            outln!(w, "total_bytes: {total}");
        }
        CacheOp::Gc {
            idle_secs,
            max_bytes,
        } => {
            // SPEC-5D: two-phase GC — first drop entries older than the idle
            // window, then if still over cap, drop the OLDEST remaining
            // entries until under `max_bytes`. The pipeline never reads a GC'd
            // entry back (they are content-addressed; a re-compute re-creates
            // the same key), so eviction is safe mid-build.
            let now = std::time::SystemTime::now();
            let idle = std::time::Duration::from_secs(idle_secs);
            let mut sizes: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = files
                .iter()
                .filter_map(|f| {
                    let meta = std::fs::metadata(f).ok()?;
                    Some((f.clone(), meta.len(), meta.modified().ok()?))
                })
                .collect();
            // Phase 1: idle eviction.
            let mut idle_removed = 0usize;
            let mut idle_bytes = 0u64;
            sizes.retain(|(path, bytes, mtime)| {
                if now
                    .duration_since(*mtime)
                    .map(|d| d > idle)
                    .unwrap_or(false)
                {
                    if std::fs::remove_file(path).is_ok() {
                        idle_removed += 1;
                        idle_bytes += *bytes;
                    }
                    false
                } else {
                    true
                }
            });
            // Phase 2: cap eviction (oldest first).
            let mut total: u64 = sizes.iter().map(|(_, b, _)| *b).sum();
            let mut cap_removed = 0usize;
            let mut cap_bytes = 0u64;
            if total > max_bytes {
                sizes.sort_by_key(|(_, _, m)| *m);
                for (path, bytes, _) in &sizes {
                    if total <= max_bytes {
                        break;
                    }
                    if std::fs::remove_file(path).is_ok() {
                        total = total.saturating_sub(*bytes);
                        cap_removed += 1;
                        cap_bytes += *bytes;
                    }
                }
            }
            outln!(
                w,
                "[ok] gc: idle_removed={idle_removed} ({idle_bytes} B), cap_removed={cap_removed} ({cap_bytes} B), remaining={} ({} B)",
                sizes.len().saturating_sub(cap_removed),
                total,
            );
        }
    }
    Ok(0)
}

/// `oxide audit` (§17.2): static advisory pass over `.oxide/modules.json` + config
/// plugins. The real registry feed plugs in behind the same output shape.
pub(crate) async fn cmd_audit(root: &str, json: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let offline = std::env::var_os("OXIDE_AUDIT_OFFLINE").is_some();

    // SPEC-19.1 / §19-B: the real advisory scan reads the lockfile's
    // installed packages and queries OSV for each. `offline` (env var) is
    // the CI-safe mode — a fresh cache hit is served, a miss is
    // AUDIT-OFFLINE.
    let pkgs = match crate::audit::read_package_lock(&rootp) {
        Ok(p) => p,
        Err(e) => {
            if json {
                let err_json = serde_json::json!({
                    "root": rootp.to_string_lossy(),
                    "error": format!("{e:#}"),
                    "exit_code": 2,
                });
                outln!(w, "{}", serde_json::to_string_pretty(&err_json)?);
            } else {
                outln!(w, "{e:#}");
            }
            return Ok(2);
        }
    };

    let advisories = match crate::audit::query_osv(&rootp, &pkgs, offline).await {
        Ok(a) => a,
        Err(e) => {
            if json {
                let err_json = serde_json::json!({
                    "root": rootp.to_string_lossy(),
                    "package_count": pkgs.len(),
                    "error": format!("{e:#}"),
                    "exit_code": 2,
                });
                outln!(w, "{}", serde_json::to_string_pretty(&err_json)?);
            } else {
                outln!(w, "{e:#}");
            }
            return Ok(2);
        }
    };

    // Exit 1 when any HIGH/CRITICAL is present (CI gate convention); a
    // low/moderate advisory is a warning-not-a-failure so a transitive
    // unfixable LOW doesn't block every build.
    let fatal = advisories
        .iter()
        .any(|a| matches!(a.severity.as_str(), "HIGH" | "CRITICAL"));
    let exit_code = if fatal { 1 } else { 0 };

    if json {
        // Stable machine-readable schema. A CI consumer can parse this
        // without depending on the human message wording (which the plan
        // has re-versioned twice since r4).
        let adv_json: Vec<serde_json::Value> = advisories
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "package": a.package,
                    "version": a.version,
                    "severity": if a.severity.is_empty() { "?" } else { a.severity.as_str() },
                    "summary": a.summary,
                })
            })
            .collect();
        let report = serde_json::json!({
            "root": rootp.to_string_lossy(),
            "package_count": pkgs.len(),
            "advisories": adv_json,
            "fatal": fatal,
            "exit_code": exit_code,
        });
        outln!(w, "{}", serde_json::to_string_pretty(&report)?);
        return Ok(exit_code);
    }

    outln!(
        w,
        "auditing {} installed npm package(s) via OSV",
        pkgs.len()
    );
    if advisories.is_empty() {
        outln!(w, "[ok] no advisories");
        return Ok(exit_code);
    }
    outln!(
        w,
        "{} advisor{} found:",
        advisories.len(),
        if advisories.len() == 1 { "y" } else { "ies" }
    );
    for a in &advisories {
        let sev = if a.severity.is_empty() {
            "?"
        } else {
            a.severity.as_str()
        };
        outln!(
            w,
            "  {} {}@{}  [{}]  {}",
            a.id,
            a.package,
            a.version,
            sev,
            a.summary
        );
    }
    Ok(exit_code)
}

pub(crate) async fn cmd_mcp(root: &str, allow_write: bool, _w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    // Progress/log go to stderr so stdout stays pure JSON-RPC.
    eprintln!(
        "oxide mcp: serving project at {} (allow_write: {})",
        rootp.display(),
        allow_write
    );
    crate::mcp::serve_stdio(rootp, allow_write).await?;
    Ok(0)
}

pub(crate) async fn cmd_test(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let client = rootp.join("dist/client/client.manifest.json");
    let server = rootp.join("dist/server/server.manifest.json");
    if client.is_file() && server.is_file() {
        outln!(w, "[ok] build manifests present");
        outln!(
            w,
            "use the in-process fetch helper from packages/oxide (oxide/test, §16.6)"
        );
        Ok(0)
    } else {
        outln!(
            w,
            "[fail] missing build manifests — run `oxide build` first"
        );
        Ok(1)
    }
}
