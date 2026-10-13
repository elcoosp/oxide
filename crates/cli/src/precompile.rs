//! §6.3 precompile pass for Vue / Svelte single-file components.
//!
//! The bundler is synchronous and has no access to the tooling pool. The
//! CLI runs this async pass once per env before invoking the bundler: it
//! walks every entry (and transitively, every non-JS import the entry
//! reaches) and asks the worker to compile any `.vue` / `.svelte` module
//! to plain JS. The result is a `path → js` map that the bundler consumes
//! via `BundleOptions::precompiled`.
//!
//! The pass is best-effort per file: a compile failure records a diagnostic
//! on stderr and skips that file. Downstream, the bundler surfaces a
//! `BLD-FRAMEWORK` diagnostic naming the file when it reaches a `.vue` or
//! `.svelte` module without a precompiled entry.

use anyhow::Context;
use oxide_jshost::{CallInput, EntryRef, Pool};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Resolve relative imports the way the bundler does, best-effort. We only
/// need to find SFCs the graph reaches, so a simple extension append is
/// enough for the precompiler — the bundler's full resolver runs later and
/// reports the real resolution errors.
fn resolve_relative(from_file: &Path, spec: &str) -> Option<PathBuf> {
    if !spec.starts_with('.') {
        return None;
    }
    let base = from_file.parent()?.join(spec);
    for cand in [
        base.clone(),
        base.with_extension("vue"),
        base.with_extension("svelte"),
        base.with_extension("tsx"),
        base.with_extension("ts"),
        base.with_extension("js"),
    ] {
        if cand.is_file() {
            return Some(cand);
        }
    }
    // Try ./Dir/index.vue style.
    for ext in ["vue", "svelte", "ts", "tsx", "js"] {
        let idx = base.join(format!("index.{ext}"));
        if idx.is_file() {
            return Some(idx);
        }
    }
    None
}

/// Best-effort static import scan: literal `import ... from "..."` and
/// `export ... from "..."`. Enough to find SFC dependencies; the bundler
/// re-scans with the real scanner.
fn scan_import_specs(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("import ") {
            if let Some(idx) = rest.find(" from ") {
                if let Some(open) = rest[idx + 6..].find('"') {
                    let after = &rest[idx + 6 + open + 1..];
                    if let Some(close) = after.find('"') {
                        out.push(after[..close].to_string());
                    }
                }
            }
        } else if let Some(rest) = t.strip_prefix("export ") {
            if let Some(idx) = rest.find(" from ") {
                if let Some(open) = rest[idx + 6..].find('"') {
                    let after = &rest[idx + 6 + open + 1..];
                    if let Some(close) = after.find('"') {
                        out.push(after[..close].to_string());
                    }
                }
            }
        }
    }
    out
}

async fn call_pool(pool: &Arc<dyn Pool>, kind: &str, payload: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let req = CallInput {
        payload,
        headers: Default::default(),
        has_body: false,
        request_id: format!("precompile:{kind}"),
    };
    let entry = EntryRef {
        kind: kind.to_string(),
        module: String::new(),
        export: String::new(),
    };
    let mut stream = pool
        .call(entry, req)
        .await
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    let bytes = stream
        .collect()
        .await
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).context("worker returned invalid JSON")?;
    if let Some(err) = v.get("__oxide_error").and_then(|x| x.as_str()) {
        anyhow::bail!("{err}");
    }
    Ok(v)
}

async fn compile_vue(pool: &Arc<dyn Pool>, path: &Path, source: &str) -> anyhow::Result<String> {
    let v = call_pool(
        pool,
        "tooling:vue-compile",
        json!({ "source": source, "filename": path.to_string_lossy() }),
    )
    .await?;
    v.get("js")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("vue-compile returned no `js` field"))
}

async fn compile_svelte(pool: &Arc<dyn Pool>, path: &Path, source: &str, generate: &str) -> anyhow::Result<String> {
    let v = call_pool(
        pool,
        "tooling:svelte-compile",
        json!({
            "source": source,
            "filename": path.to_string_lossy(),
            "generate": generate,
        }),
    )
    .await?;
    v.get("js")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("svelte-compile returned no `js` field"))
}

/// Walk the entry set and precompile every reachable `.vue` / `.svelte` file.
/// Returns a `path → JS` map ready for `BundleOptions::precompiled`.
pub async fn precompile_entries(
    pool: Option<&Arc<dyn Pool>>,
    entries: &BTreeMap<String, PathBuf>,
    svelte_generate: &str,
) -> BTreeMap<PathBuf, String> {
    let Some(pool) = pool else {
        return BTreeMap::new();
    };
    let mut out: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut queue: VecDeque<PathBuf> = entries.values().cloned().collect();

    while let Some(path) = queue.pop_front() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };

        // Always scan for imports, so SFC→SFC chains are followed.
        for spec in scan_import_specs(&source) {
            if let Some(dep) = resolve_relative(&path, &spec) {
                queue.push_back(dep);
            }
        }

        match ext {
            "vue" => match compile_vue(pool, &path, &source).await {
                Ok(js) => {
                    // Compiled JS may itself import other .vue files (rare but
                    // possible via `defineAsyncComponent`); the scan above
                    // already covered the pre-compile source, which is where
                    // those imports live in practice.
                    out.insert(path, js);
                }
                Err(e) => {
                    eprintln!("warn: vue precompile failed for {}: {e}", path.display());
                }
            },
            "svelte" => match compile_svelte(pool, &path, &source, svelte_generate).await {
                Ok(js) => {
                    out.insert(path, js);
                }
                Err(e) => {
                    eprintln!("warn: svelte precompile failed for {}: {e}", path.display());
                }
            },
            _ => { /* JS/TS/TSX handled by the bundler's Oxc lowering */ }
        }
    }
    out
}
