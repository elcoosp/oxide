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
use oxide_fw_compiler::native::{Backend, CompileRequest, Generate, NativeRegistry};
use oxide_fw_compiler::precompile_engine::{PrecompileJob, compile_jobs, width_for};
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
            // BUG-MAPKEY: the returned path IS the precompiled-map key. A
            // lexical join keeps the specifier's `..` segments
            // (`/proj/src/routes/index/../../components/a.vue`), while the
            // bundler resolves the same import to the normalized
            // `/proj/src/components/a.vue` — the lookup missed, the raw SFC
            // reached Rolldown's parser, and the build died with the
            // file-less `Unexpected JSX expression`. Normalize so both sides
            // speak the same path spelling.
            return Some(oxide_fw_bundler::normalize(&cand));
        }
    }
    // Try ./Dir/index.vue style.
    for ext in ["vue", "svelte", "mdx", "ts", "tsx", "js"] {
        let idx = base.join(format!("index.{ext}"));
        if idx.is_file() {
            return Some(oxide_fw_bundler::normalize(&idx));
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
///
/// `jsx` stays `false` (the MDX default): the compile emits a pure-JS program
/// that calls the automatic runtime (`react/jsx-runtime`). With `jsx: true`
/// the output kept literal JSX syntax and the bundler rejected every MDX
/// chunk with `Unexpected JSX expression` — the doc-commented regression the
/// `bundle_edge_entry_accepts_mdx_reachable_from_the_entry` test pins.
async fn compile_mdx(pool: &Arc<dyn Pool>, path: &Path) -> anyhow::Result<Option<String>> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
    let _filename = path
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
                    "jsx": false,
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
/// freshly compiled value is returned.///
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
            tracing::warn!(error = %e.0.message, "precompile cache store failed");
        }
        // §5.4: write back through the remote so another job gets a hit.
        c.put_remote(&key, &js).await;
    }
    Ok(js)
}

/// The compile kind a discovered file routes to (§V2-B07 phase 1): the
/// Solid JSX compiler is `babel-preset-solid`, not Oxc's automatic
/// runtime — its output is `ssrElement`/template calls, not `jsx()`
/// factories. Route `.tsx`/`.jsx` through it when the project declares
/// the Solid adapter.
fn compile_kind(ext: &str, framework: Option<&str>) -> Option<&'static str> {
    match ext {
        "vue" => Some("vue"),
        "svelte" => Some("svelte"),
        "mdx" => Some("mdx"),
        "tsx" | "jsx" if framework == Some("solid") => Some("solid"),
        _ => None,
    }
}

/// §V2-B07 phase 1: the synchronous discovery walk. Every file the entry
/// set reaches (transitively through non-JS imports), with its source
/// text in hand and its compile kind resolved — no pool calls. The walk
/// helpers are the existing ones (the BUG-MAPKEY normalization, the
/// BUG-13 span scanner), unchanged. Deterministic: the queue is seeded
/// from the BTreeMap-ordered entries and every level is read in order.
fn discover_jobs(
    entries: &BTreeMap<String, PathBuf>,
    framework: Option<&str>,
) -> Vec<PrecompileJob> {
    let mut jobs: Vec<PrecompileJob> = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut queue: VecDeque<PathBuf> = entries.values().cloned().collect();
    while let Some(path) = queue.pop_front() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Always scan for imports, so SFC→SFC chains are followed.
        for spec in scan_import_specs(&source) {
            if let Some(dep) = resolve_relative(&path, &spec) {
                queue.push_back(dep);
            }
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if let Some(kind) = compile_kind(ext, framework) {
            jobs.push(PrecompileJob { path, kind, source });
        }
    }
    jobs
}

/// §V2-F17: the dev DOM→source stamp's compile-side placement — the source
/// the pool/native compile receives carries ` data-ox-src="src/<rel>:1"` on
/// the first element each component renders (the cost endpoint's `?src=`
/// spelling). The engine (`oxide_fw_compiler::inject_data_ox_src`) is the
/// one definition site for both halves: the JSX visitor owns `tsx`/`jsx`
/// (React and Solid), the template scan owns `vue`/`svelte`. A source the
/// engine declines (no stampable element, already stamped, a language
/// neither half owns) or a path outside `src_root` compiles the original
/// bytes unchanged — a skipped stamp costs the overlay nothing, a wrong one
/// misleads it.
fn stamped_source<'a>(
    source: &'a str,
    kind: &str,
    path: &Path,
    src_root: &Path,
) -> std::borrow::Cow<'a, str> {
    let lang = match kind {
        "vue" => "vue",
        "svelte" => "svelte",
        // Solid rides the JSX visitor on its own extension's source type.
        "solid" => match path.extension().and_then(|e| e.to_str()) {
            Some("tsx") => "tsx",
            _ => "jsx",
        },
        _ => return std::borrow::Cow::Borrowed(source),
    };
    let Ok(rel) = path.strip_prefix(src_root) else {
        return std::borrow::Cow::Borrowed(source);
    };
    let spelling = format!("src/{}", rel.to_string_lossy().replace('\\', "/"));
    match oxide_fw_compiler::inject_data_ox_src(source, lang, &spelling) {
        Ok(Some(stamped)) => std::borrow::Cow::Owned(stamped),
        _ => std::borrow::Cow::Borrowed(source),
    }
}

/// §V2-B07 phase 2's per-job pass — the spec's `compile_one`: the cache
/// consulted first (the CAS short-circuit), then the kind dispatch over
/// the existing pool compilers. `mdx` keeps its optional shape: a compile
/// that yields no JS adds no row (`Ok(None)`).
///
/// §V2-C01 tail: when a native registry is supplied, `pick` is consulted
/// **ahead of the pool branch** for the three SFC kinds (`solid`, `vue`,
/// `svelte` — `mdx` has no native backend yet, C10). A pick under
/// [`Backend::Native`] that finds no healthy backend refuses with
/// `CMP-NAT-001`; the refusal is propagated so the build fails — `native`
/// is the config author's hard request, and a silent pool fallback would
/// defeat it (D26). `auto`/`node` never surface a registry error.
async fn compile_one(
    pool: &Arc<dyn Pool>,
    native: Option<&Arc<NativeRegistry>>,
    backend: Backend,
    job: &PrecompileJob,
    svelte_generate: &str,
    cache: Option<&PrecompileCache>,
    tool_version: &str,
    data_ox_src: Option<&Path>,
) -> anyhow::Result<Option<String>> {
    let PrecompileJob { path, kind, source } = job;
    // §V2-F17: the dev pass compiles the STAMPED source — the stamp lands
    // BEFORE the compile (native or pool), so every downstream shape (the
    // CAS key fold, the pool payload, the native request) sees the stamped
    // bytes consistently, and the rendered DOM carries the attribute.
    let stamped;
    let source: &str = match data_ox_src {
        Some(src_root) => {
            stamped = stamped_source(source, kind, path, src_root);
            stamped.as_ref()
        }
        None => source,
    };
    // The D40 tool-version fold: a native pick keys its artifacts under
    // `native:{id}@{version}`, the pool under the CLI's version string —
    // switching `compilerBackend` never reuses a stale artifact.
    if !matches!(*kind, "mdx")
        && let Some(reg) = native
    {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match reg.pick(kind, ext, backend) {
            Ok(Some(compiler)) => {
                let native_tool_version = oxide_fw_compiler::native::cas_tool_version(compiler);
                let key = cache_key_for(path, source, kind, svelte_generate, &native_tool_version);
                let reg = Arc::clone(reg);
                let kind = *kind;
                let ext = ext.to_string();
                let path = path.clone();
                let source = source.to_string();
                let generate = if svelte_generate == "ssr" {
                    Generate::Ssr
                } else {
                    Generate::Dom
                };
                return cached_compile(cache, key, || async move {
                    // The compile is synchronous and CPU bound: the registry
                    // is held in an `Arc`, cloned into the blocking closure,
                    // and `pick` runs INSIDE it so the borrowed backend
                    // reference never has to be `'static` (the C01 note —
                    // no `unsafe`).
                    tokio::task::spawn_blocking(move || {
                        let picked = reg
                            .pick(kind, &ext, backend)
                            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?
                            .ok_or_else(|| {
                                anyhow::anyhow!("native backend vanished mid-compile")
                            })?;
                        picked
                            .compile(&CompileRequest {
                                path: &path,
                                source: &source,
                                generate,
                                dev: false,
                            })
                            .map(|o| o.js)
                            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))
                    })
                    .await?
                })
                .await
                .map(Some);
            }
            Ok(None) => { /* no healthy native backend — the pool compiles */ }
            Err(e) => return Err(anyhow::anyhow!("{} {}", e.0.code, e.0.message)),
        }
    }
    match *kind {
        "solid" => {
            let ssr = svelte_generate == "ssr";
            let key = cache_key_for(path, source, "solid", svelte_generate, tool_version);
            cached_compile(cache, key, || compile_solid(pool, path, source, ssr))
                .await
                .map(Some)
        }
        "vue" => {
            let key = cache_key_for(path, source, "vue", svelte_generate, tool_version);
            cached_compile(cache, key, || compile_vue(pool, path, source))
                .await
                .map(Some)
        }
        "svelte" => {
            let key = cache_key_for(path, source, "svelte", svelte_generate, tool_version);
            cached_compile(cache, key, || {
                compile_svelte(pool, path, source, svelte_generate)
            })
            .await
            .map(Some)
        }
        "mdx" => {
            // SPEC-16-C / §5.4: compile via @mdx-js/mdx in the tooling
            // pool, wrapped in the precompile cache so a warm build
            // reuses the compiled output instead of re-invoking the JS
            // compiler on every run.
            let key = cache_key_for(path, source, "mdx", svelte_generate, tool_version);
            cached_compile_optional(cache, key, || async { compile_mdx(pool, path).await }).await
        }
        // Discovery only produces the kinds above; the catch-all keeps
        // the match total without an unreachable path.
        _ => Ok(None),
    }
}

/// Walk the entry set and precompile every reachable `.vue` / `.svelte` /
/// `.mdx` file (and Solid's `.tsx`/`.jsx` when the project declares the
/// Solid adapter). Returns a `path → JS` map ready for
/// `BundleOptions::precompiled`.
///
/// §V2-B07: the walk is two phases — a synchronous discovery pass (no
/// pool calls) and a bounded-concurrency compile of the independent jobs
/// through the V8-free engine (`oxide-fw-compiler::precompile_engine`),
/// so the tooling pool's workers stop sitting idle while the walk awaits
/// each compile in turn. The window is the spec's
/// `pool.stats().workers * 2`; the output map is deterministic (input
/// order folded into the BTreeMap) and the pass stays best-effort per
/// file (§6.3): a compile failure warns and drops its own row only.
///
/// §V2-C01 tail: `native` is the compiled-in registry the per-file
/// `pick` consults ahead of the pool (`None` — the C01 default tree —
/// skips the native arm entirely), and `backend` is the resolved
/// `framework.compilerBackend`. Best-effort stays the rule for
/// `auto`/`node`; under `native` a registry refusal (`CMP-NAT-001`) is
/// the build's failure — the config author asked for the native path,
/// and a silent fallback would defeat the hard request.
/// The per-walk precompile inputs. The walk grew an eighth parameter with
/// the C01 native arm — past the arity budget, and every caller passes the
/// same build-scoped tuple (pool + entries + the framework shape + the
/// cache + the native registry/backend) — so the parameters are one struct
/// the call sites build. Field order mirrors the old signature; the
/// `native` registry stays owned (`Option<Arc<…>>`) so the per-job closure
/// can re-pick without a borrowed lifetime.
pub struct PrecompilePlan<'a> {
    /// The tooling pool (`None` — no worker host — skips the walk
    /// entirely and returns an empty map, the pre-C01 shape).
    pub pool: Option<&'a Arc<dyn Pool>>,
    /// The entry set the walk scans (`route id → file`).
    pub entries: &'a BTreeMap<String, PathBuf>,
    /// Svelte's generate target: `"dom"` for the client pass, `"ssr"`
    /// for the server pass.
    pub svelte_generate: &'a str,
    /// The resolved framework (`Some("vue")` switches the Solid kinds on).
    pub framework: Option<&'a str>,
    /// The precompile CAS (`None` compiles uncached — dev).
    pub cache: Option<&'a PrecompileCache>,
    /// The tool version folded into the CAS key (D40).
    pub tool_version: &'a str,
    /// The compiled-in native registry (`None` skips the native arm —
    /// the C01 default tree).
    pub native: Option<Arc<NativeRegistry>>,
    /// The resolved `framework.compilerBackend`.
    pub backend: Backend,
    /// §V2-F17: the DEV pass's DOM→source stamp — `Some(src_root)` compiles
    /// the STAMPED source (the engine's `inject_data_ox_src` runs before the
    /// pool/native compile, so the rendered DOM carries
    /// `data-ox-src="src/<rel>:1"` back to the component, the cost
    /// endpoint's `?src=` spelling). `None` — the production shape — never
    /// stamps; the CAS keys separate the two shapes because the stamp folds
    /// into the source hash. The dev SSR shim passes `Some(<root>/src)`;
    /// the build pass passes `None`.
    pub data_ox_src: Option<&'a Path>,
}

pub async fn precompile_entries(
    plan: PrecompilePlan<'_>,
) -> anyhow::Result<BTreeMap<PathBuf, String>> {
    let PrecompilePlan {
        pool,
        entries,
        svelte_generate,
        framework,
        cache,
        tool_version,
        native,
        backend,
        data_ox_src,
    } = plan;
    let Some(pool) = pool else {
        return Ok(BTreeMap::new());
    };
    let jobs = discover_jobs(entries, framework);
    let width = width_for(pool.stats().workers);
    let pool_ref = Arc::clone(pool);
    // Copy the borrow out first: the registry is only ever read through
    // the `Arc`, and a `&Arc` is `Copy` so the per-job closures capture it
    // without moving the caller's handle.
    let native_ref = native.as_ref();
    let results = compile_jobs(jobs, width, |job| {
        let pool = Arc::clone(&pool_ref);
        async move {
            compile_one(
                &pool,
                native_ref,
                backend,
                &job,
                svelte_generate,
                cache,
                tool_version,
                data_ox_src,
            )
            .await
        }
    })
    .await;
    let mut out = BTreeMap::new();
    for (path, result) in results {
        match result {
            Ok(Some(js)) if !js.is_empty() => {
                out.insert(path, js);
            }
            Ok(Some(js)) => {
                // A defensive empty row: not produced by any kind today,
                // but an empty module must not shadow the raw source.
                let _ = js;
                tracing::warn!(path = %path.display(), "precompile produced empty JS; skipped");
            }
            Ok(None) => { /* an mdx file that compiles to nothing adds no row */ }
            Err(e) => {
                // The `native` hard request: the registry's `CMP-NAT-001`
                // refusal fails the build instead of degrading to the
                // pool. Every other error stays the §6.3 best-effort skip.
                if backend == Backend::Native {
                    return Err(e);
                }
                tracing::warn!(path = %path.display(), error = %e, "framework precompile failed");
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod bug_mapkey_tests {

    //! BUG-MAPKEY: `resolve_relative` produces the precompiled-map keys. The
    //! historical failure: the lexical join kept the specifier's `..`
    //! segments, the bundler resolved the same import to the normalized path,
    //! the lookup missed, the raw SFC reached Rolldown's parser, and the
    //! build died with the file-less `Unexpected JSX expression`.

    use super::*;

    fn write_tree(root: &std::path::Path) -> std::path::PathBuf {
        let comp = root.join("src/components/hmr-target.vue");
        std::fs::create_dir_all(comp.parent().unwrap()).unwrap();
        std::fs::write(&comp, "<template><p>x</p></template>").unwrap();
        let page = root.join("src/routes/index/page.vue");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, "<template><main /></template>").unwrap();
        page
    }

    #[test]
    fn parent_relative_specifiers_resolve_to_normalized_paths() {
        let tmp = std::env::temp_dir().join("oxide-bug-mapkey-resolve-relative");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let page = write_tree(&tmp);

        let got =
            resolve_relative(&page, "../../components/hmr-target.vue").expect("import resolves");
        assert_eq!(
            got,
            tmp.join("src/components/hmr-target.vue"),
            "the map key must NOT carry `..` segments — the bundler resolves the same import to the normalized spelling"
        );
        assert!(
            !got.to_string_lossy().contains(".."),
            "key must be lexically normalized, got {}",
            got.display()
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn same_dir_specifiers_stay_normalized() {
        let tmp = std::env::temp_dir().join("oxide-bug-mapkey-resolve-relative-2");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src/routes")).unwrap();
        let page = tmp.join("src/routes/page.vue");
        std::fs::write(&page, "<template />").unwrap();
        std::fs::write(tmp.join("src/routes/partial.vue"), "<template />").unwrap();
        let got = resolve_relative(&page, "./partial.vue").expect("import resolves");
        assert_eq!(got, tmp.join("src/routes/partial.vue"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn index_file_resolution_is_normalized_too() {
        let tmp = std::env::temp_dir().join("oxide-bug-mapkey-resolve-relative-3");
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("src/components/widget");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.vue"), "<template />").unwrap();
        let page = tmp.join("src/routes/other/page.vue");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, "<template />").unwrap();
        let got = resolve_relative(&page, "../../components/widget").expect("import resolves");
        assert_eq!(got, tmp.join("src/components/widget/index.vue"));
        assert!(!got.to_string_lossy().contains(".."));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn non_relative_specifiers_return_none() {
        let tmp = std::env::temp_dir().join("oxide-bug-mapkey-resolve-relative-4");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let page = write_tree(&tmp);
        assert!(resolve_relative(&page, "vue").is_none());
        assert!(resolve_relative(&page, "@oxide-fw/core").is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[cfg(test)]
mod discovery_tests {
    //! §V2-B07 phase 1: the discovery walk's own pins. The walk is pure
    //! (no pool), so these run without a worker; the parallel-compile
    //! semantics they feed live in the V8-free engine
    //! (`crates/compiler/tests/precompile_engine_tdd.rs`).

    use super::*;

    /// An entry page importing two components, one of which imports a
    /// third — the transitive chain discovery must follow.
    fn write_chain(root: &std::path::Path) -> std::path::PathBuf {
        std::fs::create_dir_all(root.join("src/routes")).unwrap();
        std::fs::create_dir_all(root.join("src/components")).unwrap();
        std::fs::write(
            root.join("src/components/inner.vue"),
            "<template><i/></template>",
        )
        .unwrap();
        std::fs::write(
            root.join("src/components/outer.svelte"),
            "<script>import './inner.vue';</script><p>o</p>",
        )
        .unwrap();
        let page = root.join("src/routes/index/page.vue");
        // `fs::write` does not create parents — the `index` route dir must
        // exist first (the pre-existing pins wrote straight into it).
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(
            &page,
            "<script>import '../../components/outer.svelte';</script><template><p/></template>",
        )
        .unwrap();
        page
    }

    #[test]
    fn discovery_follows_sfc_import_chains() {
        let tmp = tempfile::tempdir().unwrap();
        let page = write_chain(tmp.path());
        let mut entries = BTreeMap::new();
        entries.insert("routes/index".to_string(), page);

        let jobs = discover_jobs(&entries, None);
        let paths: Vec<&std::path::Path> = jobs.iter().map(|j| j.path.as_path()).collect();
        assert!(paths.contains(&tmp.path().join("src/routes/index/page.vue").as_path()));
        assert!(paths.contains(&tmp.path().join("src/components/outer.svelte").as_path()));
        assert!(paths.contains(&tmp.path().join("src/components/inner.vue").as_path()));
    }

    #[test]
    fn discovery_marks_the_compilable_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/routes")).unwrap();
        std::fs::create_dir_all(root.join("src/components")).unwrap();
        // `fs::write` does not create parents — the `index` route dir
        // must exist before the page lands in it.
        std::fs::create_dir_all(root.join("src/routes/index")).unwrap();
        // The components are only discovered when the entry REACHES them (the
        // walk is import-driven), so the page imports both fixtures.
        std::fs::write(
            root.join("src/routes/index/page.vue"),
            "<script>import '../../components/a.svelte';import '../../components/b.mdx';</script><template />",
        )
        .unwrap();
        std::fs::write(root.join("src/components/a.svelte"), "<p/>").unwrap();
        std::fs::write(root.join("src/components/b.mdx"), "# x").unwrap();
        std::fs::write(root.join("src/components/plain.ts"), "export {}").unwrap();
        let mut entries = BTreeMap::new();
        entries.insert(
            "routes/index".to_string(),
            root.join("src/routes/index/page.vue"),
        );

        let jobs = discover_jobs(&entries, None);
        let by_name: std::collections::BTreeMap<String, &str> = jobs
            .iter()
            .map(|j| {
                (
                    j.path.file_name().unwrap().to_string_lossy().into_owned(),
                    j.kind,
                )
            })
            .collect();
        assert_eq!(by_name.get("page.vue"), Some(&"vue"));
        assert_eq!(by_name.get("a.svelte"), Some(&"svelte"));
        assert_eq!(by_name.get("b.mdx"), Some(&"mdx"));
        assert!(
            !by_name.contains_key("plain.ts"),
            "plain TS is the bundler's Oxc lowering, not a precompile kind"
        );
    }

    #[test]
    fn discovery_routes_solid_tsx_only_when_the_project_declares_solid() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/routes")).unwrap();
        let tsx = root.join("src/routes/island.tsx");
        std::fs::write(&tsx, "export default () => null;").unwrap();
        let mut entries = BTreeMap::new();
        entries.insert("routes/island".to_string(), tsx.clone());

        assert!(discover_jobs(&entries, Some("react")).is_empty());
        let solid = discover_jobs(&entries, Some("solid"));
        assert_eq!(solid.len(), 1);
        assert_eq!(solid[0].kind, "solid");
    }

    #[test]
    fn discovery_is_deterministic_and_dedupes() {
        let tmp = tempfile::tempdir().unwrap();
        let page = write_chain(tmp.path());
        // Both entries reach the same component set.
        let mut entries = BTreeMap::new();
        entries.insert("routes/index".to_string(), page.clone());
        entries.insert("routes/index-again".to_string(), page);

        let first = discover_jobs(&entries, None);
        let second = discover_jobs(&entries, None);
        assert_eq!(first, second, "two runs, identical jobs");
        let mut paths: Vec<&std::path::Path> = first.iter().map(|j| j.path.as_path()).collect();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), first.len(), "no duplicate jobs");
    }
}

#[cfg(test)]
mod native_pick_tests {
    //! §V2-C01 tail pins: the CLI's `compile_one` consults the registry
    //! ahead of the pool branch, folds the D40 tool version into the CAS
    //! key (`native:{id}@{version}` — a backend upgrade or a
    //! `compilerBackend` flip re-compiles instead of reusing the pool's
    //! artifact), and the `native` hard request refuses end-to-end with
    //! `CMP-NAT-001` instead of silently degrading to the pool.

    use super::*;
    use bytes::Bytes;
    use oxide_fw_compiler::native::{CompileOutput, NativeCompiler};
    use oxide_fw_jshost::{CallStream, PoolConfig, ScriptedPool};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A healthy fake Vue backend: the emitted JS carries the request so
    /// the plumbing pins can tell a native compile from a pool compile,
    /// and every real compile bumps the shared counter (the cache-hit
    /// pins read it: a cache hit must NOT re-compile).
    struct FakeVueNative {
        id: &'static str,
        version: &'static str,
        compile_calls: Arc<AtomicUsize>,
    }

    impl NativeCompiler for FakeVueNative {
        fn framework(&self) -> &'static str {
            "vue"
        }
        fn id(&self) -> &'static str {
            self.id
        }
        fn version(&self) -> String {
            self.version.to_string()
        }
        fn handles(&self, ext: &str) -> bool {
            ext == "vue"
        }
        fn compile(&self, req: &CompileRequest<'_>) -> oxide_fw_compiler::Result<CompileOutput> {
            self.compile_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CompileOutput {
                js: format!("native js for {}", req.source),
                css: None,
                warnings: Vec::new(),
                source_map: None,
            })
        }
        fn health_fixture(&self) -> (&'static str, &'static str) {
            ("fixture.vue", "<template />")
        }
    }

    /// A scripted Node pool: every call answers `{"js": "pool js"}` and
    /// bumps the shared counter, so a pin can prove the pool was (or was
    /// not) consulted.
    fn scripted_pool(calls: Arc<AtomicUsize>) -> Arc<dyn Pool> {
        Arc::new(ScriptedPool::new(
            Arc::new(move |_entry, _req| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(CallStream::Complete(Bytes::from(
                    serde_json::json!({ "js": "pool js" }).to_string(),
                )))
            }),
            PoolConfig::default(),
        ))
    }

    /// One `.vue` entry under a temp root (the discovery walk reads it).
    fn vue_entries(root: &Path) -> BTreeMap<String, PathBuf> {
        let page = root.join("src/routes/index/page.vue");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, "<template><p>hi</p></template>").unwrap();
        let mut entries = BTreeMap::new();
        entries.insert("routes/index".to_string(), page);
        entries
    }

    #[tokio::test]
    async fn native_pick_beats_the_pool_and_the_tool_version_folds_into_the_key() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = vue_entries(tmp.path());
        let pool_calls = Arc::new(AtomicUsize::new(0));
        let pool = scripted_pool(Arc::clone(&pool_calls));

        let native_calls = Arc::new(AtomicUsize::new(0));
        let mut reg = NativeRegistry::empty();
        reg.register(Box::new(FakeVueNative {
            id: "vue-native",
            version: "0.1.2",
            compile_calls: Arc::clone(&native_calls),
        }));
        let reg = Arc::new(reg);

        // First run: the native backend compiles; the pool is never called.
        let out = precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: None,
            tool_version: "0.0.0-test",
            native: Some(Arc::clone(&reg)),
            backend: Backend::Auto,
            data_ox_src: None,
        })
        .await
        .expect("native pick compiles");
        assert_eq!(pool_calls.load(Ordering::SeqCst), 0, "pool not consulted");
        // The fake counts the once-only health probe (the registry's
        // `pick` compiles the fixture first) plus the real compile: 2.
        assert_eq!(native_calls.load(Ordering::SeqCst), 2);
        let js = out.values().next().expect("one compiled row");
        assert!(js.contains("native js"), "the native output landed");

        // Second run over the same warm CAS: the folded
        // `native:vue-native@0.1.2` key hits; neither the backend nor the
        // pool re-compiles.
        let cache_dir = tempfile::tempdir().unwrap();
        let cache = PrecompileCache::open_soft(cache_dir.path());
        let out2 = precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: cache.as_ref(),
            tool_version: "0.0.0-test",
            native: Some(Arc::clone(&reg)),
            backend: Backend::Auto,
            data_ox_src: None,
        })
        .await
        .expect("second run compiles (cold cache)");
        // Cold cache: the real compile again, but the health verdict is
        // cached on the same registry instance — no new probe.
        assert_eq!(native_calls.load(Ordering::SeqCst), 3);
        assert_eq!(pool_calls.load(Ordering::SeqCst), 0);
        let js2 = out2.values().next().unwrap().clone();
        let out3 = precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: cache.as_ref(),
            tool_version: "0.0.0-test",
            native: Some(Arc::clone(&reg)),
            backend: Backend::Auto,
            data_ox_src: None,
        })
        .await
        .expect("third run");
        assert_eq!(
            native_calls.load(Ordering::SeqCst),
            3,
            "the warm CAS hit — no re-compile"
        );
        assert_eq!(out3.values().next(), Some(&js2), "cached output reused");

        // A backend VERSION bump changes the D40 tool version, so the key
        // misses and the (new-version) backend re-compiles — a stale
        // artifact from the old backend is never reused. The upgraded
        // registry probes its own fixture first (1) then compiles (2).
        let upgraded_calls = Arc::new(AtomicUsize::new(0));
        let mut upgraded = NativeRegistry::empty();
        upgraded.register(Box::new(FakeVueNative {
            id: "vue-native",
            version: "0.1.3",
            compile_calls: Arc::clone(&upgraded_calls),
        }));
        precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: cache.as_ref(),
            tool_version: "0.0.0-test",
            native: Some(Arc::new(upgraded)),
            backend: Backend::Auto,
            data_ox_src: None,
        })
        .await
        .expect("upgraded backend compiles");
        assert_eq!(
            upgraded_calls.load(Ordering::SeqCst),
            2,
            "version bump ⇒ key miss ⇒ re-compile (plus the fresh instance's health probe)"
        );
        assert_eq!(pool_calls.load(Ordering::SeqCst), 0, "never the pool");
    }

    #[tokio::test]
    async fn cmp_nat_001_refuses_end_to_end_under_native() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = vue_entries(tmp.path());
        let pool_calls = Arc::new(AtomicUsize::new(0));
        let pool = scripted_pool(Arc::clone(&pool_calls));
        // The default build's registry: nothing compiled in.
        let reg = Arc::new(NativeRegistry::from_enabled_features());
        let e = precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: None,
            tool_version: "0.0.0-test",
            native: Some(Arc::clone(&reg)),
            backend: Backend::Native,
            data_ox_src: None,
        })
        .await
        .expect_err("`native` with no compiled-in backend fails the pass");
        assert!(
            e.to_string().contains("CMP-NAT-001"),
            "the registry refusal surfaces end-to-end, got: {e}"
        );
        assert_eq!(
            pool_calls.load(Ordering::SeqCst),
            0,
            "no silent pool fallback behind the author's hard request"
        );
    }

    #[tokio::test]
    async fn auto_without_a_backend_still_uses_the_pool() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = vue_entries(tmp.path());
        let pool_calls = Arc::new(AtomicUsize::new(0));
        let pool = scripted_pool(Arc::clone(&pool_calls));
        let reg = Arc::new(NativeRegistry::from_enabled_features());
        let out = precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: None,
            tool_version: "0.0.0-test",
            native: Some(reg),
            backend: Backend::Auto,
            data_ox_src: None,
        })
        .await
        .expect("the pool compiles");
        assert_eq!(out.values().next(), Some(&"pool js".to_string()));
        assert_eq!(pool_calls.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod data_src_stamp_tests {
    //! §V2-F17 pins: the DOM→source stamp (`data-ox-src="src/<rel>:1"`, the
    //! cost endpoint's `?src=` spelling) rides the DEV precompile payloads —
    //! the pool compiles the STAMPED source so the rendered DOM carries the
    //! attribute — and the production pass (the flag off) stays
    //! byte-identical. The engine (`oxide_fw_compiler::inject_data_ox_src`)
    //! is the one definition site; these pins hold the WIRE.

    use super::*;
    use bytes::Bytes;
    use oxide_fw_compiler::native::{CompileOutput, NativeCompiler};
    use oxide_fw_jshost::{CallStream, PoolConfig, ScriptedPool};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A scripted pool that records every payload it is handed, so a pin
    /// can assert the SOURCE the compile path actually sent.
    fn capturing_pool(seen: Arc<Mutex<Vec<serde_json::Value>>>) -> Arc<dyn Pool> {
        Arc::new(ScriptedPool::new(
            Arc::new(move |_entry, req| {
                seen.lock().unwrap().push(req.payload.clone());
                Ok(CallStream::Complete(Bytes::from(
                    serde_json::json!({ "js": "pool js" }).to_string(),
                )))
            }),
            PoolConfig::default(),
        ))
    }

    /// One `.vue` page under `<root>/src/routes/index/` — the discovery
    /// walk reads the source, the compile path stamps it.
    fn vue_entries(root: &Path) -> BTreeMap<String, PathBuf> {
        let page = root.join("src/routes/index/page.vue");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, "<template><p>hi</p></template>").unwrap();
        let mut entries = BTreeMap::new();
        entries.insert("routes/index".to_string(), page);
        entries
    }

    /// A native vue backend that echoes the request source into its output,
    /// so a pin can assert the source the NATIVE arm received.
    struct EchoVueNative {
        calls: Arc<AtomicUsize>,
    }

    impl NativeCompiler for EchoVueNative {
        fn framework(&self) -> &'static str {
            "vue"
        }
        fn id(&self) -> &'static str {
            "vue-echo"
        }
        fn version(&self) -> String {
            "0.0.0-test".to_string()
        }
        fn handles(&self, ext: &str) -> bool {
            ext == "vue"
        }
        fn compile(&self, req: &CompileRequest<'_>) -> oxide_fw_compiler::Result<CompileOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(CompileOutput {
                js: format!("native js for {}", req.source),
                css: None,
                warnings: Vec::new(),
                source_map: None,
            })
        }
        fn health_fixture(&self) -> (&'static str, &'static str) {
            ("fixture.vue", "<template />")
        }
    }

    #[tokio::test]
    async fn dev_stamp_rides_the_pool_payload_for_vue() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = vue_entries(tmp.path());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let pool = capturing_pool(Arc::clone(&seen));
        let src_root = tmp.path().join("src");
        precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: None,
            tool_version: "0.0.0-test",
            native: None,
            backend: Backend::Auto,
            data_ox_src: Some(&src_root),
        })
        .await
        .expect("the dev pass compiles");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "one pool call");
        let source = seen[0]
            .get("source")
            .and_then(|v| v.as_str())
            .expect("payload source");
        assert!(
            source.contains(r#"data-ox-src="src/routes/index/page.vue:1""#),
            "the pool must receive the STAMPED source (the ?src= spelling), got: {source}"
        );
    }

    #[tokio::test]
    async fn stamp_off_keeps_the_pool_payload_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = vue_entries(tmp.path());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let pool = capturing_pool(Arc::clone(&seen));
        precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: None,
            tool_version: "0.0.0-test",
            native: None,
            backend: Backend::Auto,
            data_ox_src: None,
        })
        .await
        .expect("the production pass compiles");
        let seen = seen.lock().unwrap();
        let source = seen[0]
            .get("source")
            .and_then(|v| v.as_str())
            .expect("payload source");
        assert_eq!(
            source, "<template><p>hi</p></template>",
            "the production shape is byte-identical — no stamp in the payload"
        );
    }

    #[tokio::test]
    async fn dev_stamp_covers_svelte_and_solid_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let svelte = tmp.path().join("src/routes/index/page.svelte");
        std::fs::create_dir_all(svelte.parent().unwrap()).unwrap();
        std::fs::write(
            &svelte,
            "<script>\nlet n = 0;\n</script>\n<main><span>n</span></main>\n",
        )
        .unwrap();
        let tsx = tmp.path().join("src/routes/island.tsx");
        std::fs::write(
            &tsx,
            "export default function Island() {\n  return <main>hi</main>;\n}\n",
        )
        .unwrap();
        let mut entries = BTreeMap::new();
        entries.insert("routes/index".to_string(), svelte);
        entries.insert("routes/island".to_string(), tsx);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let pool = capturing_pool(Arc::clone(&seen));
        let src_root = tmp.path().join("src");
        precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("solid"),
            cache: None,
            tool_version: "0.0.0-test",
            native: None,
            backend: Backend::Auto,
            data_ox_src: Some(&src_root),
        })
        .await
        .expect("the dev pass compiles");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "svelte + solid payloads");
        let sources: Vec<String> = seen
            .iter()
            .filter_map(|p| p.get("source").and_then(|v| v.as_str()))
            .map(str::to_string)
            .collect();
        assert!(
            sources
                .iter()
                .any(|s| s.contains(r#"data-ox-src="src/routes/index/page.svelte:1""#)),
            "the svelte markup region's first tag carries the stamp, got: {sources:?}"
        );
        assert!(
            sources
                .iter()
                .any(|s| s.contains(r#"data-ox-src="src/routes/island.tsx:1""#)),
            "the solid JSX first element carries the stamp, got: {sources:?}"
        );
    }

    #[tokio::test]
    async fn dev_stamp_rides_the_native_arm_source() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = vue_entries(tmp.path());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut reg = NativeRegistry::empty();
        reg.register(Box::new(EchoVueNative {
            calls: Arc::clone(&calls),
        }));
        let reg = Arc::new(reg);
        let src_root = tmp.path().join("src");
        let out = precompile_entries(PrecompilePlan {
            pool: Some(&pool_never()),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: None,
            tool_version: "0.0.0-test",
            native: Some(Arc::clone(&reg)),
            backend: Backend::Auto,
            data_ox_src: Some(&src_root),
        })
        .await
        .expect("the native arm compiles");
        let js = out.values().next().expect("one compiled row");
        assert!(
            js.contains(r#"data-ox-src="src/routes/index/page.vue:1""#),
            "the native backend received the STAMPED source (its output echoes it), got: {js}"
        );
        // The flag off: the same native arm receives the raw bytes.
        let out = precompile_entries(PrecompilePlan {
            pool: Some(&pool_never()),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: None,
            tool_version: "0.0.0-test",
            native: Some(reg),
            backend: Backend::Auto,
            data_ox_src: None,
        })
        .await
        .expect("the native arm compiles");
        let js = out.values().next().expect("one compiled row");
        assert!(
            !js.contains("data-ox-src"),
            "the production native arm stays unstamped, got: {js}"
        );
    }

    /// A pool the pin proves is never consulted (the native pick wins).
    fn pool_never() -> Arc<dyn Pool> {
        Arc::new(ScriptedPool::without_handler(PoolConfig::default()))
    }

    #[tokio::test]
    async fn stamped_source_keys_separately_in_the_cas() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = vue_entries(tmp.path());
        let pool_calls = Arc::new(AtomicUsize::new(0));
        let pool = scripted_pool(Arc::clone(&pool_calls));
        let cache_dir = tempfile::tempdir().unwrap();
        let cache = PrecompileCache::open_soft(cache_dir.path());
        let src_root = tmp.path().join("src");
        // Warm the CAS under the DEV shape (stamped source).
        precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: cache.as_ref(),
            tool_version: "0.0.0-test",
            native: None,
            backend: Backend::Auto,
            data_ox_src: Some(&src_root),
        })
        .await
        .expect("the dev pass compiles");
        assert_eq!(pool_calls.load(Ordering::SeqCst), 1);
        // Same run again: the stamped key hits, the pool is not consulted.
        precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: cache.as_ref(),
            tool_version: "0.0.0-test",
            native: None,
            backend: Backend::Auto,
            data_ox_src: Some(&src_root),
        })
        .await
        .expect("the warm dev pass hits the CAS");
        assert_eq!(
            pool_calls.load(Ordering::SeqCst),
            1,
            "CAS hit, no recompile"
        );
        // The production shape (unstamped) does NOT reuse the stamped entry.
        precompile_entries(PrecompilePlan {
            pool: Some(&pool),
            entries: &entries,
            svelte_generate: "dom",
            framework: Some("vue"),
            cache: cache.as_ref(),
            tool_version: "0.0.0-test",
            native: None,
            backend: Backend::Auto,
            data_ox_src: None,
        })
        .await
        .expect("the production pass compiles");
        assert_eq!(
            pool_calls.load(Ordering::SeqCst),
            2,
            "a stamped entry never serves the unstamped shape"
        );
    }

    /// The repo's own scripted pool (the `native_pick_tests` shape): every
    /// call answers `{"js": "pool js"}` and bumps the shared counter.
    fn scripted_pool(calls: Arc<AtomicUsize>) -> Arc<dyn Pool> {
        Arc::new(ScriptedPool::new(
            Arc::new(move |_entry, _req| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(CallStream::Complete(Bytes::from(
                    serde_json::json!({ "js": "pool js" }).to_string(),
                )))
            }),
            PoolConfig::default(),
        ))
    }
}
