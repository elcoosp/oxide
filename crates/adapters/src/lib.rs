//! oxide-adapters — deployment adapters (spec §18).
//!
//! A deploy adapter transforms build output into a deployable artifact. The portable
//! contract is the web-standard handler `(Request) => Promise<Response>` plus the
//! manifests. Targets: binary, node, static, docker, cloudflare, lambda/vercel/netlify,
//! deno/bun. A published capability matrix states per target which features work and
//! how absent ones degrade; the build refuses a configuration that needs an unavailable
//! feature rather than failing at runtime.

use oxide_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
}

/// Deploy targets (§18 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Binary,
    Node,
    Static,
    Docker,
    Cloudflare,
    Lambda,
    Vercel,
    Netlify,
    Deno,
    Bun,
}

impl Target {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "binary" => Target::Binary,
            "node" => Target::Node,
            "static" => Target::Static,
            "docker" => Target::Docker,
            "cloudflare" => Target::Cloudflare,
            "lambda" => Target::Lambda,
            "vercel" => Target::Vercel,
            "netlify" => Target::Netlify,
            "deno" => Target::Deno,
            "bun" => Target::Bun,
            _ => return None,
        })
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Target::Binary => "binary",
            Target::Node => "node",
            Target::Static => "static",
            Target::Docker => "docker",
            Target::Cloudflare => "cloudflare",
            Target::Lambda => "lambda",
            Target::Vercel => "vercel",
            Target::Netlify => "netlify",
            Target::Deno => "deno",
            Target::Bun => "bun",
        }
    }
}

/// Capabilities per target (§18 capability matrix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    L0Cache,
    Isr,
    Sockets,
    ImageOptimization,
    NativeRules,
    Snapshots,
    Tls,
}

/// The capability matrix: which features work per target and how absent ones degrade.
pub fn capability_matrix(target: Target) -> BTreeMap<Capability, FeatureSupport> {
    use Capability::{ImageOptimization, Isr, L0Cache, NativeRules, Snapshots, Sockets, Tls};
    use FeatureSupport::{Degraded, Full, Unsupported};
    /// §18 table row, written as (capability, support) pairs in canonical order.
    fn row(pairs: [(Capability, FeatureSupport); 7]) -> BTreeMap<Capability, FeatureSupport> {
        pairs.into_iter().collect()
    }
    match target {
        // One executable, everything native.
        Target::Binary | Target::Docker => row([
            (L0Cache, Full),
            (Isr, Full),
            (Sockets, Full),
            (ImageOptimization, Full),
            (NativeRules, Full),
            (Snapshots, Full),
            (Tls, Full),
        ]),
        // NodeHost-backed runtimes: only snapshots degrade (code cache warms cold start).
        Target::Node | Target::Deno | Target::Bun => row([
            (L0Cache, Full),
            (Isr, Full),
            (Sockets, Full),
            (ImageOptimization, Full),
            (NativeRules, Full),
            (Snapshots, Degraded),
            (Tls, Full),
        ]),
        // No runtime: everything dynamic is out; what remains is CDN/build-time.
        Target::Static => row([
            (L0Cache, Degraded),
            (Isr, Unsupported),
            (Sockets, Unsupported),
            (ImageOptimization, Degraded),
            (NativeRules, Degraded),
            (Snapshots, Unsupported),
            (Tls, Unsupported),
        ]),
        // Workers: WinterCG edge subset; KV/R2 for cache, no sockets, no images.
        Target::Cloudflare => row([
            (L0Cache, Degraded),
            (Isr, Degraded),
            (Sockets, Unsupported),
            (ImageOptimization, Unsupported),
            (NativeRules, Full),
            (Snapshots, Unsupported),
            (Tls, Full),
        ]),
        // Serverless platforms: remote cache + platform-managed TLS.
        Target::Lambda | Target::Vercel | Target::Netlify => row([
            (L0Cache, Degraded),
            (Isr, Degraded),
            (Sockets, Unsupported),
            (ImageOptimization, Degraded),
            (NativeRules, Full),
            (Snapshots, Degraded),
            (Tls, Unsupported),
        ]),
    }
}

/// Why a [`FeatureSupport::Degraded`] cell degrades, or `None` for Full/Unsupported
/// (absence is not degradation — the refusal rule §18 covers Unsupported at build
/// time, and [`capability_matrix`] is the machine-readable source of truth).
pub fn degradation(target: Target, cap: Capability) -> Option<&'static str> {
    use Capability::{ImageOptimization, Isr, L0Cache, NativeRules, Snapshots};
    let reason: &str = match (target, cap) {
        (Target::Node | Target::Deno | Target::Bun, Snapshots) => {
            "no v8 startup snapshot on this runtime; cold start warmed via code cache"
        }
        (Target::Static, L0Cache) => "cache served from the CDN; no server-side L0 layer",
        (Target::Static, ImageOptimization) => {
            "build-time optimization only; no runtime /_oxide/image endpoint"
        }
        (Target::Static, NativeRules) => {
            "subset: redirects/headers via _redirects/_headers only (no rewrites/auth)"
        }
        (Target::Cloudflare, L0Cache) => "KV/R2-backed cache with eventually-consistent fill",
        (Target::Cloudflare, Isr) => "ISR via stale-while-revalidate at the edge",
        (Target::Lambda | Target::Vercel | Target::Netlify, L0Cache) => {
            "remote cache handler; L0 is cold per instance"
        }
        (Target::Lambda | Target::Vercel | Target::Netlify, Isr) => {
            "ISR via on-demand regeneration where the platform supports it"
        }
        (Target::Lambda | Target::Vercel | Target::Netlify, ImageOptimization) => {
            "build-time optimization plus the platform image service"
        }
        (Target::Lambda | Target::Vercel | Target::Netlify, Snapshots) => {
            "snapshot-friendly cold start; isolate reuse not guaranteed"
        }
        _ => return None,
    };
    Some(reason)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureSupport {
    Full,
    /// Works with degraded behavior; the degradation is documented.
    Degraded,
    Unsupported,
}

/// Build output handed to adapters (from bundler manifests + prerendered routes).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildArtifact {
    pub out_dir: PathBuf,
    pub build_id: String,
    pub client_manifest: serde_json::Value,
    pub server_manifest: serde_json::Value,
    /// Prerendered HTML pages: route id → list of output files (one per
    /// `generateParams` value for a dynamic route, §10.5).
    pub prerendered: BTreeMap<String, Vec<String>>,
    /// Path to a fallback HTML file emitted for dynamic routes without
    /// prerendered output (§10.5). When `Some`, the static target emits it as
    /// `404.html` instead of refusing the build.
    pub static_fallback: Option<String>,
    pub server_bundle: Option<PathBuf>,
}

/// Emit a deployable artifact for `target` into `out_dir` (§18).
/// Emits: manifests, `_headers`/`_redirects` for static (§10.5), Dockerfile for docker,
/// Worker entry for cloudflare, handler wrappers for serverless. Fails the build when a
/// selected feature is unavailable for the target (§18 refusal rule).
pub fn emit(target: Target, artifact: &BuildArtifact, out_dir: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(out_dir).map_err(|e| {
        err(
            "ADP-IO",
            format!("cannot create {}: {e}", out_dir.display()),
        )
    })?;
    check_requirements(target, artifact)?;

    let build_id = artifact.build_id.as_str();
    let mut written: Vec<PathBuf> = Vec::new();
    match target {
        Target::Binary | Target::Docker => {
            written.extend(copy_manifests(artifact, out_dir)?);
            if target == Target::Docker {
                written.push(write_file(out_dir.join("Dockerfile"), &dockerfile(false))?);
            } else {
                // Points the operator at the run command (§18: one executable).
                written.push(write_file(
                    out_dir.join("README.txt"),
                    &format!(
                        "Oxide binary target (build {build_id}).\n\n\
                         Run the compiled server with `oxide start` (or the `server` \
                         binary directly). Health endpoint: /_oxide/health (§12.2).\n",
                    ),
                )?);
            }
            // The run entrypoint is the compiled server binary (§18: one executable).
            written.push(write_deploy(out_dir, target, build_id, "server")?);
        }
        Target::Node => {
            written.push(write_file(
                out_dir.join("server.mjs"),
                &node_server_entry(build_id),
            )?);
            written.push(write_json(
                out_dir.join("package.json"),
                &serde_json::json!({
                    "name": format!("oxide-node-{build_id}"),
                    "private": true,
                    "type": "module",
                    "main": "server.mjs"
                }),
            )?);
            written.push(write_deploy(out_dir, target, build_id, "server.mjs")?);
        }
        Target::Static => {
            // §10.5 failure rule: a dynamic route without prerendered output cannot
            // be statically exported — unless `static.fallback` provides an HTML
            // file to emit as `404.html`, in which case the config accepts the
            // trade-off explicitly.
            if artifact.static_fallback.is_none() {
                if let Some(routes) = route_table(&artifact.server_manifest) {
                    for (route, row) in &routes {
                        let mode = row
                            .get("render_mode")
                            .or_else(|| row.get("renderMode"))
                            .and_then(|v| v.as_str());
                        if mode == Some("dynamic") && !artifact.prerendered.contains_key(route) {
                            return Err(err(
                                "ADP-STATIC-DYN",
                                format!(
                                    "route `{route}` is `dynamic` and has no prerendered output; \
                                     static export requires prerendering or `static.fallback` (§10.5)"
                                ),
                            ));
                        }
                    }
                }
            }
            // Copy every prerendered output. The intermediate `prerendered/`
            // prefix is stripped so files land at host-ready paths
            // (`/blog/hello.html`, not `/prerendered/blog/hello.html`).
            for (route, rels) in &artifact.prerendered {
                for rel in rels {
                    let src = artifact.out_dir.join(rel);
                    let bytes = std::fs::read(&src).map_err(|e| {
                        err(
                            "ADP-IO",
                            format!(
                                "prerendered output for `{route}` missing at {}: {e}",
                                src.display()
                            ),
                        )
                    })?;
                    let host_rel = rel.strip_prefix("prerendered/").unwrap_or(rel);
                    let dst = out_dir.join(host_rel);
                    written.push(write_bytes(dst, &bytes)?);
                }
            }
            // §10.5 fallback: emit the configured HTML as 404.html.
            if let Some(fb) = &artifact.static_fallback {
                let src = artifact.out_dir.join(fb);
                let bytes = std::fs::read(&src).map_err(|e| {
                    err(
                        "ADP-IO",
                        format!(
                            "static.fallback `{fb}` missing at {}: {e}",
                            src.display()
                        ),
                    )
                })?;
                written.push(write_bytes(out_dir.join("404.html"), &bytes)?);
            }
            // Default: security headers for every path; no redirect rules are invented.
            written.push(write_file(
                out_dir.join("_headers"),
                &emit_headers(&[("/*".to_string(), default_security_headers())]),
            )?);
            // No redirect rules by default; the file is kept (comment-only) so hosts
            // that require its presence find it.
            written.push(write_file(
                out_dir.join("_redirects"),
                "# Oxide static export: no redirects configured.\n",
            )?);
            written.push(write_deploy(out_dir, target, build_id, "index.html")?);
        }
        Target::Cloudflare => {
            // Complete Cloudflare Workers package: an ESM entry that consumes
            // the edge handler, the handler itself (WASM-accelerated when the
            // optional `edge.wasm` is present, pure-JS fallback otherwise —
            // spec §2 D18: "Core features work where Rust cannot run V8"), a
            // `_routes.json` for asset bypass, and a `wrangler.toml` that
            // wires the wasm module when present.
            written.push(write_file(
                out_dir.join("worker.js"),
                &cloudflare_worker_entry(build_id),
            )?);
            written.push(write_file(
                out_dir.join("edge.js"),
                &edge_handler_js(build_id),
            )?);
            // `_routes.json`: bypass the Worker for static assets so the CDN
            // serves them directly (Cloudflare convention).
            written.push(write_json(
                out_dir.join("_routes.json"),
                &serde_json::json!({
                    "version": 1,
                    "include": ["/*"],
                    "exclude": [
                        "/assets/*",
                        "/_oxide/health",
                        "/*.ico",
                        "/*.png",
                        "/*.svg",
                        "/*.woff2",
                    ]
                }),
            )?);
            // Does the caller supply an edge.wasm? The CLI compiles
            // `oxide-core-wasm` to wasm32-unknown-unknown best-effort and adds
            // its path to the artifact when successful. Absence is normal and
            // the JS fallback covers it.
            let has_wasm = artifact
                .out_dir
                .join("edge.wasm")
                .is_file();
            written.push(write_file(
                out_dir.join("wrangler.toml"),
                &cloudflare_wrangler_toml(build_id, has_wasm),
            )?);
            if has_wasm {
                // Copy the compiled WASM alongside the worker.
                let src = artifact.out_dir.join("edge.wasm");
                let bytes = std::fs::read(&src).map_err(|e| {
                    err(
                        "ADP-IO",
                        format!("edge.wasm missing at {}: {e}", src.display()),
                    )
                })?;
                written.push(write_bytes(out_dir.join("edge.wasm"), &bytes)?);
            }
            written.push(write_deploy(out_dir, target, build_id, "worker.js")?);
        }
        Target::Lambda | Target::Vercel | Target::Netlify => {
            written.push(write_file(
                out_dir.join("handler.mjs"),
                &serverless_handler(build_id),
            )?);
            if target == Target::Vercel {
                written.push(write_json(
                    out_dir.join("vercel.json"),
                    &serde_json::json!({ "version": 2, "entrypoint": "handler.mjs" }),
                )?);
            }
            if target == Target::Netlify {
                written.push(write_file(
                    out_dir.join("netlify.toml"),
                    "# Oxide Netlify target (§18). Generated by oxide-adapters.\n\
                     [functions]\n  node_bundler = \"esbuild\"\n\n\
                     [[redirects]]\n  from = \"/*\"\n\
                     to = \"/.netlify/functions/handler\"\n  status = 200\n",
                )?);
            }
            written.push(write_deploy(out_dir, target, build_id, "handler.mjs")?);
        }
        Target::Deno | Target::Bun => {
            written.push(write_file(
                out_dir.join("server.ts"),
                &runtime_server_ts(target, build_id),
            )?);
            written.push(write_deploy(out_dir, target, build_id, "server.ts")?);
        }
    }
    debug_assert!(
        written.iter().all(|p| p.exists()),
        "every written file exists"
    );
    Ok(written)
}

/// §18 refusal rule: the server manifest may declare hard requirements
/// (`"requires": {"sockets": true}`); a target where the matching capability is
/// `Unsupported` fails the build instead of failing at runtime.
fn check_requirements(target: Target, artifact: &BuildArtifact) -> Result<()> {
    let Some(requires) = artifact
        .server_manifest
        .get("requires")
        .and_then(|r| r.as_object())
    else {
        return Ok(());
    };
    for (feature, wanted) in requires {
        if wanted.as_bool() != Some(true) {
            continue;
        }
        let cap = match feature.as_str() {
            "l0Cache" | "l0_cache" => Some(Capability::L0Cache),
            "isr" => Some(Capability::Isr),
            "sockets" => Some(Capability::Sockets),
            "imageOptimization" | "image_optimization" => Some(Capability::ImageOptimization),
            "nativeRules" | "native_rules" => Some(Capability::NativeRules),
            "snapshots" => Some(Capability::Snapshots),
            "tls" => Some(Capability::Tls),
            _ => None,
        };
        let unsupported = cap
            .and_then(|c| capability_matrix(target).get(&c).copied())
            .is_some_and(|s| s == FeatureSupport::Unsupported);
        if unsupported {
            return Err(err(
                "ADP-CAP",
                format!(
                    "target `{}` does not support `{feature}`; remove the requirement or \
                     pick another target (§18 refusal rule)",
                    target.as_str()
                ),
            ));
        }
    }
    Ok(())
}

/// The `routes` object of a server-manifest JSON value, if present.
fn route_table(
    server_manifest: &serde_json::Value,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    server_manifest
        .get("routes")
        .and_then(|r| r.as_object())
        .cloned()
}

/// Default security headers for the static export (§17.1 subset, §10.5 `_headers`).
fn default_security_headers() -> BTreeMap<String, String> {
    [
        ("X-Frame-Options", "DENY"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "strict-origin-when-cross-origin"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// Copy the bundler manifests into the deploy dir when the artifact carries them.
fn copy_manifests(artifact: &BuildArtifact, out_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for (name, value) in [
        ("client.manifest.json", &artifact.client_manifest),
        ("server.manifest.json", &artifact.server_manifest),
    ] {
        if value.is_object() {
            out.push(write_json(out_dir.join(name), value)?);
        }
    }
    Ok(out)
}

fn write_file(path: PathBuf, content: &str) -> Result<PathBuf> {
    write_bytes(path, content.as_bytes())
}

fn write_json(path: PathBuf, value: &serde_json::Value) -> Result<PathBuf> {
    let s = serde_json::to_string_pretty(value).map_err(|e| {
        err(
            "ADP-JSON",
            format!("serialize {name}: {e}", name = path.display()),
        )
    })?;
    write_file(path, &format!("{s}\n"))
}

fn write_bytes(path: PathBuf, bytes: &[u8]) -> Result<PathBuf> {
    debug_assert!(
        !bytes.is_empty(),
        "emitted files are non-empty ({})",
        path.display()
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| err("ADP-IO", format!("cannot create {}: {e}", parent.display())))?;
    }
    std::fs::write(&path, bytes)
        .map_err(|e| err("ADP-IO", format!("cannot write {}: {e}", path.display())))?;
    Ok(path)
}

fn write_deploy(out_dir: &Path, target: Target, build_id: &str, entry: &str) -> Result<PathBuf> {
    write_json(
        out_dir.join("deploy.json"),
        &serde_json::json!({
            "target": target.as_str(),
            "build_id": build_id,
            "entry": entry,
        }),
    )
}

/// Serverless wrapper: web-standard `Request → Response` calling the server bundle
/// export (§18 "Serverless: Request/Response handler").
fn serverless_handler(build_id: &str) -> String {
    format!(
        "// Oxide serverless handler (\u{a7}18). build_id: {build_id}\n\
         // Web-standard Request \u{2192} Response adapter; the platform drives this export.\n\
         import handler from \"./server/index.js\";\n\
         \n\
         export default async function handlerWrapper(request) {{\n\
           const response = await handler(request);\n\
           return response ?? new Response(\"Not Found\", {{ status: 404 }});\n\
         }}\n\
         \n\
         export {{ handlerWrapper as fetch }};\n"
    )
}

/// Deno / Bun thin wrapper (same handler contract, runtime-specific serve call).
fn runtime_server_ts(target: Target, build_id: &str) -> String {
    let name = target.as_str();
    match target {
        Target::Deno => format!(
            "// Oxide {name} entry (\u{a7}18). build_id: {build_id}\n\
             import handler from \"./server/index.js\";\n\
             \n\
             Deno.serve({{ port: Number(Deno.env.get(\"PORT\") ?? 3000), fetch: handler }});\n"
        ),
        _ => format!(
            "// Oxide {name} entry (\u{a7}18). build_id: {build_id}\n\
             import handler from \"./server/index.js\";\n\
             \n\
             Bun.serve({{ port: Number(process.env.PORT ?? 3000), fetch: handler }});\n"
        ),
    }
}

/// `_headers` emitter (static hosts).
///
/// Each rule is one section: the path pattern on its own line followed by two-space
/// indented `Header: value` lines (header order = `BTreeMap` order, deterministic);
/// sections are separated by a blank line.
pub fn emit_headers(rules: &[(String, BTreeMap<String, String>)]) -> String {
    let mut sections: Vec<String> = Vec::new();
    for (path, headers) in rules {
        let mut section = format!("{path}\n");
        for (name, value) in headers {
            section.push_str(&format!("  {name}: {value}\n"));
        }
        sections.push(section);
    }
    sections.join("\n")
}

/// `_redirects` emitter (static hosts): one `/from /to STATUS` line per rule;
/// status `0` means "platform default" and is emitted as `301`.
pub fn emit_redirects(rules: &[(String, String, u16)]) -> String {
    let mut out = String::new();
    for (from, to, status) in rules {
        let status = if *status == 0 { 301 } else { *status };
        out.push_str(&format!("{from} {to} {status}\n"));
    }
    out
}

/// Dockerfile for the `docker` target: distroless around `binary`, healthcheck,
/// non-root, reproducible layers (§18).
///
/// `embed = true` bakes the client asset dist into the image (`ARG` + `COPY`),
/// mirroring `adapter: 'binary', embed: true` (§18). The healthcheck invokes the
/// server binary's `health` subcommand — distroless images ship no shell/curl.
pub fn dockerfile(embed: bool) -> String {
    let embed_block = if embed {
        "\n# Assets baked into the image (embed: true, \u{a7}18).\nARG OXIDE_EMBED=1\nCOPY --from=build /src/dist /app/dist\nENV OXIDE_EMBEDDED_ASSETS=/app/dist\n"
    } else {
        ""
    };
    format!(
        "# syntax=docker/dockerfile:1\n\
         # Oxide docker target (\u{a7}18) — distroless, non-root, healthcheck,\n\
         # reproducible layers. Generated by oxide-adapters; do not edit.\n\n\
         FROM rust:1 AS build\n\
         WORKDIR /src\n\
         COPY . .\n\
         RUN cargo build --release -p oxide-server\n\
         RUN strip target/release/server 2>/dev/null || true\n\n\
         FROM gcr.io/distroless/cc-debian12\n\
         WORKDIR /app\n\
         COPY --from=build /src/target/release/server /app/server\n\
         {embed_block}\
         USER nonroot:nonroot\n\
         ENV PORT=3000\n\
         EXPOSE 3000\n\
         HEALTHCHECK --interval=30s --timeout=3s --start-period=5s CMD [\"/app/server\", \"health\"]\n\
         ENTRYPOINT [\"/app/server\"]\n"
    )
}

/// Cloudflare Workers entry: WinterCG-compatible JS consuming the edge build (D18).
/// Deterministic; interpolates the `build_id` into a provenance comment.
pub fn cloudflare_worker_entry(build_id: &str) -> String {
    format!(
        "// Oxide Cloudflare Workers entry (\u{a7}18, \u{a7}2 D18). build_id: {build_id}\n\
         // WinterCG-compatible: `edge.js` dispatches through the web-standard\n\
         // handler contract `(Request) => Promise<Response>`.\n\
         import handler from \"./edge.js\";\n\
         \n\
         export default {{\n\
           async fetch(request, env, ctx) {{\n\
             const response = await handler(request, env, ctx);\n\
             return response ?? new Response(\"Not Found\", {{ status: 404 }});\n\
           }},\n\
         }};\n"
    )
}


/// Edge handler (`edge.js`) for the Cloudflare target (§18, §2 D18).
///
/// WinterCG-compatible ESM: exports `default(req, env, ctx) => Response`.
/// Loads `./edge.wasm` when present; that module provides the accelerated
/// `edge_cache_key`, `merge_head` and `encode_odf` functions compiled from
/// `oxide-core-wasm`. When the wasm module is absent, the pure-JS fallbacks
/// below produce identical output, so the Worker is correct either way.
///
/// Edge cache keys are the **raw composite string** (`route\0params\0vary\0build_id`),
/// not a hash: KV/R2 accept arbitrary-length keys up to 512 bytes, and this
/// keeps the edge key space explicitly distinct from the binary server's
/// blake3-hashed L0 keys (which would otherwise collide).
pub fn edge_handler_js(build_id: &str) -> String {
    // Format! with escaped braces is painful; use a raw string + replace.
    let tpl = r#"// Oxide edge handler (spec §18, §2 D18). build_id: __BUILD_ID__
// WinterCG-compatible: default export is (req, env, ctx) => Promise<Response>.
// Loads ./edge.wasm when present; pure-JS fallbacks below are semantically
// identical to the WASM path (spec D18).

let wasm = null;
try {
  // Cloudflare binds wasm as an ESM module when `wrangler.toml` declares it.
  const mod = await import("./edge.wasm");
  wasm = mod.default ?? mod;
} catch {
  // No wasm: JS fallbacks cover the same three functions.
  wasm = null;
}

// ---- edge cache key ---------------------------------------------------------
// The edge key is the raw composite string, NOT a hash. KV keys allow up to
// 512 bytes; a hash would only save bytes we do not need. It also keeps edge
// keys explicitly out of the binary-server's blake3 keyspace (which would
// otherwise risk accidental collisions across processes).
function cacheKey(route, params, vary, build_id) {
  return ["edge", route, params, vary, build_id].join("\u0000");
}

// ---- head merge -------------------------------------------------------------
// Merge two head-tag models: layout (parent) then page (child). Dedupe by
// (kind, key) with page winning. Matches `oxide_core_wasm::merge_head`.
function mergeHead(a, b) {
  const out = { title: null, meta: [], links: [], json_ld: [] };
  for (const src of [a, b]) {
    if (!src) continue;
    if (src.title) out.title = src.title;
    if (src.meta) out.meta.push(...src.meta);
    if (src.links) out.links.push(...src.links);
    if (src.json_ld) out.json_ld.push(...src.json_ld);
  }
  out.meta = dedupe(out.meta, (t) => t[0]);
  out.links = dedupe(out.links, (t) => t[0] + ":" + JSON.stringify(t[1]));
  return out;
}
function dedupe(arr, keyFn) {
  const seen = new Map();
  for (const t of arr) seen.set(keyFn(t), t);
  return Array.from(seen.values());
}

// ---- ODF encoder ------------------------------------------------------------
// Minimal devalue-compatible JSON subset: primitives, arrays, objects. Maps,
// Sets, BigInt, Date and cyclic refs are server-side only; the client router
// uses the ~1 KB decoder in packages/oxide.
function encodeOdf(value) {
  const bytes = new TextEncoder().encode(JSON.stringify(value));
  return bytes;
}

// ---- request handling -------------------------------------------------------
async function handler(request, env, ctx) {
  const url = new URL(request.url);
  // Health endpoint (§12.2) — Rust answers in the binary target; here it is
  // trivial enough to answer from the Worker directly.
  if (request.method === "GET" && url.pathname === "/_oxide/health") {
    return new Response("ok", {
      status: 200,
      headers: { "content-type": "text/plain; charset=utf-8" },
    });
  }
  // Route dispatch: this default Worker handles everything not excluded by
  // `_routes.json` (assets bypass it at the CDN). The user's edge bundle
  // (`edge_bundle.js`) is dropped in by the CLI at build time when present.
  try {
    const user = await import("./edge_bundle.js");
    const r = await user.default(request, env, ctx);
    if (r) return r;
  } catch (e) {
    if (!/Cannot find module|Module not found/.test(String(e))) {
      return new Response("edge handler error: " + (e?.message ?? e), {
        status: 500,
        headers: { "content-type": "text/plain; charset=utf-8" },
      });
    }
    // No user bundle — fall through to 404.
  }
  return new Response("Not Found", { status: 404 });
}

export default { fetch: handler };
export { cacheKey, mergeHead, encodeOdf };
"#;
    tpl.replace("__BUILD_ID__", build_id)
}

/// `wrangler.toml` for the Cloudflare target. Declares the wasm module
/// binding when `edge.wasm` is present.
pub fn cloudflare_wrangler_toml(build_id: &str, has_wasm: bool) -> String {
    let mut s = String::new();
    s.push_str("# Oxide Cloudflare Workers target (§18). Generated by oxide-adapters.\n");
    s.push_str(&format!("name = \"oxide-{build_id}\"\n"));
    s.push_str("main = \"worker.js\"\n");
    s.push_str("compatibility_date = \"2024-09-23\"\n");
    s.push_str("compatibility_flags = [\"nodejs_compat\"]\n");
    if has_wasm {
        s.push_str("\n[wasm_modules]\n");
        s.push_str("edge = \"./edge.wasm\"\n");
    }
    s.push_str("\n# Static assets bypass the Worker via _routes.json and are served\n");
    s.push_str("# directly by the Cloudflare CDN. Upload ./assets/ to your bucket.\n");
    s
}

/// Node server entry for the `node` target (uses the NodeHost backend, §18).
///
/// Dependency-free bridge: imports the server bundle export (`./server/index.js`,
/// which must export the web-standard handler) and serves it over `node:http`,
/// converting Node req/res to `Request`/`Response`.
pub fn node_server_entry(build_id: &str) -> String {
    format!(
        "// Oxide node target entry (\u{a7}18). build_id: {build_id}\n\
         // Bridges Node's http server to the web-standard handler contract\n\
         // `(Request) => Promise<Response>` exported by the server bundle.\n\
         import http from \"node:http\";\n\
         import handler from \"./server/index.js\";\n\
         \n\
         const port = Number(process.env.PORT ?? 3000);\n\
         \n\
         const server = http.createServer(async (req, res) => {{\n\
           const url = new URL(req.url ?? \"/\", `http://${{req.headers.host ?? \"localhost\"}}`);\n\
           const headers = new Headers();\n\
           for (let i = 0; i < req.rawHeaders.length; i += 2) {{\n\
             headers.append(req.rawHeaders[i], req.rawHeaders[i + 1]);\n\
           }}\n\
           const method = req.method ?? \"GET\";\n\
           const hasBody = method !== \"GET\" && method !== \"HEAD\";\n\
           const request = new Request(url, {{\n\
             method,\n\
             headers,\n\
             body: hasBody ? req : undefined,\n\
             ...(hasBody ? {{ duplex: \"half\" }} : {{}}),\n\
           }});\n\
           const response = await handler(request);\n\
           res.statusCode = response.status;\n\
           response.headers.forEach((value, key) => res.setHeader(key, value));\n\
           if (response.body) {{\n\
             for await (const chunk of response.body) res.write(chunk);\n\
           }}\n\
           res.end();\n\
         }});\n\
         \n\
         server.listen(port);\n"
    )
}

#[cfg(test)]
mod tests;
