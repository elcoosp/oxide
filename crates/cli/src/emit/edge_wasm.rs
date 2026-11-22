//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// `oxide build` (§6, §10.5): bundle client + ssr, prerender static routes, emit the
/// deploy artifact for the configured adapter (§18). `--static` forces a static
/// export target, which refuses dynamic routes without prerendered output (§10.5).///
/// Compile the `oxide-fw-core-wasm` crate to wasm32-unknown-unknown (§2 D18).
///
/// Best-effort: returns `Ok(None)` when the target isn't installed (the
/// Worker's JS fallback handles it), `Err` on a genuine compile failure.
/// Output goes to `<dist>/edge.wasm`, which is where the Cloudflare emit
/// branch looks for it.
pub(crate) fn compile_edge_wasm(_root: &Path, dist: &Path) -> anyhow::Result<Option<PathBuf>> {
    // WP-C2: the wasm-bindgen-processed `oxide-fw-core-wasm` artifact is embedded
    // into the CLI binary at compile time (see `EDGE_WASM_BYTES` above and
    // `scripts/regen-edge-wasm.sh`). The previous implementation ran
    // `cargo build -p oxide-fw-core-wasm` inside `root`, which only worked when
    // `root` was the oxide workspace itself. A real user project is a JS/TS
    // tree with no cargo workspace containing `oxide-fw-core-wasm`, so that path
    // failed with a non-zero exit for every real project, silently shipping
    // the JS fallback (deviation 6, docs/deviations.md). Embedding the
    // artifact at CLI build time is exactly what the deviation named as the
    // fix.
    std::fs::create_dir_all(dist)?;
    let out = dist.join("edge.wasm");
    std::fs::write(&out, EDGE_WASM_BYTES)?;
    // V2-E09 / P0-1c: ship the wasm-bindgen `--target web` glue alongside the
    // wasm so the Worker's loader can `await init(wasm)` — the payload is
    // functional (no inert JS fallback). The adapters Cloudflare emit copies
    // both into the deploy directory.
    std::fs::write(dist.join("edge.js"), EDGE_GLUE_JS)?;
    Ok(Some(out))
}

/// §18 / P0-1b: locate the project's edge entry.
///
/// Cloudflare Workers convention: a single `src/edge.{ts,mjs,js,tsx,jsx}`
/// exports `default (req, env, ctx) => Response`. The typed extension is
/// preferred so a project with a stale `.js` and a real `.ts` gets the
/// typed one. Returns `None` when no entry exists -- the emitted
/// `edge_handler.js` catches the resulting `Cannot find module` and falls
/// through to the built-in JS path.
pub(crate) fn find_edge_entry(root: &Path) -> Option<PathBuf> {
    for ext in ["ts", "mjs", "js", "tsx", "jsx"] {
        let candidate = root.join("src").join("edge").with_extension(ext);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// §18 / P0-1b: bundle the edge entry into `dist/deploy/edge_bundle.js`.
///
/// The emitted `edge_handler.js` does `import("./edge_bundle.js")` and
/// calls its default export. This helper produces that file: a
/// single-entry bundle in `BundleEnv::Edge` (production defines,
/// minified, no sourcemap) written next to the handler.
///
/// **Chunking:** rolldown may emit shared chunks alongside the entry when
/// the entry imports common code. All chunks are written to
/// `dist/deploy/` (the whole directory deploys together for Cloudflare
/// Workers); only the entry chunk is renamed to `edge_bundle.js`. Its
/// relative imports of the sibling chunks resolve at their hashed names.
///
/// **Externals:** none. The edge handler must self-contain relative
/// imports; a bare npm import either resolves to the package's edge
/// entry or fails with BLD-RESOLVE at build time. `node:*` imports are
/// not specially handled -- a project using them gets the build-time
/// resolve error, which is the honest signal (Cloudflare Workers'
/// node-compat is a runtime flag the build cannot know about).
pub(crate) async fn bundle_edge_entry(
    root: &Path,
    dist: &Path,
    entry: &Path,
    css_transform: oxide_fw_bundler::CssTransformHook,
    // §18 / D18: specifiers left external in the edge bundle. Empty by
    // default; a Worker declaring node-compat lists what it expects the
    // runtime to provide.
    edge_externals: Vec<String>,
    // §6.3 / §16.3: precompiled sources for SFCs and MDX reachable from
    // the edge entry — the same async tooling-pool pass the client and
    // SSR bundles use. Without it, the bundler refuses a
    // .vue/.svelte/.mdx module reachable from src/edge.ts (BLD-FRAMEWORK).
    precompiled: std::collections::BTreeMap<std::path::PathBuf, String>,
    w: Writer<'_>,
) -> anyhow::Result<Option<PathBuf>> {
    use oxide_fw_bundler::Bundler as _;
    let out_dir = dist.join("deploy");
    std::fs::create_dir_all(&out_dir)?;

    let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    entries.insert("edge".to_string(), entry.to_path_buf());

    let opts = bundle_env_options(BundleEnvArgs {
        env: oxide_fw_bundler::BundleEnv::Edge,
        entries,
        out_dir: out_dir.clone(),
        precompiled,
        jsx_import_source: None,
        build_secret: "edge".to_string(),
        transform_hook: None,
        css_transform,
        ssr_externals: Vec::new(),
        sourcemap_on: false,
        minify: true,
    });

    // `bundle_env_options` only populates externals for the SSR env; the edge
    // env is self-contained by default. Apply the declared set here rather
    // than widening that function's signature for one caller.
    let mut opts = opts;
    opts.externals = edge_externals;
    let bundler = oxide_fw_bundler::RolldownBundler::new(root);
    let out = bundler
        .bundle(&opts)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    let entry_chunks: Vec<_> = out
        .chunks
        .iter()
        .filter(|c| c.kind == oxide_fw_bundler::ChunkKind::Entry)
        .collect();
    if entry_chunks.is_empty() {
        return Ok(None);
    }
    let entry_chunk = entry_chunks[0];
    let produced = out_dir.join(&entry_chunk.file);
    let target = out_dir.join("edge_bundle.js");
    if target.exists() {
        std::fs::remove_file(&target)?;
    }
    std::fs::rename(&produced, &target)?;
    let bytes = std::fs::metadata(&target)?.len();
    outln!(
        w,
        "edge bundle: {} ({} bytes, env=edge)",
        target.display(),
        bytes
    );
    Ok(Some(target))
}
