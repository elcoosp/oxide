//! Regression guard for the open bug in the 2026-10-13 handoff: `bundle_impl`
//! used to emit raw TypeScript + JSX into SSR chunks. Now every non-asset
//! module runs through `oxide_compiler::OxcTransformer` (Oxc parse + transform
//! + codegen) before it lands in a chunk file.
//!
//! These tests shell out to `node --check` on every emitted `.js` chunk in
//! both `BundleEnv::Ssr` and `BundleEnv::Client` — the same check that failed
//! on the smoke-app's server chunks at handoff time.

use oxide_bundler::{BundleEnv, BundleOptions, Bundler, ChunkBundler};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

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
    }
}

/// Shell out to `node --check <file>`. Returns Ok on success, Err with the
/// captured stderr otherwise. Skips cleanly if `node` is not on PATH.
fn node_check(file: &Path) -> Result<(), String> {
    let out = Command::new("node")
        .arg("--check")
        .arg(file)
        .output()
        .map_err(|e| format!("failed to spawn node: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "node --check {} failed:\n{}",
            file.display(),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn assert_all_chunks_parse_as_js(root: &Path) -> usize {
    let mut count = 0usize;
    for entry in walk(&root.join("dist")) {
        if entry.extension().is_none_or(|e| e != "js") {
            continue;
        }
        node_check(&entry).unwrap_or_else(|e| panic!("{e}"));
        count += 1;
    }
    assert!(count > 0, "no .js chunks were emitted under {}", root.display());
    count
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn ssr_chunk_emits_plain_js_for_tsx_page() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/routes/index/page.tsx",
        "interface Props { name: string }\n\
         export default function Page(p: Props) {\n\
           return <h1>Hello {p.name}</h1>;\n\
         }\n",
    );
    let out = ChunkBundler::new(dir.path())
        .bundle(&opts(dir.path(), BundleEnv::Ssr, &[("index", "src/routes/index/page.tsx")]))
        .unwrap();
    // no diagnostics from the compiler's parse pass
    assert!(
        !out.diagnostics.iter().any(|d| d.code == "CMP-PARSE"),
        "unexpected parse diagnostics: {:?}",
        out.diagnostics
    );
    let n = assert_all_chunks_parse_as_js(dir.path());
    assert!(n >= 1, "expected at least one emitted chunk");
}

#[test]
fn ssr_chunk_emits_plain_js_for_ts_loader() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/routes/index/page.server.ts",
        "export interface LoaderData { title: string }\n\
         export async function loader(): Promise<LoaderData> {\n\
           return { title: 'Hi' };\n\
         }\n",
    );
    ChunkBundler::new(dir.path())
        .bundle(&opts(dir.path(), BundleEnv::Ssr, &[("index", "src/routes/index/page.server.ts")]))
        .unwrap();
    assert_all_chunks_parse_as_js(dir.path());
}

#[test]
fn client_chunk_emits_plain_js_for_tsx_page() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/routes/index/page.tsx",
        "export default function Page() {\n\
           return <div className=\"x\">ok</div>;\n\
         }\n",
    );
    ChunkBundler::new(dir.path())
        .bundle(&opts(dir.path(), BundleEnv::Client, &[("index", "src/routes/index/page.tsx")]))
        .unwrap();
    assert_all_chunks_parse_as_js(dir.path());
}
