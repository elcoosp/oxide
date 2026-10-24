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

use crate::precompile_cache::{PrecompileCache, PrecompileKey};
use anyhow::Context;
use oxide_fw_jshost::{CallInput, EntryRef, Pool};
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
        base.with_extension("mdx"),
        base.with_extension("tsx"),
        base.with_extension("ts"),
        base.with_extension("js"),
    ] {
        if cand.is_file() {
            return Some(cand);
        }
    }
    // Try ./Dir/index.vue style.
    for ext in ["vue", "svelte", "mdx", "ts", "tsx", "js"] {
        let idx = base.join(format!("index.{ext}"));
        if idx.is_file() {
            return Some(idx);
        }
    }
    None
}

/// Import scan via the compiler's byte-span scanner (BUG-13): the old
/// line-based scan missed multi-line `import {\n A,\n} from 'x'` forms,
/// leaving the import graph incomplete. The bundler re-scans with the same
/// real scanner, so the CAS key is unchanged.
fn scan_import_specs(src: &str) -> Vec<String> {
    oxide_fw_compiler::scan_import_spans(src)
        .into_iter()
        .map(|sp| sp.specifier)
        .collect()
}

/// SPEC-16-C: compile a `.mdx` file via \`@mdx-js/mdx\` in the tooling pool.
/// Returns `None` when the compile failed (best-effort — same convention as
/// \`.vue\`/\`.svelte\`).
async fn compile_mdx(pool: &Arc<dyn Pool>, path: &Path) -> anyhow::Result<Option<String>> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("input.mdx");
    let out = call_pool(
        pool,
        "tooling:module",
        serde_json::json!({
            "module": "@mdx-js/mdx",
            "export": "compile",
            "args": [
                source,
                {
                    "outputFormat": "program",
                    "jsx": true,
                    "jsxImportSource": "react"
                }
            ],
        }),
    )
    .await?;
    if let Some(s) = out.get("code").and_then(|v| v.as_str()) {
        return Ok(Some(s.to_string()));
    }
    if let Some(s) = out.as_str() {
        return Ok(Some(s.to_string()));
    }
    if let Some(v) = out.get("value").and_then(|v| v.as_str()) {
        return Ok(Some(v.to_string()));
    }
    Ok(None)
}

async fn call_pool(
    pool: &Arc<dyn Pool>,
    kind: &str,
    payload: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let req = CallInput {
        payload,
        headers: Default::default(),
        has_body: false,
        request_id: format!("precompile:{kind}"),
        route: None,
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
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).context("worker returned invalid JSON")?;
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

async fn compile_solid(
    pool: &Arc<dyn Pool>,
    path: &Path,
    source: &str,
    ssr: bool,
) -> anyhow::Result<String> {
    let v = call_pool(
        pool,
        "tooling:solid-compile",
        json!({
            "source": source,
            "filename": path.to_string_lossy(),
            "ssr": ssr,
        }),
    )
    .await?;
    v.get("js")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("solid-compile returned no `js` field"))
}

async fn compile_svelte(
    pool: &Arc<dyn Pool>,
    path: &Path,
    source: &str,
    generate: &str,
) -> anyhow::Result<String> {
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

/// A cache key for one file. `None` framework falls back to the file extension.
fn cache_key_for(
    path: &Path,
    source: &str,
    framework: &str,
    generate: &str,
    tool_version: &str,
) -> PrecompileKey {
    // Include the path in the source digest so two identical files in different
    // directories do not share an entry — the compiler output can embed the
    // filename (Vue scope ids, Svelte css hashing), so path is an input.
    let mut h = blake3::Hasher::new();
    h.update(path.to_string_lossy().as_bytes());
    h.update(&[0]);
    h.update(source.as_bytes());
    PrecompileKey {
        source_hash: h.finalize().to_hex().to_string(),
        framework: framework.to_string(),
        generate: generate.to_string(),
        tool_version: tool_version.to_string(),
    }
}

/// Run `compile`, consulting `cache` first. On a hit the compiler is skipped
/// entirely; on a miss the result is stored. A cache error is logged and the
/// freshly compiled value is returned.

/// §5.4: cached version of a compiler that may legitimately return
/// `None` (an empty MDX file compiles to nothing). Mirrors `cached_compile`,
/// but distinguishes "compiled to nothing" (Ok(None), not cached) from
/// "compiled to JS" (Ok(Some(js))).
async fn cached_compile_optional<F, Fut>(
    cache: Option<&PrecompileCache>,
    key: PrecompileKey,
    compile: F,
) -> anyhow::Result<Option<String>>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<String>>>,
{
    if let Some(c) = cache {
        if let Some(hit) = c.get(&key) {
            return Ok(Some(hit));
        }
        if let Some(hit) = c.get_remote(&key).await {
            let _ = c.put(&key, &hit);
            return Ok(Some(hit));
        }
    }
    let result = compile().await?;
    if let (Some(js), Some(c)) = (&result, cache) {
        let _ = c.put(&key, js);
    }
    Ok(result)
}

async fn cached_compile<F, Fut>(
    cache: Option<&PrecompileCache>,
    key: PrecompileKey,
    compile: F,
) -> anyhow::Result<String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<String>>,
{
    if let Some(c) = cache {
        if let Some(hit) = c.get(&key) {
            return Ok(hit);
        }
        // §5.4: on a local miss, try the remote read-through before
        // compiling. A remote hit is promoted to the local CAS so the next
        // build is a local hit.
        if let Some(hit) = c.get_remote(&key).await {
            let _ = c.put(&key, &hit);
            return Ok(hit);
        }
    }
    let js = compile().await?;
    if let Some(c) = cache {
        if let Err(e) = c.put(&key, &js) {
            eprintln!("warn: precompile cache store failed: {}", e.0.message);
        }
        // §5.4: write back through the remote so another job gets a hit.
        c.put_remote(&key, &js).await;
    }
    Ok(js)
}

/// Walk the entry set and precompile every reachable `.vue` / `.svelte` file.
/// Returns a `path → JS` map ready for `BundleOptions::precompiled`.
pub async fn precompile_entries(
    pool: Option<&Arc<dyn Pool>>,
    entries: &BTreeMap<String, PathBuf>,
    svelte_generate: &str,
    framework: Option<&str>,
    cache: Option<&PrecompileCache>,
    tool_version: &str,
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

        // §6.3: Solid's JSX compiler is `babel-preset-solid`, not Oxc's
        // automatic runtime — its output is `ssrElement`/template calls,
        // not `jsx()` factories. Route `.tsx`/`.jsx` through it when the
        // project declares the Solid adapter.
        if matches!(ext, "tsx" | "jsx") && framework == Some("solid") {
            let ssr = svelte_generate == "ssr";
            let key = cache_key_for(&path, &source, "solid", svelte_generate, tool_version);
            match cached_compile(cache, key, || compile_solid(pool, &path, &source, ssr)).await {
                Ok(js) => {
                    out.insert(path, js);
                }
                Err(e) => {
                    eprintln!("warn: solid precompile failed for {}: {e}", path.display());
                }
            }
            continue;
        }
        match ext {
            "vue" => {
                let key = cache_key_for(&path, &source, "vue", svelte_generate, tool_version);
                match cached_compile(cache, key, || compile_vue(pool, &path, &source)).await {
                    Ok(js) => {
                        // Compiled JS may itself import other .vue files (rare
                        // but possible via `defineAsyncComponent`); the scan
                        // above already covered the pre-compile source.
                        out.insert(path, js);
                    }
                    Err(e) => {
                        eprintln!("warn: vue precompile failed for {}: {e}", path.display());
                    }
                }
            }
            "svelte" => {
                let key = cache_key_for(&path, &source, "svelte", svelte_generate, tool_version);
                match cached_compile(cache, key, || {
                    compile_svelte(pool, &path, &source, svelte_generate)
                })
                .await
                {
                    Ok(js) => {
                        out.insert(path, js);
                    }
                    Err(e) => {
                        eprintln!("warn: svelte precompile failed for {}: {e}", path.display());
                    }
                }
            }
            "mdx" => {
                // SPEC-16-C / §5.4: compile via @mdx-js/mdx in the
                // tooling pool, wrapped in the precompile cache so a
                // warm build reuses the compiled output instead of
                // re-invoking the JS compiler on every run.
                let key = cache_key_for(&path, &source, "mdx", svelte_generate, tool_version);
                match cached_compile_optional(cache, key, || async {
                    compile_mdx(pool, &path).await
                })
                .await
                {
                    Ok(Some(js)) => {
                        out.insert(path, js);
                    }
                    Ok(None) => { /* empty */ }
                    Err(e) => eprintln!("warn: mdx compile {}: {e}", path.display()),
                }
            }
            _ => { /* JS/TS/TSX handled by the bundler's Oxc lowering */ }
        }
    }
    out
}
