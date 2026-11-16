//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// §4.2: for the V8 backend, bundle `oxide.config.ts` (and the framework
/// adapter it imports) into a single self-contained ESM file. Bare V8 ESM has
/// no CommonJS interop — `react` is CJS and `import('react')` fails with
/// "does not provide an export named 'createElement'". Rolldown resolves CJS,
/// so bundling the config graph once at build gives V8 a file it can import.
///
/// Written to `<root>/.oxide/config.server.js`; `build_framework_stack` points
/// the V8 renderer at it. A no-op for the Node backend, which imports the
/// config directly.
pub(crate) fn emit_config_bundle(
    _cfg: &oxide_fw_config::OxideConfig,
    root: &Path,
    precompiled: BTreeMap<PathBuf, String>,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    // Always bundle the config — do NOT gate on `cfg.server.backend`. The
    // static extractor's view of a nested `server.backend` is not reliable
    // enough to gate a file another component needs: when extraction returned
    // `Node` for a config that says `v8`, this returned early, no
    // `config.server.js` was written, and the V8 prerender then failed to load
    // it. Bundling is cheap (~30 ms) and only the V8 backend reads the result.
    let Some(config_file) = find_config_file(root).ok() else {
        return Ok(());
    };
    // Bundle into a DEDICATED directory. Rolldown's `write()` may empty its
    // output dir, and `.oxide/` holds the island boot entry + the snapshot the
    // prerender pass reads — a shared dir raced those writes and the config
    // bundle vanished mid-build. A private dir is never touched by another
    // writer.
    let bundle_dir = root.join(".oxide/config-build");
    if bundle_dir.exists() {
        let _ = std::fs::remove_dir_all(&bundle_dir);
    }
    std::fs::create_dir_all(&bundle_dir)
        .map_err(|e| anyhow::anyhow!("create {}: {e}", bundle_dir.display()))?;

    let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    entries.insert("config".to_string(), config_file.clone());
    let opts = oxide_fw_bundler::BundleOptions {
        // §4.6: the V8 sandbox is browser-like (Web globals, no Node built-ins).
        // Resolve with BROWSER conditions so `react-dom/server` picks
        // `server.browser.js` — the `server.node.js` build `require('stream')`
        // and fails with "require() is not available in the V8 runtime".
        env: oxide_fw_bundler::BundleEnv::Client,
        entries,
        out_dir: bundle_dir.clone(),
        minify: false,
        sourcemap: false,
        // The config bundle and the route chunks must bind the SAME module
        // instances. `Vec::new()` inlined react + react-dom here — a
        // pre-`e122122` workaround from the days the V8 loader had no
        // CJS interop — which produced two React copies under V8: the
        // renderer's hooks dispatcher was set on the inlined copy while
        // the route chunks' `useState` read the external one
        // ("Cannot read properties of null (reading 'useState')").
        // The loader now externalizes CJS properly (§21), so externalize
        // the framework runtimes here too; both resolve to the same
        // canonical module URL at runtime.
        externals: default_ssr_externals(),
        define: BTreeMap::new(),
        tsconfig_paths: BTreeMap::new(),
        build_secret: "config-bundle".to_string(),
        // The config imports the SFCs directly (`import Home from './page.vue'`),
        // so the precompiled JS must ride along or rolldown parses the raw SFC.
        precompiled,
        jsx_import_source: None,
        transform_hook: None,
        resolve_hook: None,
        load_hook: None,
        render_chunk_hook: None,
        generate_bundle_hook: None,
        transform_html_hook: None,
        css_transform: None,
        // §4.2: one self-contained file. A code-split bundle needs its sibling
        // chunks copied alongside and its `__toESM` dynamic-import wrappers
        // resolved by the loader; a single inlined file is simpler and what the
        // bare V8 host wants.
        vendor_groups: Vec::new(),
        inline_dynamic_imports: true,
        // BUG-SOLID: resolve conditions must match the V8 loader
        // (`v8_loader.rs`). `worker` FIRST gives `@solidjs/web` its SSR build
        // (`dist/server.js`); the `browser`-only default gave it the CLIENT
        // build, whose `ssr()` returns undefined → a shell with an empty body.
        // `worker` still yields a browser-compatible `react-dom/server`.
        resolve_conditions: Some(vec![
            "worker".into(),
            "browser".into(),
            "import".into(),
            "module".into(),
            "default".into(),
        ]),
        virtual_modules: Default::default(),
    };
    use oxide_fw_bundler::Bundler as _;
    let backend = oxide_fw_bundler::RolldownBundler::new(root);
    match backend.bundle(&opts) {
        Ok(out) => {
            // The entry chunk is written as `<name>-<hash>.js`; rename it to
            // the stable name the loader expects.
            // Find the entry chunk's written file and copy it to the stable
            // name the V8 renderer imports. `copy` (not `rename`) so a
            // re-run finds the source again; a missing source is a warning,
            // not a build failure — the Node backend never needs this file.
            let entry_file = out
                .chunks
                .iter()
                .find(|c| c.kind == oxide_fw_bundler::ChunkKind::Entry)
                .map(|c| c.file.clone());
            match entry_file {
                Some(file) => {
                    // The bundle is a small graph: the entry chunk imports
                    // sibling chunks (`react-*.js`, …). Copy ALL of them into
                    // `.oxide/` so the entry's relative imports resolve, then
                    // rename the entry to the stable name the V8 loader opens.
                    let mut copied = 0usize;
                    if let Ok(rd) = std::fs::read_dir(&bundle_dir) {
                        for e in rd.flatten() {
                            let src = e.path();
                            if src.extension().is_some_and(|x| x == "js")
                                && let Some(name) = src.file_name()
                            {
                                let dst = root.join(".oxide").join(name);
                                std::fs::copy(&src, &dst).map_err(|err| {
                                    anyhow::anyhow!(
                                        "copy {} -> {}: {err}",
                                        src.display(),
                                        dst.display()
                                    )
                                })?;
                                copied += 1;
                            }
                        }
                    }
                    let from = root.join(".oxide").join(&file);
                    let to = root.join(".oxide/config.server.js");
                    if from != to {
                        std::fs::copy(&from, &to).map_err(|e| {
                            anyhow::anyhow!("copy {} -> config.server.js: {e}", from.display())
                        })?;
                    }
                    outln!(
                        w,
                        "config bundle: .oxide/config.server.js (+{} chunks)",
                        copied.saturating_sub(1)
                    );
                }
                None => {
                    outln!(
                        w,
                        "config bundle: no entry chunk produced; V8 will load the raw config"
                    );
                }
            }
        }
        Err(e) => {
            outln!(w, "config bundle: skipped ({} {})", e.0.code, e.0.message);
        }
    }
    Ok(())
}
