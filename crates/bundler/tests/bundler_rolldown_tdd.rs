//! TDD suite for the Rolldown-backed `Bundler` implementation (D3).
//!
//! These tests exercise `RolldownBundler` directly. They verify the spec's
//! D3 contract: Rolldown used as a library, writing content-hashed ESM chunks
//! plus client/server manifests. Per-module metadata and CSS-asset wiring are
//! not asserted here (see docs/deviations.md for the remaining gap).

use oxide_bundler::{BundleEnv, BundleOptions, Bundler, ChunkKind, RolldownBundler};
use std::collections::BTreeMap;
use std::path::Path;

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn opts(root: &Path, env: BundleEnv, entries: &[(&str, &str)]) -> BundleOptions {
    BundleOptions {
        env,
        entries: entries
            .iter()
            .map(|(r, p)| (r.to_string(), root.join(p)))
            .collect(),
        out_dir: root.join("dist"),
        minify: false,
        sourcemap: false,
        externals: Vec::new(),
        define: BTreeMap::new(),
        tsconfig_paths: BTreeMap::new(),
        build_secret: "test-secret".to_string(),
        precompiled: BTreeMap::new(),
        jsx_import_source: None,
        transform_hook: None,
        resolve_hook: None,
        load_hook: None,
        render_chunk_hook: None,
        generate_bundle_hook: None,
        transform_html_hook: None,
        css_transform: None,
        inline_dynamic_imports: false,
        resolve_conditions: None,
        virtual_modules: Default::default(),
    }
}

#[test]
fn rolldown_bundles_single_entry_and_writes_chunk() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/index.js",
        "import { x } from './x.js';\nexport const out = x + 1;\nconsole.log(out);\n",
    );
    write(dir.path(), "src/x.js", "export const x = 41;\n");

    let out = RolldownBundler::new(dir.path())
        .bundle(&opts(
            dir.path(),
            BundleEnv::Client,
            &[("index", "src/index.js")],
        ))
        .expect("rolldown bundle");

    assert!(
        !out.chunks.is_empty(),
        "rolldown should produce at least one chunk"
    );
    let entry = out
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Entry)
        .expect("entry chunk present");
    let written = dir.path().join("dist").join(&entry.file);
    assert!(written.is_file(), "chunk file {} not written", entry.file);
    let code = std::fs::read_to_string(&written).unwrap();
    // Rolldown constant-folds `x + 1` where `x = 41` → `const out = 42`.
    assert!(
        code.contains("42"),
        "expected folded constant 42 in chunk body:\n{code}"
    );
    assert!(!entry.hash.is_empty());
    assert!(entry.hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn rolldown_emits_client_manifest_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/index.js", "export const a = 1;\n");
    let out = RolldownBundler::new(dir.path())
        .bundle(&opts(
            dir.path(),
            BundleEnv::Client,
            &[("home", "src/index.js")],
        ))
        .unwrap();

    assert!(out.client_manifest.is_some());
    let man = out.client_manifest.unwrap();
    assert_eq!(man.build_id.len(), 16);
    assert!(man.routes.contains_key("home"));
    let disk = dir.path().join("dist/client.manifest.json");
    assert!(disk.is_file(), "client manifest not written");
    let round: oxide_bundler::ClientManifest =
        serde_json::from_str(&std::fs::read_to_string(&disk).unwrap()).unwrap();
    assert_eq!(round.build_id, man.build_id);
}

#[test]
fn rolldown_emits_server_manifest_for_ssr_env() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/handler.js",
        "export const GET = () => ({ ok: true });\n",
    );
    let out = RolldownBundler::new(dir.path())
        .bundle(&opts(
            dir.path(),
            BundleEnv::Ssr,
            &[("api", "src/handler.js")],
        ))
        .unwrap();

    assert!(out.server_manifest.is_some());
    assert!(out.client_manifest.is_none());
    let disk = dir.path().join("dist/server.manifest.json");
    assert!(disk.is_file(), "server manifest not written");
}

#[test]
fn rolldown_minify_changes_chunk_hash() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/index.js",
        "// a comment that minify should remove\nexport const greeting = 'hello world';\nconsole.log(greeting);\n",
    );
    let mut o1 = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.js")]);
    o1.out_dir = dir.path().join("dist-a");
    let a = RolldownBundler::new(dir.path()).bundle(&o1).unwrap();
    let a_file = &a.chunks[0].file;

    let mut o2 = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.js")]);
    o2.minify = true;
    o2.out_dir = dir.path().join("dist-b");
    let b = RolldownBundler::new(dir.path()).bundle(&o2).unwrap();
    let b_file = &b.chunks[0].file;

    let a_code = std::fs::read_to_string(dir.path().join("dist-a").join(a_file)).unwrap();
    let b_code = std::fs::read_to_string(dir.path().join("dist-b").join(b_file)).unwrap();
    // Rolldown strips `//` comments during parsing regardless of minify, so the
    // comment marker is gone from both outputs. Minify removes whitespace and
    // newlines, which is the observable difference.
    let a_newlines = a_code.matches('\n').count();
    let b_newlines = b_code.matches('\n').count();
    assert!(
        b_code.len() <= a_code.len(),
        "minified should not be larger: {} vs {}",
        b_code.len(),
        a_code.len()
    );
    assert!(
        b_newlines <= a_newlines,
        "minified should have ≤ newlines: {b_newlines} vs {a_newlines}"
    );
    // The two builds differ observably: either byte length or newline count.
    assert!(
        b_code.len() < a_code.len() || b_newlines < a_newlines,
        "minified output should be observably different from unminified:\n\
         unminified ({} bytes, {} newlines):\n{}\n\
         minified ({} bytes, {} newlines):\n{}",
        a_code.len(),
        a_newlines,
        a_code,
        b_code.len(),
        b_newlines,
        b_code
    );
    // Chunk content hashes differ because the underlying bytes differ.
    assert_ne!(
        a.chunks[0].hash, b.chunks[0].hash,
        "minified and unminified chunks must have different content hashes"
    );
}

#[test]
fn rolldown_deterministic_across_runs() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/index.js",
        "export const out = 1;\nconsole.log(out);\n",
    );
    let mut o1 = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.js")]);
    o1.out_dir = dir.path().join("dist-1");
    let mut o2 = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.js")]);
    o2.out_dir = dir.path().join("dist-2");
    let a = RolldownBundler::new(dir.path()).bundle(&o1).unwrap();
    let b = RolldownBundler::new(dir.path()).bundle(&o2).unwrap();
    let af: Vec<&str> = a.chunks.iter().map(|c| c.file.as_str()).collect();
    let bf: Vec<&str> = b.chunks.iter().map(|c| c.file.as_str()).collect();
    assert_eq!(af, bf, "chunk file names must be deterministic");
    assert_eq!(
        a.client_manifest.unwrap().build_id,
        b.client_manifest.unwrap().build_id
    );
}

// ---------------- externals + define + tsconfig_paths (B.1) ----------------

#[test]
fn rolldown_honours_externals() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/index.js",
        "import { x } from 'lodash';\nexport const out = x + 1;\nconsole.log(out);\n",
    );

    let mut o = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.js")]);
    o.externals = vec!["lodash".to_string()];
    let out = RolldownBundler::new(dir.path()).bundle(&o).unwrap();

    // The external specifier must survive as an import in the output.
    let entry = out
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Entry)
        .unwrap();
    let code = std::fs::read_to_string(dir.path().join("dist").join(&entry.file)).unwrap();
    assert!(
        code.contains("lodash"),
        "external import must be preserved:\n{code}"
    );
    // And the module must not be bundled (no separate node_modules chunk).
    assert!(
        !out.chunks.iter().any(|c| c.name.contains("lodash")),
        "external must not be bundled: {:?}",
        out.chunks.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
}

#[test]
fn rolldown_applies_define_substitutions() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/index.js",
        "export const env = process.env.NODE_ENV;\nconsole.log(env);\n",
    );

    let mut o = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.js")]);
    o.define.insert(
        "process.env.NODE_ENV".to_string(),
        "\"production\"".to_string(),
    );
    let out = RolldownBundler::new(dir.path()).bundle(&o).unwrap();

    let entry = out
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Entry)
        .unwrap();
    let code = std::fs::read_to_string(dir.path().join("dist").join(&entry.file)).unwrap();
    assert!(
        code.contains("\"production\"") || code.contains("'production'"),
        "define substitution must appear in output:\n{code}"
    );
    assert!(
        !code.contains("process.env.NODE_ENV"),
        "original expression must be replaced:\n{code}"
    );
}

#[test]
fn rolldown_reads_tsconfig_paths_aliases() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/util.ts", "export const util = 42;\n");
    write(
        dir.path(),
        "src/index.ts",
        "import { util } from '@app/util';\nexport const out = util;\nconsole.log(out);\n",
    );

    let mut o = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.ts")]);
    let mut paths = std::collections::BTreeMap::new();
    paths.insert("@app/*".to_string(), vec!["src/*".to_string()]);
    o.tsconfig_paths = paths;

    // A real tsconfig on disk is required for `TsConfig::Manual`; the bundler
    // synthesizes one under `out_dir` when the pipeline only provided paths.
    // Rolldown reads it and resolves `@app/util` → `src/util.ts`.
    let out = RolldownBundler::new(dir.path()).bundle(&o).unwrap();
    let entry = out
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Entry)
        .unwrap();
    let code = std::fs::read_to_string(dir.path().join("dist").join(&entry.file)).unwrap();
    assert!(
        code.contains("42"),
        "aliased util must be resolved and inlined:\n{code}"
    );
}

#[test]
fn rolldown_emits_real_sourcemaps_next_to_chunks() {
    // §6.4 / D3: sourcemaps are Rolldown's job, not the in-house bundler's.
    // `SourceMapType::File` (set by RolldownBundler when `options.sourcemap`
    // is true) writes `<chunk>.js.map` next to each chunk and appends the
    // `//# sourceMappingURL=` comment to the chunk body. Assert both.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/index.js",
        "function greet(name) {\n  return `hello ${name}`;\n}\nconsole.log(greet('world'));\n",
    );

    let mut o = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.js")]);
    o.sourcemap = true;

    let out = RolldownBundler::new(dir.path())
        .bundle(&o)
        .expect("rolldown bundle with sourcemap");

    let entry = out
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Entry)
        .expect("entry chunk present");

    // 1. The .map file exists on disk.
    let map_path = dir.path().join("dist").join(format!("{}.map", entry.file));
    assert!(
        map_path.is_file(),
        "expected sourcemap at {}, files: {:?}",
        map_path.display(),
        std::fs::read_dir(dir.path().join("dist")).map(|rd| rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>())
    );

    // 2. The .map body is valid V3 JSON with the required fields.
    let raw = std::fs::read_to_string(&map_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON sourcemap");
    assert_eq!(v["version"], 3, "sourcemap version must be 3");
    assert!(v["mappings"].is_string(), "mappings present");
    let sources = v["sources"].as_array().expect("sources array");
    assert!(
        sources
            .iter()
            .any(|s| s.as_str().is_some_and(|s| s.contains("index.js"))),
        "expected original file in sources, got {sources:?}"
    );

    // 3. The chunk body references the map.
    let js = std::fs::read_to_string(dir.path().join("dist").join(&entry.file)).unwrap();
    assert!(
        js.contains("sourceMappingURL="),
        "chunk should carry sourceMappingURL comment, body:\n{js}"
    );
}

#[test]
fn rolldown_precompiled_plugin_serves_precompiled_js() {
    // §6.3: `.vue`/`.svelte` files are lowered to JS by the tooling pool
    // and handed to the bundler via `BundleOptions::precompiled`. The
    // PrecompiledPlugin intercepts Rolldown's `load` hook and serves the
    // JS in place of the raw SFC, so RolldownBundler can handle SFC
    // projects directly (previously the CLI had to fall back to
    // ChunkBundler for them, losing sourcemaps).
    let dir = tempfile::tempdir().unwrap();

    // The "SFC" on disk is NOT valid JS — if the plugin fails to intercept,
    // Rolldown's parse will fail and the test will see the error.
    write(
        dir.path(),
        "src/page.vue",
        "<template><div>hello</div></template>\n",
    );

    let vue_path = dir.path().join("src/page.vue").canonicalize().unwrap();
    let precompiled_js = "export default { name: 'Page' };\nconsole.log('page loaded');\n";
    let mut o = opts(dir.path(), BundleEnv::Ssr, &[("page", "src/page.vue")]);
    o.precompiled.insert(vue_path, precompiled_js.to_string());

    let out = RolldownBundler::new(dir.path())
        .bundle(&o)
        .expect("rolldown should bundle the precompiled SFC");

    // The precompiled body survived; the raw `<template>` did not.
    let entry = out
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Entry)
        .expect("entry chunk");
    let js = std::fs::read_to_string(dir.path().join("dist").join(&entry.file)).unwrap();
    assert!(
        js.contains("page loaded") || js.contains("Page"),
        "precompiled body must be present in chunk:\n{js}"
    );
    assert!(
        !js.contains("<template>"),
        "raw SFC must not appear in chunk:\n{js}"
    );
}

// ---------------------------------------------------------------------------
// §15.1: a plugin-supplied `transform` hook runs on each module before the
// compiler passes.
//
// The hook is a closure the CLI builds from a loaded WASM component; the
// bundler only knows the closure. This test uses a fake hook (no wasmtime,
// fast) to prove the exact contract: it is called with `(id, source)`, its
// return value replaces the source, and the replacement reaches the chunk.
// ---------------------------------------------------------------------------

use oxide_bundler::TransformHook;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn transform_hook_runs_before_the_compiler_passes() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/index.js",
        "export const marker = 'ORIGINAL';\nconsole.log(marker);\n",
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_hook = Arc::clone(&calls);
    // The hook must be *defensive*: Rolldown calls it for whatever module
    // is in flight, and a panic inside a plugin hook on a current-thread
    // runtime deadlocks the executor (there is no other thread to unwind
    // to). Recording the id and rewriting only when the marker is present
    // is the contract a real plugin follows — return the input unchanged
    // for a module the plugin does not recognise.
    let seen_ids = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_ids_for_hook = Arc::clone(&seen_ids);
    let hook = TransformHook::new(move |id: &str, source: &str| {
        seen_ids_for_hook.lock().unwrap().push(id.to_string());
        calls_for_hook.fetch_add(1, Ordering::SeqCst);
        if source.contains("ORIGINAL") {
            Ok(source.replace("ORIGINAL", "REWRITTEN"))
        } else {
            Ok(source.to_string())
        }
    });

    let mut o = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.js")]);
    o.transform_hook = Some(hook);

    let out = RolldownBundler::new(dir.path())
        .bundle(&o)
        .expect("bundle with a transform hook");

    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "the hook ran at least once (ids seen: {:?})",
        seen_ids.lock().unwrap()
    );
    assert!(
        seen_ids
            .lock()
            .unwrap()
            .iter()
            .any(|id| id.ends_with("index.js")),
        "the hook must see the user module, not only Rolldown's virtual ones: {:?}",
        seen_ids.lock().unwrap()
    );

    let entry = out
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Entry)
        .expect("entry chunk");
    let js = std::fs::read_to_string(dir.path().join("dist").join(&entry.file)).unwrap();
    assert!(
        js.contains("REWRITTEN"),
        "the hook's replacement must reach the chunk:\n{js}"
    );
    assert!(
        !js.contains("ORIGINAL"),
        "the original text must be gone:\n{js}"
    );
}

#[test]
fn transform_hook_error_does_not_fail_the_build() {
    // A plugin that errors is reported and skipped; the build continues
    // with the module's original source. (The CLI prints the plugin's
    // diagnostic to stderr; failing the build on a plugin error is a
    // policy choice left for a follow-up.)
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/index.js", "export const a = 1;\n");

    let hook = TransformHook::new(|_id: &str, _source: &str| {
        Err(oxide_bundler::err("PLG-TEST", "simulated plugin failure"))
    });
    let mut o = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.js")]);
    o.transform_hook = Some(hook);

    let out = RolldownBundler::new(dir.path())
        .bundle(&o)
        .expect("a plugin error must not fail the bundle");
    assert!(!out.chunks.is_empty(), "the chunk is still emitted");
}

// ---------------------------------------------------------------------------
// §6.6: a `.css` import through `RolldownBundler` must run the CSS transform
// hook and emit the result as an asset. Rolldown 1.2.x refuses a `.css`
// module outright (`module_task.rs` → "Bundling CSS is no longer supported");
// the `CssPlugin` intercepts it in the `load` hook and emits the stylesheet
// via `ctx.emit_file`. Without that plugin the build fails before the hook
// can run — this test pins the behaviour, not just the hook contract.
// ---------------------------------------------------------------------------

use oxide_bundler::CssTransformHook;

#[test]
fn rolldown_css_plugin_emits_transformed_stylesheet() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/index.js",
        "import './theme.css';\nexport const x = 1;\nconsole.log(x);\n",
    );
    write(dir.path(), "src/theme.css", ".a { color: #ff0000; }\n");

    let hook = CssTransformHook::new(
        |_path: &str, source: &str, _is_module: bool, _minify: bool| {
            // A marker suffix proves the transform ran and its output is what
            // reached disk. A real hook (the CLI's) lowers the CSS via Lightning.
            Ok((format!("{source}\n/* CSS-HOOK-RAN */\n"), BTreeMap::new()))
        },
    );
    let mut o = opts(dir.path(), BundleEnv::Client, &[("index", "src/index.js")]);
    o.css_transform = Some(hook);

    let out = RolldownBundler::new(dir.path())
        .bundle(&o)
        .expect("a .css import must not fail the build");

    // The entry chunk imports the stylesheet, so the emitted CSS must exist
    // on disk with the hook's output.
    let css_paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path().join("dist"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "css"))
        .collect();
    assert!(
        !css_paths.is_empty(),
        "expected an emitted .css, dir: {:?}",
        std::fs::read_dir(dir.path().join("dist"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    );
    let css = std::fs::read_to_string(&css_paths[0]).unwrap();
    assert!(
        css.contains("CSS-HOOK-RAN"),
        "the hook output must reach disk:\n{css}"
    );
    assert!(
        css.contains("#ff0000") || css.contains("color"),
        "original declaration must survive:\n{css}"
    );

    // And the JS chunk must carry a default export for the stylesheet URL,
    // not the raw CSS text.
    let entry = out
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Entry)
        .expect("entry chunk");
    let js = std::fs::read_to_string(dir.path().join("dist").join(&entry.file)).unwrap();
    assert!(
        !js.contains("color: #ff0000") && !js.contains("color:#ff0000"),
        "raw CSS must not leak into the JS chunk:\n{js}"
    );
}

#[test]
fn rolldown_without_a_css_import_is_unaffected() {
    // A build with no stylesheet must behave exactly as before — the CSS
    // plugin is registered on every run, so it must not perturb a pure-JS
    // project.
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/index.js", "export const a = 1;\n");
    let out = RolldownBundler::new(dir.path())
        .bundle(&opts(
            dir.path(),
            BundleEnv::Client,
            &[("index", "src/index.js")],
        ))
        .expect("pure-JS bundle");
    assert_eq!(out.chunks.len(), 1);
    assert!(out.client_manifest.is_some());
}

#[test]
fn render_chunk_hook_rewrites_emitted_files() {
    // A renderChunk hook receives the emitted chunk file name + its source
    // and returns the rewritten source; the bundler writes it back.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.ts"), "export const x = 1;\n").unwrap();

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_for_hook = seen.clone();
    let hook = oxide_bundler::RenderChunkHook::new(move |file, src| {
        seen_for_hook.lock().unwrap().push(file.to_string());
        Ok(Some(format!("// renderChunk was here\n{src}")))
    });

    let mut o = opts(root, BundleEnv::Client, &[("main", "src/main.ts")]);
    o.render_chunk_hook = Some(hook);

    let rb = oxide_bundler::RolldownBundler::new(root);
    use oxide_bundler::Bundler as _;
    let _ = rb.bundle(&o).expect("bundle");

    assert!(
        !seen.lock().unwrap().is_empty(),
        "hook saw at least one chunk",
    );
    let mut found = false;
    for entry in std::fs::read_dir(root.join("dist")).unwrap().flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) == Some("js") {
            let body = std::fs::read_to_string(entry.path()).unwrap();
            if body.contains("// renderChunk was here") {
                found = true;
                break;
            }
        }
    }
    assert!(
        found,
        "at least one emitted .js carries the renderChunk marker"
    );
}

#[test]
fn generate_bundle_hook_can_veto_a_chunk() {
    // A generateBundle hook returns the file list the build should expose.
    // The bundler keeps only chunks whose file is in the returned list.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.ts"), "export const x = 1;\n").unwrap();

    let hook = oxide_bundler::GenerateBundleHook::new(|_files| Ok(Some(Vec::new())));
    let mut o = opts(root, BundleEnv::Client, &[("main", "src/main.ts")]);
    o.generate_bundle_hook = Some(hook);

    let rb = oxide_bundler::RolldownBundler::new(root);
    use oxide_bundler::Bundler as _;
    let outcome = rb.bundle(&o).expect("bundle");
    assert!(
        outcome.chunks.is_empty(),
        "a hook returning an empty list keeps no chunk in the manifest; got {}",
        outcome.chunks.len(),
    );
}
