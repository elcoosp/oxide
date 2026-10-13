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
    write(dir.path(), "src/handler.js", "export const GET = () => ({ ok: true });\n");
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

    let mut o = opts(
        dir.path(),
        BundleEnv::Client,
        &[("index", "src/index.js")],
    );
    o.externals = vec!["lodash".to_string()];
    let out = RolldownBundler::new(dir.path()).bundle(&o).unwrap();

    // The external specifier must survive as an import in the output.
    let entry = out.chunks.iter().find(|c| c.kind == ChunkKind::Entry).unwrap();
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

    let mut o = opts(
        dir.path(),
        BundleEnv::Client,
        &[("index", "src/index.js")],
    );
    o.define
        .insert("process.env.NODE_ENV".to_string(), "\"production\"".to_string());
    let out = RolldownBundler::new(dir.path()).bundle(&o).unwrap();

    let entry = out.chunks.iter().find(|c| c.kind == ChunkKind::Entry).unwrap();
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

    let mut o = opts(
        dir.path(),
        BundleEnv::Client,
        &[("index", "src/index.ts")],
    );
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
