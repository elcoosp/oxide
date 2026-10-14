//! §6.3: the bundler must refuse `.vue` and `.svelte` files loudly. It has
//! no access to the tooling pool (which owns `@vue/compiler-sfc` and
//! `svelte/compiler`); silently emitting the raw SFC into a `.js` chunk
//! would just move the failure to deploy time.
//!
//! The CLI-side precompile pass is the follow-up; these tests pin the
//! "loud refusal" behavior so a caller that forgets to precompile gets a
//! `BLD-FRAMEWORK` diagnostic instead of garbage output.

use oxide_bundler::{BundleEnv, BundleOptions, Bundler, ChunkBundler};
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
        css_transform: None,
    }
}

#[test]
fn vue_sfc_entry_is_refused_with_bld_framework() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/routes/index/page.vue",
        "<script setup>\nconst x = 1\n</script>\n<template><div>{{ x }}</div></template>\n",
    );
    let out = ChunkBundler::new(dir.path())
        .bundle(&opts(
            dir.path(),
            BundleEnv::Ssr,
            &[("index", "src/routes/index/page.vue")],
        ))
        .unwrap();
    assert!(
        out.diagnostics.iter().any(|d| d.code == "BLD-FRAMEWORK"),
        "expected BLD-FRAMEWORK diagnostic, got {:?}",
        out.diagnostics
    );
    // No chunk should be emitted for the refused module.
    assert!(
        out.chunks.iter().all(|c| !c.name.contains("page")),
        "no chunk should be emitted for the refused vue module"
    );
}

#[test]
fn svelte_entry_is_refused_with_bld_framework() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/routes/index/page.svelte",
        "<script>let x = 1;</script>\n<h1>{x}</h1>\n",
    );
    let out = ChunkBundler::new(dir.path())
        .bundle(&opts(
            dir.path(),
            BundleEnv::Ssr,
            &[("index", "src/routes/index/page.svelte")],
        ))
        .unwrap();
    assert!(
        out.diagnostics.iter().any(|d| d.code == "BLD-FRAMEWORK"),
        "expected BLD-FRAMEWORK diagnostic, got {:?}",
        out.diagnostics
    );
}

#[test]
fn tsx_entry_is_still_accepted() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/routes/index/page.tsx",
        "export default function Page() { return <h1>ok</h1>; }\n",
    );
    let out = ChunkBundler::new(dir.path())
        .bundle(&opts(
            dir.path(),
            BundleEnv::Ssr,
            &[("index", "src/routes/index/page.tsx")],
        ))
        .unwrap();
    assert!(
        !out.diagnostics.iter().any(|d| d.code == "BLD-FRAMEWORK"),
        "tsx must not trigger BLD-FRAMEWORK"
    );
}
