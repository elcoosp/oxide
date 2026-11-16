//! Moved verbatim from build/cmd.rs by scripts/build_phase_split.py (V2-A01 phase 2).
//!
//! phase_bundle_server — the SSR env bundle + the manifest reads
//! (spec WP-A01 phase 2). Bundles the server env, enriches both manifests
//! with layout chains, persists diagnostics, reads client/server manifests
//! and derives the boot script URL/integrity, the manifest integrity map,
//! the E04 head producer and the canonical build id.

use crate::*;
use oxide_fw_bundler::Bundler as _;

// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
pub(crate) async fn phase_bundle_server(cx: &mut BuildCtx<'_>) -> anyhow::Result<()> {
    let rootp = &cx.rootp;
    let cfg = &cx.cfg;
    let tree = &cx.tree;
    let client_out = cx.client_out.as_ref().expect("phase_plan sets client_out");
    let server_out = cx.server_out.as_ref().expect("phase_plan sets server_out");
    let server_opts = cx
        .server_opts
        .as_ref()
        .expect("phase_plan sets server_opts");
    let rb = cx.rb.as_ref().expect("phase_plan sets rb");
    let client_outcome = cx
        .client_outcome
        .as_ref()
        .expect("phase_bundle_client sets client_outcome");
    let boot_chunk_url = &cx.boot_chunk_url;
    // <<< phase-wrapper
    let server_outcome = rb.bundle(server_opts).map_err(|e| {
        let mut diags = client_outcome.diagnostics.clone();
        diags.push((*e.0).clone());
        persist_diagnostics(rootp, &diags);
        eprintln!("bundler (server) failed — full chain: {e:#}");
        anyhow::anyhow!("{} {}", e.0.code, e.0.message)
    })?;

    // §8.2 / §16.3 / O1: enrich both manifests with layout chains from the
    // router tree. The bundler doesn't know the tree, so its layout_chain
    // is []; the CLI fills it in here. Best-effort — a missing manifest
    // (an older build, or a non-SSR env) is a no-op.
    let _ = enrich_manifest_layout_chains(&client_out.join("client.manifest.json"), tree);
    let _ = enrich_manifest_layout_chains(&server_out.join("server.manifest.json"), tree);
    // BUG-06: persist build diagnostics so MCP `oxide_errors` has data.
    // Written on success too (possibly `[]`) so stale errors don't linger.
    {
        let mut all = client_outcome.diagnostics.clone();
        all.extend(server_outcome.diagnostics.clone());
        persist_diagnostics(rootp, &all);
    }
    // The bundler writes client.manifest.json / server.manifest.json into the out dirs.
    let client_manifest = read_json(&client_out.join("client.manifest.json"))
        .ok_or_else(|| anyhow::anyhow!("bundler did not write client.manifest.json"))?;
    let client_manifest_snapshot = client_manifest.clone();
    // §10.3: the boot chunk URL, if islands were registered. `RolldownBundler`
    // emits `client.manifest.json.routes["__boot__"].entry_chunk` as the
    // hashed file name; the shell references `/client/<file>`.
    let boot_script_url: Option<String> = if boot_chunk_url.is_some() {
        client_manifest
            .get("routes")
            .and_then(|r| r.get("__boot__"))
            .and_then(|r| r.get("entry_chunk"))
            .and_then(|v| v.as_str())
            .map(|file| format!("client/{file}"))
    } else {
        None
    };
    // V2-E04: the manifest's integrity digest for the boot chunk, stamped
    // onto the boot `<script type=module>` tag when `security.sri` is on
    // (the default). A manifest written by a pre-E04 bundler carries no
    // `integrity` map — the lookup returns `None` and the tag stays
    // byte-identical, so an incremental rebuild cannot emit a digest that
    // does not match the chunk it names.
    let boot_script_integrity: Option<String> = if cfg.security.sri {
        boot_script_url
            .as_deref()
            .and_then(|url| manifest_integrity_for(client_manifest.get("integrity"), url))
    } else {
        None
    };
    // V2-E04: the manifest's whole integrity map for the Injector (the
    // prerendered shells attach it; a modulepreload/stylesheet link whose
    // URL resolves to a digest emits `integrity` + `crossorigin`). Gated on
    // `security.sri` like the boot tag — off keeps every link byte-identical.
    let manifest_integrity: std::collections::BTreeMap<String, String> = if cfg.security.sri {
        client_manifest
            .get("integrity")
            .and_then(|v| v.as_object())
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        std::collections::BTreeMap::new()
    };
    // V2-E04 tail: the adapter-level head producer — the manifest-driven
    // projection of each route's modulepreload/stylesheet URLs (the seam
    // the E04 record left wired-and-ready). The typed parse runs beside
    // the JSON `Value` above (which stays authoritative for the locale
    // stamp and the boot lookups); a manifest that fails the typed parse
    // degrades to no producer — no links, the pre-milestone bytes — with
    // a loud warning, never a build failure.
    let head_producer: Option<oxide_fw_bundler::head::ManifestHeadProducer> =
        match serde_json::from_value::<oxide_fw_bundler::ClientManifest>(client_manifest.clone()) {
            Ok(typed) => Some(oxide_fw_bundler::head::ManifestHeadProducer::new(typed)),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "client.manifest.json does not type-parse; prerender shells carry no manifest head links"
                );
                None
            }
        };
    let server_manifest = read_json(&server_out.join("server.manifest.json"))
        .ok_or_else(|| anyhow::anyhow!("bundler did not write server.manifest.json"))?;
    let build_id = client_manifest
        .get("build_id")
        .and_then(|v| v.as_str())
        .unwrap_or("dev")
        .to_string();

    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
    cx.client_manifest = Some(client_manifest);
    cx.client_manifest_snapshot = Some(client_manifest_snapshot);
    cx.boot_script_url = boot_script_url;
    cx.boot_script_integrity = boot_script_integrity;
    cx.manifest_integrity = Some(manifest_integrity);
    cx.head_producer = head_producer;
    cx.server_manifest = Some(server_manifest);
    cx.build_id = Some(build_id);
    Ok(())
    // <<< phase-wrapper
}
