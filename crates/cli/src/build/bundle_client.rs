//! Moved verbatim from build/cmd.rs by scripts/build_phase_split.py (V2-A01 phase 2).
//!
//! phase_bundle_client — the client env bundle + its gates
//! (spec WP-A01 phase 2). Runs the incremental client bundle (§5-A env
//! fingerprint skip) and the post-bundle gates: the V8 `node:` requirement
//! scan (JSH-V8KIND), the forbidden server-export guard (CMP-GUARD) and
//! the client secret scan (CMP-SECRET), then records the env fingerprint.

use crate::*;
use oxide_fw_bundler::Bundler as _;

// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
pub(crate) async fn phase_bundle_client(cx: &mut BuildCtx<'_>) -> anyhow::Result<()> {
    let rootp = &cx.rootp;
    let cfg = &cx.cfg;
    let w = &mut *cx.w;
    let client_out = cx.client_out.as_ref().expect("phase_plan sets client_out");
    let client_opts = cx
        .client_opts
        .as_ref()
        .expect("phase_plan sets client_opts");
    let rb = cx.rb.as_ref().expect("phase_plan sets rb");
    let build_secret_for_fp = cx
        .build_secret_for_fp
        .as_ref()
        .expect("phase_plan sets build_secret_for_fp");
    // <<< phase-wrapper
    // §5-A: skip the client bundle when nothing it depends on changed. The
    // env fingerprint excludes `*.server.*` (never in the client bundle), so
    // a server-only edit leaves this env valid. The manifests are read from
    // disk below either way, so a skip is safe only when they exist.
    let client_fp = build_cache::env_fingerprint(
        rootp,
        "client",
        &client_opts
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>(),
        &[&client_out.to_string_lossy(), build_secret_for_fp.as_str()],
    );
    let client_manifest_on_disk = client_out.join("client.manifest.json");
    // `OXIDE_FORCE_BUNDLE` forces a re-bundle; `OXIDE_FORCE_BUILD` forces
    // the whole build (which includes every env). The two are separate
    // because "the whole-build fingerprint said nothing changed" and "this
    // env's bundle is still valid" are different questions — a developer
    // forcing a rebuild to re-run a later phase should not also pay for
    // re-bundling an unchanged env.
    let force_bundle = std::env::var_os("OXIDE_FORCE_BUNDLE").is_some();
    let skip_client = client_manifest_on_disk.is_file()
        && !force_bundle
        && build_cache::env_is_up_to_date(rootp, "client", &client_fp);
    let client_outcome = if skip_client {
        outln!(
            w,
            "bundle: client unchanged (env fingerprint match) — skipped"
        );
        oxide_fw_bundler::BundleOutput {
            modules: Vec::new(),
            chunks: Vec::new(),
            client_manifest: None,
            server_manifest: None,
            diagnostics: Vec::new(),
        }
    } else {
        rb.bundle(client_opts).map_err(|e| {
            persist_diagnostics(rootp, std::slice::from_ref(&e.0));
            // The Diagnostic message flattens the error chain; print the full
            // chain on stderr so a bundler failure is debuggable without
            // re-running under a debugger.
            eprintln!("bundler (client) failed — full chain: {e:#}");
            anyhow::anyhow!("{} {}", e.0.code, e.0.message)
        })?
    };
    // SPEC-02 / §4.6.3: a V8 build whose server modules need Node (a `.node`
    // addon or an unshimmed `node:` builtin) must fail loudly. The bundler
    // externalizes `node:` specifiers, so scanning the emitted JS misses them;
    // scan the source server modules instead.
    if matches!(cfg.server.backend, oxide_fw_config::JsBackend::V8) {
        let mut needs: Vec<String> = Vec::new();
        let mut stack = vec![rootp.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let n = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
                    if !matches!(n, "node_modules" | "dist" | ".oxide" | "target" | ".git") {
                        stack.push(p);
                    }
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !(name.ends_with(".ts") || name.ends_with(".tsx") || name.ends_with(".js")) {
                    continue;
                }
                let Ok(code) = std::fs::read_to_string(&p) else {
                    continue;
                };
                for quote in ['\'', '"'] {
                    let pat = format!("{quote}node:");
                    let mut rest = code.as_str();
                    while let Some(i) = rest.find(&pat) {
                        let after = &rest[i + pat.len()..];
                        let bname: String = after
                            .chars()
                            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                            .collect();
                        if let Some(reason) = oxide_fw_jshost::node_compat::requires_node_backend(
                            &format!("node:{bname}"),
                        ) {
                            needs.push(reason);
                        }
                        rest = &after[bname.len().min(after.len())..];
                    }
                }
                for ext in [".node\"", ".node'"] {
                    if code.contains(ext) {
                        needs.push("a .node native addon".to_string());
                    }
                }
            }
        }
        needs.sort();
        needs.dedup();
        if !needs.is_empty() {
            let msg = format!(
                "server.backend = v8, but the server code needs Node: {}. Set server.backend to node.",
                needs.join(", ")
            );
            outln!(w, "error JSH-V8KIND: {msg}");
            let d = oxide_fw_diagnostics::Diagnostic::error("JSH-V8KIND", msg);
            persist_diagnostics(rootp, &[d]);
            anyhow::bail!("V8 backend incompatible with the server code (JSH-V8KIND)");
        }
    }

    // SPEC-05 / §6.4: the forbidden-import guard. A `*.server.ts` module must
    // never ship its real body into the client bundle. A legitimate import
    // goes through the compiler's server-fn stub (a `serverFn(...)` export
    // compiles to a `POST /_oxide/fn/<id>` stub with none of the module's own
    // identifiers); a raw import leaks the body. So: collect every non-stub
    // export name from every server module under the routes root, and fail if
    // any appears in ANY emitted client chunk. This is metadata-independent —
    // it reads the actual emitted bytes.
    {
        let mut server_exports: Vec<(std::path::PathBuf, String)> = Vec::new();
        // Scan the WHOLE project, not just the routes dir: a server module
        // can live anywhere (`src/posts.server.ts`). Skip build/vendor dirs.
        let mut stack = vec![rootp.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let n = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
                    if !matches!(n, "node_modules" | "dist" | ".oxide" | "target" | ".git") {
                        stack.push(p);
                    }
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !(name.ends_with(".server.ts") || name.ends_with(".server.tsx")) {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                for line in src.lines() {
                    let t = line.trim();
                    // Only a NON-FUNCTION `export const` is a leak risk — that
                    // is a literal value (a secret, a base URL, a table). A
                    // function export (`load`, `getPost`, a `serverFn`) never
                    // ships its body to the client: loaders run server-side,
                    // serverFns compile to stubs, and the compiler's cache
                    // wrapper keeps the function on the server. Flagging a
                    // generic name like `load` would false-positive on every
                    // chunk (it is a common word).
                    if let Some(rest) = t.strip_prefix("export const ") {
                        if t.contains("serverFn(") {
                            continue; // compiles to a stub — legitimate
                        }
                        let ident: String = rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        // Build-time metadata the compiler consumes and never
                        // ships as a body.
                        if matches!(
                            ident.as_str(),
                            "config" | "prerender" | "generateParams" | "load"
                        ) {
                            continue;
                        }
                        // A function-ish RHS (`= async`, `= (`, `= function`,
                        // `=>` on the same line) is not a literal export.
                        let after = rest[ident.len()..].trim_start();
                        let is_fn = after.starts_with("= async")
                            || after.starts_with("= (")
                            || after.starts_with("= function")
                            || after.starts_with("=>")
                            || after.contains("=> {");
                        if !is_fn && ident.len() >= 3 {
                            // The emitted chunk may inline the value, so look
                            // for any string literal in the RHS (a base URL, a
                            // secret). The bare identifier is NOT pushed: an
                            // object/array export (`actions`, `config`, a route
                            // table) has a common name that appears in every
                            // chunk — only its literal *contents* leak.
                            let rhs = after.split_once('=').map(|x| x.1).unwrap_or("");
                            let mut in_str = false;
                            let mut lit = String::new();
                            let mut quote = '\0';
                            for ch in rhs.chars() {
                                if !in_str && (ch == '\'' || ch == '"' || ch == '`') {
                                    in_str = true;
                                    quote = ch;
                                } else if in_str && ch == quote {
                                    if lit.len() >= 8 {
                                        server_exports.push((p.clone(), lit.clone()));
                                    }
                                    in_str = false;
                                    lit.clear();
                                } else if in_str {
                                    lit.push(ch);
                                }
                            }
                        }
                    }
                }
            }
        }
        // Read every emitted client .js once.
        let mut client_code: Vec<(String, String)> = Vec::new();
        let mut stack = vec![client_out.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("js") {
                    continue;
                }
                if let Ok(code) = std::fs::read_to_string(&p) {
                    client_code.push((p.to_string_lossy().to_string(), code));
                }
            }
        }
        let mut hits: Vec<oxide_fw_diagnostics::Diagnostic> = Vec::new();
        for (sp, ident) in &server_exports {
            for (chunk, code) in &client_code {
                if code.contains(ident.as_str()) {
                    // 19-E: locate the literal in its source file so the
                    // diagnostic can show the offending line. `ident` is the
                    // string literal the guard found (>= 8 chars), so a
                    // substring search for it in the quoted source is exact
                    // enough to name the line.
                    let mut diag = oxide_fw_diagnostics::Diagnostic::error(
                        "CMP-GUARD",
                        format!(
                            "server-only `{ident}` from `{}` shipped into client chunk `{chunk}`",
                            sp.display()
                        ),
                    )
                    .with_fix(
                        "import it via a server function (9.4) or move the value \
                             into a non-server module",
                    );
                    if let Ok(src) = std::fs::read_to_string(sp)
                        && let Some((line_no, col)) = find_literal_line(&src, ident)
                    {
                        diag = diag.at(oxide_fw_diagnostics::Location::file(
                            sp.to_string_lossy(),
                            line_no as u32,
                            col as u32,
                        ));
                        let frame =
                            oxide_fw_diagnostics::render_code_frame(&src, line_no, Some(col));
                        if !frame.is_empty() {
                            diag = diag.with_code_frame(frame);
                        }
                    }
                    hits.push(diag);
                    break;
                }
            }
        }
        if !hits.is_empty() {
            for d in &hits {
                outln!(w, "error {}: {}", d.code, d.message);
            }
            persist_diagnostics(rootp, &hits);
            anyhow::bail!(
                "{} server-only export(s) reached the client bundle (CMP-GUARD)",
                hits.len()
            );
        }
    }

    // SEC-08: fail the build if a secret-looking `process.env.*` read shipped
    // into the client bundle.
    let secret_hits = oxide_fw_bundler::scan_client_secrets(&client_outcome.chunks);
    if !secret_hits.is_empty() {
        for d in &secret_hits {
            outln!(w, "error {}: {}", d.code, d.message);
        }
        persist_diagnostics(rootp, &secret_hits);
        anyhow::bail!(
            "{} client secret(s) would leak to the browser (CMP-SECRET)",
            secret_hits.len()
        );
    }
    // Only record the client env fingerprint AFTER the client bundle passed
    // every post-bundle gate (secret scan included). Storing it before the
    // scan let a failed `CMP-SECRET` build look "unchanged" on the next run:
    // `skip_client` was true, the scan saw no chunks, and the build went
    // green without ever re-checking the leaking bundle.
    if !skip_client {
        if let Err(e) = build_cache::env_store(rootp, "client", &client_fp) {
            eprintln!("warn: could not store client env fingerprint: {e}");
        }
        // V2-F17 (the dev feed): persist the cost records beside the client
        // manifest — the file the serve boot's `FileCostsSource` re-reads per
        // stat move, so `GET /__oxide/cost?src=` answers the CURRENT build
        // without re-attaching. A write failure warns and the build
        // continues: the file is dev-plane telemetry, the endpoint answers
        // its documented 404s without it. On a fingerprint-match skip the
        // previous file stays (the bundle is byte-identical, so are its
        // records).
        let records = oxide_fw_bundler::cost::source_costs(&client_outcome, rootp);
        if let Err(e) = oxide_fw_bundler::cost::write_source_costs(
            &records,
            &client_out.join("client.costs.json"),
        ) {
            eprintln!("warn: could not write client cost records: {e}");
        }
    }
    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
    cx.client_outcome = Some(client_outcome);
    Ok(())
    // <<< phase-wrapper
}
