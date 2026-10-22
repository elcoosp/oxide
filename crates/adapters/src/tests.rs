//! TDD suite for oxide-adapters (spec §18, §10.5).
//!
//! Written BEFORE the implementation; every pub fn of the crate is exercised.

use super::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

// ------------------------------------------------------------------ helpers

fn caps(target: Target) -> Vec<(Capability, FeatureSupport)> {
    capability_matrix(target).into_iter().collect()
}

fn artifact(build_id: &str, server_manifest: serde_json::Value) -> BuildArtifact {
    BuildArtifact {
        out_dir: PathBuf::from("/nonexistent-artifact-out"),
        build_id: build_id.into(),
        client_manifest: serde_json::json!({ "build_id": build_id, "routes": {} }),
        server_manifest,
        prerendered: BTreeMap::new(),
        static_fallback: None,
        server_bundle: None,
    }
}

fn manifest_json() -> serde_json::Value {
    serde_json::json!({
        "build_id": "b1",
        "routes": { "routes/index/page": { "module": "a.js", "render_mode": "static", "tags": [], "prerendered": null } }
    })
}

fn all_files(dir: &std::path::Path, written: &[PathBuf]) -> Vec<(PathBuf, String)> {
    written
        .iter()
        .map(|p| {
            let bytes = std::fs::read(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
            assert!(!bytes.is_empty(), "{} must be non-empty", p.display());
            assert!(
                p.starts_with(dir),
                "{} must live under {}",
                p.display(),
                dir.display()
            );
            (p.clone(), String::from_utf8_lossy(&bytes).to_string())
        })
        .collect()
}

// ------------------------------------------------------- capability matrix (§18)

#[test]
fn dockerfile_user_is_on_its_own_line() {
    // Regression: the emit helper built the Dockerfile with a Rust
    // line-continuation that swallowed the newline before `USER`, so the
    // emitted file was `.../app/serverUSER nonroot:nonroot`. The
    // `USER` instruction therefore never ran — the container defaulted to
    // root. Assert the emitted bytes have `USER` at line start.
    let df = dockerfile(false);
    let user_lines: Vec<&str> = df
        .lines()
        .filter(|l| l.trim_start().starts_with("USER "))
        .collect();
    assert_eq!(
        user_lines.len(),
        1,
        "expected exactly one USER directive:\n{df}"
    );
    assert_eq!(user_lines[0].trim(), "USER nonroot:nonroot");
    // The preceding COPY must end at `/app/server`, not `/app/serverUSER`.
    assert!(
        df.contains("/app/server\nUSER")
            || df.lines().any(|l| l.trim_end().ends_with("/app/server")),
        "COPY destination must terminate at /app/server:\n{df}"
    );
}

#[test]
fn dockerfile_every_instruction_starts_its_own_line() {
    let df = dockerfile(false);
    const KNOWN: &[&str] = &[
        "FROM ",
        "WORKDIR ",
        "COPY ",
        "RUN ",
        "USER ",
        "ENV ",
        "EXPOSE ",
        "HEALTHCHECK ",
        "ENTRYPOINT ",
        "CMD ",
        "ARG ",
    ];
    for line in df.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        assert!(
            KNOWN.iter().any(|p| t.starts_with(p)),
            "line looks malformed (glued directives?): {t:?}"
        );
    }
}

#[test]
fn capability_matrix_binary_is_all_full() {
    let m = caps(Target::Binary);
    assert_eq!(m.len(), 7, "all seven §18 capabilities reported");
    for (cap, sup) in &m {
        assert_eq!(*sup, FeatureSupport::Full, "{cap:?} must be Full on binary");
    }
}

#[test]
fn capability_matrix_docker_matches_binary() {
    assert_eq!(
        caps(Target::Docker),
        caps(Target::Binary),
        "§18: docker = binary"
    );
}

#[test]
fn capability_matrix_node_deno_bun_match() {
    let node = caps(Target::Node);
    assert_eq!(caps(Target::Deno), node, "§18: deno = node");
    assert_eq!(caps(Target::Bun), node, "§18: bun = node");
    for (cap, sup) in &node {
        if *cap == Capability::Snapshots {
            assert_eq!(
                *sup,
                FeatureSupport::Degraded,
                "node snapshots = degraded (code cache)"
            );
        } else {
            assert_eq!(*sup, FeatureSupport::Full, "{cap:?} must be Full on node");
        }
    }
}

#[test]
fn capability_matrix_static_exact() {
    use Capability::*;
    use FeatureSupport::*;
    let m = capability_matrix(Target::Static);
    assert_eq!(m.get(&L0Cache), Some(&Degraded), "static L0 = CDN");
    assert_eq!(m.get(&Isr), Some(&Unsupported));
    assert_eq!(m.get(&Sockets), Some(&Unsupported));
    assert_eq!(
        m.get(&ImageOptimization),
        Some(&Degraded),
        "build-time only"
    );
    assert_eq!(
        m.get(&NativeRules),
        Some(&Degraded),
        "_headers/_redirects subset"
    );
    assert_eq!(m.get(&Snapshots), Some(&Unsupported));
    assert_eq!(m.get(&Tls), Some(&Unsupported));
}

#[test]
fn capability_matrix_cloudflare_exact() {
    use Capability::*;
    use FeatureSupport::*;
    let m = capability_matrix(Target::Cloudflare);
    assert_eq!(m.get(&L0Cache), Some(&Degraded), "KV-backed");
    assert_eq!(m.get(&Isr), Some(&Degraded));
    assert_eq!(m.get(&Sockets), Some(&Unsupported));
    assert_eq!(m.get(&ImageOptimization), Some(&Unsupported));
    assert_eq!(m.get(&NativeRules), Some(&Full));
    assert_eq!(m.get(&Snapshots), Some(&Unsupported));
    assert_eq!(m.get(&Tls), Some(&Full), "TLS terminates at the edge");
}

#[test]
fn capability_matrix_serverless_targets_match() {
    let lambda = caps(Target::Lambda);
    assert_eq!(caps(Target::Vercel), lambda, "§18: vercel = lambda class");
    assert_eq!(caps(Target::Netlify), lambda, "§18: netlify = lambda class");
    use Capability::*;
    use FeatureSupport::*;
    let m = capability_matrix(Target::Lambda);
    assert_eq!(m.get(&L0Cache), Some(&Degraded));
    assert_eq!(m.get(&Isr), Some(&Degraded));
    assert_eq!(m.get(&Sockets), Some(&Unsupported));
    assert_eq!(m.get(&ImageOptimization), Some(&Degraded));
    assert_eq!(m.get(&NativeRules), Some(&Full));
    assert_eq!(m.get(&Snapshots), Some(&Degraded));
    assert_eq!(m.get(&Tls), Some(&Unsupported), "handled by platform");
}

#[test]
fn degradation_explains_degraded_only() {
    // Degraded cells carry a documented reason.
    assert!(degradation(Target::Node, Capability::Snapshots).is_some());
    assert!(degradation(Target::Static, Capability::L0Cache).is_some());
    assert!(degradation(Target::Static, Capability::NativeRules).is_some());
    assert!(degradation(Target::Static, Capability::ImageOptimization).is_some());
    assert!(degradation(Target::Cloudflare, Capability::L0Cache).is_some());
    assert!(degradation(Target::Cloudflare, Capability::Isr).is_some());
    assert!(degradation(Target::Lambda, Capability::L0Cache).is_some());
    assert!(degradation(Target::Lambda, Capability::Snapshots).is_some());
    // Full and Unsupported cells carry no degradation note.
    assert_eq!(degradation(Target::Binary, Capability::Sockets), None);
    assert_eq!(degradation(Target::Cloudflare, Capability::Sockets), None);
    assert_eq!(degradation(Target::Node, Capability::L0Cache), None);
}

// ------------------------------------------------------------------ emit (§18)

#[test]
fn emit_binary_writes_manifests_deploy_json_and_readme() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("deploy");
    let written = emit(Target::Binary, &artifact("b1", manifest_json()), &out).unwrap();
    let files = all_files(out.as_path(), &written);
    let names: Vec<String> = written
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for expected in [
        "deploy.json",
        "README.txt",
        "client.manifest.json",
        "server.manifest.json",
    ] {
        assert!(
            names.contains(&expected.into()),
            "binary emit missing {expected}"
        );
    }
    let deploy = files
        .iter()
        .find(|(p, _)| p.ends_with("deploy.json"))
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&deploy.1).unwrap();
    assert_eq!(v["target"], "binary");
    assert_eq!(v["build_id"], "b1");
    assert_eq!(v["entry"], "server");
    let readme = files
        .iter()
        .find(|(p, _)| p.ends_with("README.txt"))
        .unwrap();
    assert!(
        readme.1.contains("oxide start"),
        "README must point at `oxide start`"
    );
}

#[test]
fn emit_node_writes_entry_package_and_deploy_json() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("deploy");
    let written = emit(Target::Node, &artifact("b2", manifest_json()), &out).unwrap();
    let files = all_files(out.as_path(), &written);
    let get = |name: &str| files.iter().find(|(p, _)| p.ends_with(name)).unwrap();
    let server = get("server.mjs");
    assert!(server.1.contains("b2"), "entry interpolates build_id");
    assert!(
        server.1.contains("./server/index.js"),
        "imports the server bundle"
    );
    assert!(
        server.1.contains("http.createServer"),
        "bridges to Node http"
    );
    let pkg: serde_json::Value = serde_json::from_str(&get("package.json").1).unwrap();
    assert_eq!(pkg["type"], "module");
    assert_eq!(pkg["main"], "server.mjs");
    let deploy: serde_json::Value = serde_json::from_str(&get("deploy.json").1).unwrap();
    assert_eq!(deploy["target"], "node");
    assert_eq!(deploy["entry"], "server.mjs");
}

#[test]
fn emit_static_copies_prerendered_and_emits_headers_redirects() {
    let dir = tempfile::tempdir().unwrap();
    let build_out = dir.path().join("dist");
    std::fs::create_dir_all(build_out.join("prerendered")).unwrap();
    std::fs::write(
        build_out.join("prerendered/index.html"),
        "<!doctype html><html><body>home</body></html>",
    )
    .unwrap();
    std::fs::create_dir_all(build_out.join("prerendered/blog")).unwrap();
    std::fs::write(
        build_out.join("prerendered/blog/hello.html"),
        "<!doctype html><html><body>post</body></html>",
    )
    .unwrap();

    let mut art = artifact("b3", manifest_json());
    art.out_dir = build_out.clone();
    art.prerendered.insert(
        "routes/index/page".into(),
        vec!["prerendered/index.html".into()],
    );
    art.prerendered.insert(
        "routes/blog/hello/page".into(),
        vec!["prerendered/blog/hello.html".into()],
    );

    let out = dir.path().join("deploy");
    let written = emit(Target::Static, &art, &out).unwrap();
    let files = all_files(out.as_path(), &written);
    let get = |name: &str| files.iter().find(|(p, _)| p.ends_with(name)).unwrap();
    assert_eq!(
        get("index.html").1,
        "<!doctype html><html><body>home</body></html>"
    );
    assert_eq!(
        get("hello.html").1,
        "<!doctype html><html><body>post</body></html>"
    );
    let headers = &get("_headers").1;
    assert!(
        headers.starts_with("/*\n"),
        "_headers starts with a /* section"
    );
    assert!(headers.contains("X-Frame-Options: DENY"));
    assert!(headers.contains("X-Content-Type-Options: nosniff"));
    let redirects = &get("_redirects").1;
    assert!(
        redirects.contains("no redirects"),
        "empty rules → comment-only file"
    );
}

#[test]
fn emit_static_refuses_dynamic_route_without_prerender() {
    let dir = tempfile::tempdir().unwrap();
    let sm = serde_json::json!({
        "routes": { "routes/dashboard/page": { "module": "d.js", "render_mode": "dynamic" } }
    });
    let art = artifact("b4", sm);
    let out = dir.path().join("deploy");
    let e = emit(Target::Static, &art, &out).unwrap_err();
    assert_eq!(
        e.0.code, "ADP-STATIC-DYN",
        "§10.5 failure rule via §18 refusal"
    );
    assert!(e.0.message.contains("dashboard"));
}

#[test]
fn emit_refuses_unsupported_capability_naming_target_and_feature() {
    let dir = tempfile::tempdir().unwrap();
    let sm = serde_json::json!({ "requires": { "sockets": true } });
    let art = artifact("b5", sm.clone());
    let out = dir.path().join("deploy");
    let e = emit(Target::Cloudflare, &art, &out).unwrap_err();
    assert_eq!(e.0.code, "ADP-CAP");
    assert!(e.0.message.contains("cloudflare"), "error names the target");
    assert!(e.0.message.contains("sockets"), "error names the feature");
    // Same requirement on a target with Full sockets is accepted.
    let ok = emit(
        Target::Binary,
        &artifact("b5", sm.clone()),
        &dir.path().join("d2"),
    )
    .unwrap();
    assert!(!ok.is_empty());
    // A degraded capability never refuses.
    let isr = serde_json::json!({ "requires": { "isr": true } });
    emit(
        Target::Cloudflare,
        &artifact("b6", isr),
        &dir.path().join("d3"),
    )
    .unwrap();
}

#[test]
fn emit_cloudflare_writes_worker_wrangler_and_deploy() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("deploy");
    let written = emit(Target::Cloudflare, &artifact("b7", manifest_json()), &out).unwrap();
    let files = all_files(out.as_path(), &written);
    let get = |name: &str| files.iter().find(|(p, _)| p.ends_with(name)).unwrap();
    let worker = &get("worker.js").1;
    assert!(worker.contains("b7"));
    assert!(worker.contains("fetch"), "WinterCG handler contract");
    let wrangler = &get("wrangler.toml").1;
    assert!(wrangler.contains("main = \"worker.js\""));
    assert!(wrangler.contains("compatibility_date"));
    assert!(wrangler.contains("name ="));
    let deploy: serde_json::Value = serde_json::from_str(&get("deploy.json").1).unwrap();
    assert_eq!(deploy["target"], "cloudflare");
    assert_eq!(deploy["entry"], "worker.js");
}

#[test]
fn emit_serverless_targets_write_handler_and_configs() {
    let dir = tempfile::tempdir().unwrap();
    for (i, target) in [Target::Lambda, Target::Vercel, Target::Netlify]
        .into_iter()
        .enumerate()
    {
        let out = dir.path().join(format!("d{i}"));
        let written = emit(target, &artifact("b8", manifest_json()), &out).unwrap();
        let files = all_files(out.as_path(), &written);
        let get = |name: &str| files.iter().find(|(p, _)| p.ends_with(name)).unwrap();
        let handler = &get("handler.mjs").1;
        assert!(
            handler.contains("b8"),
            "{:?} handler interpolates build_id",
            target
        );
        assert!(handler.contains("Request"), "web-standard wrapper");
        // The handler file sits at a different depth per platform; the
        // import path back to server/ssr/index.js matches that depth.
        let expected_import = match target {
            Target::Lambda => "./server/ssr/index.js",
            Target::Vercel => "../server/ssr/index.js",
            Target::Netlify => "../../server/ssr/index.js",
            _ => unreachable!(),
        };
        assert!(
            handler.contains(expected_import),
            "{target:?}: handler must import {expected_import:?}; got:\n{handler}"
        );
        let deploy: serde_json::Value = serde_json::from_str(&get("deploy.json").1).unwrap();
        assert_eq!(deploy["target"], target.as_str());
        match target {
            Target::Vercel => assert!(files.iter().any(|(p, _)| p.ends_with("vercel.json"))),
            Target::Netlify => assert!(files.iter().any(|(p, _)| p.ends_with("netlify.toml"))),
            _ => {}
        }
    }
}

#[test]
fn emit_deno_bun_write_server_ts_wrapper() {
    let dir = tempfile::tempdir().unwrap();
    for (i, target) in [Target::Deno, Target::Bun].into_iter().enumerate() {
        let out = dir.path().join(format!("d{i}"));
        let written = emit(target, &artifact("b9", manifest_json()), &out).unwrap();
        let files = all_files(out.as_path(), &written);
        let server_ts = files
            .iter()
            .find(|(p, _)| p.ends_with("server.ts"))
            .unwrap();
        assert!(server_ts.1.contains("b9"));
        assert!(server_ts.1.contains("./server/index.js"));
        match target {
            Target::Deno => assert!(server_ts.1.contains("Deno.serve")),
            Target::Bun => assert!(server_ts.1.contains("Bun.serve")),
            _ => unreachable!(),
        }
    }
}

#[test]
fn emit_docker_writes_dockerfile_and_deploy() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("deploy");
    let written = emit(Target::Docker, &artifact("b10", manifest_json()), &out).unwrap();
    let files = all_files(out.as_path(), &written);
    let df = files
        .iter()
        .find(|(p, _)| p.ends_with("Dockerfile"))
        .unwrap();
    assert!(df.1.contains("gcr.io/distroless/cc"));
    let deploy: serde_json::Value = serde_json::from_str(
        &files
            .iter()
            .find(|(p, _)| p.ends_with("deploy.json"))
            .unwrap()
            .1,
    )
    .unwrap();
    assert_eq!(deploy["target"], "docker");
}

// ------------------------------------------------- headers / redirects (§10.5)

#[test]
fn emit_headers_sections_and_ordering() {
    let mut first: BTreeMap<String, String> = BTreeMap::new();
    first.insert("X-Frame-Options".into(), "DENY".to_string());
    first.insert("X-Content-Type-Options".into(), "nosniff".to_string());
    let mut second: BTreeMap<String, String> = BTreeMap::new();
    second.insert("Cache-Control".into(), "public, max-age=3600".to_string());
    let rules = vec![("/*".to_string(), first), ("/static/*".to_string(), second)];
    let out = emit_headers(&rules);
    assert_eq!(
        out,
        "/*\n  X-Content-Type-Options: nosniff\n  X-Frame-Options: DENY\n\n/static/*\n  Cache-Control: public, max-age=3600\n"
    );
    assert_eq!(emit_headers(&[]), "");
}

#[test]
fn emit_redirects_lines_with_status_default() {
    let rules = vec![
        ("/old".to_string(), "/new".to_string(), 0u16),
        ("/a".to_string(), "/b".to_string(), 308u16),
    ];
    let out = emit_redirects(&rules);
    assert_eq!(out, "/old /new 301\n/a /b 308\n");
    assert_eq!(emit_redirects(&[]), "");
}

// ------------------------------------------------------------------ dockerfile

#[test]
fn dockerfile_is_deterministic_distroless_nonroot_healthcheck() {
    let a = dockerfile(false);
    let b = dockerfile(false);
    assert_eq!(a, b, "deterministic output");
    assert!(
        a.contains("FROM rust:1 AS build"),
        "multi-stage build stage"
    );
    assert!(
        a.contains("FROM gcr.io/distroless/cc"),
        "distroless runtime"
    );
    assert!(a.contains("USER nonroot"), "non-root (§18)");
    assert!(a.contains("HEALTHCHECK"), "healthcheck (§18)");
    assert!(a.contains("EXPOSE 3000"));
    assert!(a.contains("ENTRYPOINT"));
    assert!(
        !a.contains("COPY --from=build /src/dist"),
        "embed=false: no dist baked"
    );
    let e = dockerfile(true);
    assert_eq!(e, dockerfile(true), "deterministic output");
    assert!(e.contains("ARG"), "embed=true: ARG present");
    assert!(
        e.contains("COPY --from=build /src/dist"),
        "assets baked into the image"
    );
}

// ------------------------------------------------------------- JS entries (§18)

#[test]
fn js_entries_are_deterministic_and_reference_handler_contract() {
    let w1 = cloudflare_worker_entry("edge42");
    assert_eq!(w1, cloudflare_worker_entry("edge42"));
    assert!(w1.contains("edge42"));
    assert!(w1.contains("Request"), "web-standard handler contract");
    assert!(
        w1.contains("export default"),
        "module export for the worker"
    );

    let n1 = node_server_entry("node42");
    assert_eq!(n1, node_server_entry("node42"));
    assert!(n1.contains("node42"));
    assert!(n1.contains("import handler from \"./server/index.js\""));
    assert!(n1.contains("http.createServer"));
    assert!(
        n1.contains("new Request("),
        "bridges to web-standard Request"
    );
    assert!(
        n1.contains("new Response") || n1.contains("response.status"),
        "bridges Response"
    );
    // Dependency-free: only the node:http builtin + the local server bundle import.
    let imports: Vec<&str> = n1.lines().filter(|l| l.starts_with("import ")).collect();
    assert_eq!(
        imports,
        vec![
            "import http from \"node:http\";",
            "import handler from \"./server/index.js\";",
        ],
        "no npm dependencies in the node entry"
    );
}

#[test]
fn cloudflare_emits_complete_worker_package() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("deploy");
    let written = emit(Target::Cloudflare, &artifact("b_cf", manifest_json()), &out).unwrap();
    let names: Vec<String> = written
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for expected in [
        "worker.js",
        "edge.js",
        "_routes.json",
        "wrangler.toml",
        "deploy.json",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "missing {expected}: {names:?}"
        );
    }
    // The worker imports edge.js (not the never-written ./server/edge.js).
    let worker = std::fs::read_to_string(out.join("worker.js")).unwrap();
    assert!(worker.contains("from \"./edge.js\""), "{worker}");
    assert!(!worker.contains("./server/edge.js"), "{worker}");
    // edge.js is a real WinterCG handler with the JS fallbacks.
    let edge = std::fs::read_to_string(out.join("edge.js")).unwrap();
    assert!(edge.contains("export default { fetch: handler }"), "{edge}");
    assert!(
        edge.contains("export { cacheKey, mergeHead, encodeOdf }"),
        "{edge}"
    );
    // The routes file excludes assets.
    let routes: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("_routes.json")).unwrap()).unwrap();
    assert_eq!(routes["version"], 1);
    assert!(
        routes["include"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "/*")
    );
    // Wrangler has the compat flags and doesn't declare wasm when absent.
    let wrangler = std::fs::read_to_string(out.join("wrangler.toml")).unwrap();
    assert!(wrangler.contains("main = \"worker.js\""));
    assert!(wrangler.contains("nodejs_compat"));
    assert!(!wrangler.contains("[wasm_modules]"));
}

#[test]
fn cloudflare_wrangler_declares_wasm_when_present() {
    let s = cloudflare_wrangler_toml("b_wasm", true);
    assert!(s.contains("[wasm_modules]"), "{s}");
    assert!(s.contains("edge = \"./edge.wasm\""), "{s}");
    let s2 = cloudflare_wrangler_toml("b_nowasm", false);
    assert!(!s2.contains("[wasm_modules]"), "{s2}");
}

#[test]
fn edge_handler_js_is_deterministic_and_contains_build_id() {
    let a = edge_handler_js("edge-xyz");
    let b = edge_handler_js("edge-xyz");
    assert_eq!(a, b);
    assert!(a.contains("build_id: edge-xyz"));
    assert!(a.contains("edge_cache_key") || a.contains("cacheKey"));
}

/// P0-1: the edge handler's `mergeHead` must be JS-only. The Rust
/// `merge_head` merges flat `Vec<(String, String)>` pairs; the edge head
/// model is structured and deduped per kind. The old wasm path returned
/// `[]` silently (serde_json deserialize failure swallowed by
/// `unwrap_or_default`, then the parse of `"[]"` returned `[]` with no
/// throw, so the JS fallback never ran). This test ensures the template
/// cannot regress into that path.
#[test]
fn edge_merge_head_is_js_only() {
    let js = edge_handler_js("build-abc123");
    // No wasm call for merge_head.
    assert!(
        !js.contains(r#"useWasm("merge_head""#),
        "mergeHead must not call wasm (semantic mismatch: flat pairs vs structured head): \n{}",
        js.lines()
            .filter(|l| l.contains("merge_head"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // The JS implementation is present.
    assert!(
        js.contains("function mergeHead(a, b)"),
        "mergeHead JS implementation must be present"
    );
    // The other two wasm-routed functions still prefer wasm.
    assert!(
        js.contains(r#"useWasm("edge_cache_key""#),
        "cacheKey still prefers wasm"
    );
    assert!(
        js.contains(r#"useWasm("encode_odf""#),
        "encodeOdf still prefers wasm"
    );
}
