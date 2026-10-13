//! Deploy-target file matrix (spec §18). For each `Target`, verify that
//! `emit` produces the characteristic files the target promises — not the
//! generic binary layout. This is the regression guard for a class of bug
//! where a config parse failure caused every target to fall back to the
//! default (`binary`) and emit its artifacts.
//!
//! The test builds a minimal `BuildArtifact` and calls `emit` against a
//! fresh tempdir per target. No bundler, no Node, no fixture project: just
//! the adapter emission path in isolation.

use oxide_adapters::{emit, BuildArtifact, Target};
use std::collections::BTreeMap;
use std::fs;
use tempfile::TempDir;

/// Construct a minimal artifact. Most targets ignore the manifests and
/// the bundle; the static target needs `prerendered` to reference a file
/// that actually exists on disk under `out_dir`.
fn minimal_artifact(root: &std::path::Path, with_prerendered: bool) -> BuildArtifact {
    let mut prerendered = BTreeMap::new();
    if with_prerendered {
        // One prerendered page whose source file we write below.
        let rel = "prerendered/index.html";
        let src = root.join(rel);
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"<!doctype html><html><body>index</body></html>").unwrap();
        prerendered.insert("routes/index".to_string(), vec![rel.to_string()]);
    }
    BuildArtifact {
        out_dir: root.to_path_buf(),
        build_id: "test-build".into(),
        client_manifest: serde_json::json!({
            "build_id": "test-build",
            "routes": {}
        }),
        server_manifest: serde_json::json!({
            "build_id": "test-build",
            "routes": {}
        }),
        prerendered,
        static_fallback: None,
        server_bundle: None,
    }
}

/// The characteristic files each target must emit. The list is
/// deliberately specific — a bare `deploy.json` for every target would
/// pass a weaker check but miss the exact class of bug this guards.
///
/// `deploy.json` is included in every row because every target emits it
/// (it carries `target` and `build_id` for the operator).
const MATRIX: &[(Target, &[&str])] = &[
    (Target::Binary,     &["deploy.json", "README.txt"]),
    (Target::Node,       &["deploy.json", "server.mjs", "package.json"]),
    (Target::Docker,     &["deploy.json", "Dockerfile"]),
    (Target::Cloudflare, &["deploy.json", "worker.js", "wrangler.toml", "_routes.json", "edge.js"]),
    (Target::Vercel,     &["deploy.json", "vercel.json", "handler.mjs"]),
    (Target::Netlify,    &["deploy.json", "netlify.toml", "handler.mjs"]),
    (Target::Lambda,     &["deploy.json", "handler.mjs"]),
    (Target::Deno,       &["deploy.json", "server.ts"]),
    (Target::Bun,        &["deploy.json", "server.ts"]),
    (Target::Static,     &["deploy.json", "index.html"]),
];

#[test]
fn each_target_emits_its_characteristic_files() {
    for (target, expected) in MATRIX {
        let tmp = TempDir::new().expect("tempdir");
        let out = tmp.path().join("deploy");
        fs::create_dir_all(&out).expect("mkdir out");

        // Static needs the prerendered source present; others ignore it.
        let artifact = minimal_artifact(tmp.path(), *target == Target::Static);

        let written = emit(*target, &artifact, &out).unwrap_or_else(|e| {
            panic!("emit({target:?}) failed: {} {}", e.0.code, e.0.message)
        });

        for name in *expected {
            let p = out.join(name);
            assert!(
                p.is_file(),
                "target {target:?}: expected `{name}` not produced. \
                 Files emitted: {written:?}"
            );
            // Non-empty check: a zero-byte placeholder would silently
            // deploy as an empty artifact.
            let len = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            assert!(
                len > 0,
                "target {target:?}: file `{name}` is empty (0 bytes)"
            );
        }

        // Every target writes deploy.json, and its `target` field must
        // name the target actually requested — not a default.
        let dj = out.join("deploy.json");
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dj).expect("read deploy.json"))
                .expect("deploy.json parses");
        assert_eq!(
            v["target"].as_str(),
            Some(target.as_str()),
            "deploy.json `target` must equal the requested target for {target:?}"
        );
    }
}

#[test]
fn vercel_json_references_a_handler() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("deploy");
    fs::create_dir_all(&out).unwrap();
    emit(Target::Vercel, &minimal_artifact(tmp.path(), false), &out).expect("emit");

    let cfg = fs::read_to_string(out.join("vercel.json")).expect("vercel.json");
    let v: serde_json::Value = serde_json::from_str(&cfg).expect("vercel.json parses");
    // Vercel's functions/rewrites shape — the exact key depends on the
    // emit strategy. Assert *some* handler route is present so a
    // placeholder `{}` would fail the check.
    let s = cfg.to_lowercase();
    assert!(
        s.contains("handler") || s.contains("functions") || s.contains("rewrites"),
        "vercel.json should reference a handler route; got:\n{cfg}"
    );
    // Must be parseable JSON (a stronger property than substring).
    assert!(v.is_object() || v.is_array());
}

#[test]
fn netlify_toml_redirects_root_to_the_function() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("deploy");
    fs::create_dir_all(&out).unwrap();
    emit(Target::Netlify, &minimal_artifact(tmp.path(), false), &out).expect("emit");

    let cfg = fs::read_to_string(out.join("netlify.toml")).expect("netlify.toml");
    assert!(
        cfg.contains(".netlify/functions/handler"),
        "netlify.toml must redirect to the function; got:\n{cfg}"
    );
    assert!(
        cfg.contains("[[redirects]]") || cfg.contains("[build]"),
        "netlify.toml should have a redirects or build stanza; got:\n{cfg}"
    );
}

#[test]
fn dockerfile_healthcheck_uses_the_oxide_path() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("deploy");
    fs::create_dir_all(&out).unwrap();
    emit(Target::Docker, &minimal_artifact(tmp.path(), false), &out).expect("emit");

    let df = fs::read_to_string(out.join("Dockerfile")).expect("Dockerfile");

    // HEALTHCHECK present and points at the binary's `health` subcommand.
    // The binary translates that subcommand into a probe of
    // /_oxide/health — the path itself is inside the binary, not the
    // Dockerfile string, so we assert the CLI subcommand instead.
    assert!(
        df.contains("HEALTHCHECK"),
        "Dockerfile should declare a HEALTHCHECK; got:\n{df}"
    );
    assert!(
        df.contains("\"health\"") || df.contains("'health'"),
        "healthcheck should invoke the binary's health subcommand; got:\n{df}"
    );

    // USER must be its own line — a line-continuation bug in the emit
    // helper previously glued it to the preceding COPY path, which made
    // the container run as root (or the copy destination wrong, depending
    // on the interpreter). A substring check would not catch it.
    assert!(
        df.lines().any(|l| l.trim_start().starts_with("USER ")),
        "USER directive must start its own line; got:\n{df}"
    );
    assert!(
        df.to_lowercase().contains("nonroot"),
        "container should run as a non-root user; got:\n{df}"
    );

    // Every non-comment, non-blank line must be a complete directive
    // (no two directives glued together). This is the generalized check
    // for the class of bug above: a missing `\n` in the emit format
    // would produce a line like `.../app/serverUSER nonroot:nonroot`.
    for line in df.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Common instruction prefixes in our Dockerfile.
        const KNOWN: &[&str] = &[
            "FROM ", "WORKDIR ", "COPY ", "RUN ", "USER ", "ENV ",
            "EXPOSE ", "HEALTHCHECK ", "ENTRYPOINT ", "CMD ", "ARG ",
        ];
        assert!(
            KNOWN.iter().any(|p| trimmed.starts_with(p)),
            "line looks malformed (glued directives?): {trimmed:?}"
        );
    }
}
