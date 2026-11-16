//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// `oxide render` (§7): load the framework adapter and dispatch a page
/// render through it. Writes the resulting HTML to stdout. This is the
/// read-path proof: config → adapter → render → HTML.
pub(crate) async fn cmd_render(
    root: &str,
    route: &str,
    url: Option<&str>,
    params: &[String],
    data_json: &str,
    stream: bool,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    use oxide_fw_jshost::{FrameworkHost, JsHost, LoaderHost, NodeHost, PoolConfig};

    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    // §16.5 / WP-r9-1: OXIDE_ROOT is written PRE-RUNTIME by
    // `apply_startup_env` (P1 readiness env-race fix); nothing to do here.
    let config_path = find_config_file(&rootp)?;
    // Load the config to find `routes.dir` for the loader lookup. Non-strict:
    // a config that the Rust extractor cannot fully parse (e.g. a
    // `createRealAdapter(...)` factory call with a callback arg) still
    // renders fine because the worker re-loads the file fresh. Falling back
    // to defaults here just means the loader path is `<root>/src/routes`.
    let cfg = load_config_or_default(&rootp, "production");

    // Parse params (`k=v` pairs; comma splits produce arrays).
    let mut params_map: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for raw in params {
        let Some((k, v)) = raw.split_once('=') else {
            anyhow::bail!("--param expects KEY=VALUE, got `{raw}`");
        };
        let entry = params_map
            .entry(k.to_string())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        match entry {
            serde_json::Value::Array(a) => a.push(serde_json::Value::String(v.to_string())),
            _ => unreachable!("entry initialized as array"),
        }
    }
    let params_json: serde_json::Map<String, serde_json::Value> = params_map
        .into_iter()
        .map(|(k, v)| {
            let scalar = match v {
                serde_json::Value::Array(ref a) if a.len() == 1 => a[0].clone(),
                other => other,
            };
            (k, scalar)
        })
        .collect();

    let data: serde_json::Value = serde_json::from_str(data_json)
        .map_err(|e| anyhow::anyhow!("--data is not valid JSON: {e}"))?;

    let url_str = url.map(String::from).unwrap_or_else(|| {
        let stripped = route.strip_prefix("routes/").unwrap_or(route);
        format!("/{stripped}")
    });

    // One worker pool backs both the loader and the renderer.
    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool = host
        .tooling_pool(PoolConfig::default())
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    // §9.1: run the route's `page.server.ts` loader (if present) so the
    // CLI one-shot renders the same output as dev/serve. `--data` is the
    // fallback; a loader that returns non-200 short-circuits with a
    // diagnostic.
    let mut effective_data = data.clone();
    let route_dir = route.strip_prefix("routes/").unwrap_or(route);
    let page_server_path = rootp
        .join(&cfg.routes.dir)
        .join(route_dir)
        .join("page.server.ts");
    if page_server_path.is_file() {
        let loader_host = LoaderHost::new(std::sync::Arc::clone(&pool));
        let params_for_loader: serde_json::Value = serde_json::Value::Object(params_json.clone());
        match loader_host
            .run(
                &page_server_path.to_string_lossy(),
                serde_json::json!({
                    "route": route,
                    "params": params_for_loader,
                    "url": url_str,
                    "method": "GET",
                    "headers": [],
                }),
            )
            .await
        {
            Ok(out) => {
                let status = out.get("status").and_then(|v| v.as_u64()).unwrap_or(200) as u16;
                if status != 200 {
                    let loc = out.get("location").and_then(|v| v.as_str()).unwrap_or("");
                    anyhow::bail!(
                        "loader short-circuited with status {status}{}",
                        if loc.is_empty() {
                            String::new()
                        } else {
                            format!(" -> {loc}")
                        }
                    );
                }
                if let Some(d) = out.get("data") {
                    effective_data = d.clone();
                }
            }
            Err(e) => {
                anyhow::bail!("loader failed: {} {}", e.0.code, e.0.message);
            }
        }
    }

    let mut input = serde_json::json!({
        "route": route,
        "url": url_str,
        "params": params_json,
        "data": effective_data,
        "head": { "meta": [], "link": [], "style": [], "script": [] },
    });
    // §6.4 / P0-10: `oxide render` must attach the same ComponentSource
    // the serve path builds, or a real-SSR config renders through the
    // worker's fallback path (which has no config-side component
    // resolution anymore) and 500s on every page. The entry is resolved
    // from the manifest by the same helper the renderer uses; the
    // filesystem probe is the fallback for a project that has not built
    // yet (in which case the worker's own IR path takes over and no
    // component is needed).
    if let Some(entry) = crate::framework_shim::resolve_ssr_entry(&rootp.join("dist/server")) {
        input["components"] = serde_json::json!({ "entry": entry });
    }

    let fw = FrameworkHost::new(pool);
    let path_str = config_path.to_string_lossy().to_string();

    if stream {
        // Exercise the streaming trait: pull chunks as they arrive and
        // write each one to the writer immediately. Output bytes are
        // identical to the non-streaming path; the difference is that a
        // slow upstream would start producing output before the whole
        // payload is ready.
        //
        // FrameworkHost::render_streaming returns a CallStream — an enum
        // over either one Bytes (Complete) or an mpsc Receiver<Result<Bytes>>
        // (Chunked). Iterate the two variants directly rather than wrapping
        // it as a Stream.
        let mut s = fw
            .render_streaming(&path_str, input)
            .await
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        match &mut s {
            oxide_fw_jshost::CallStream::Complete(bytes) => {
                w.write_all(bytes)
                    .map_err(|e| anyhow::anyhow!("write stdout: {e}"))?;
                w.flush()
                    .map_err(|e| anyhow::anyhow!("flush stdout: {e}"))?;
            }
            oxide_fw_jshost::CallStream::Chunked(rx) => {
                while let Some(item) = rx.recv().await {
                    let bytes =
                        item.map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
                    w.write_all(&bytes)
                        .map_err(|e| anyhow::anyhow!("write stdout: {e}"))?;
                    w.flush()
                        .map_err(|e| anyhow::anyhow!("flush stdout: {e}"))?;
                }
            }
        }
        // Match the non-streaming path's trailing newline. `outln!` appends
        // `\n`; the stream path writes bytes directly, so add it here.
        w.write_all(b"\n")
            .map_err(|e| anyhow::anyhow!("write stdout: {e}"))?;
        w.flush()
            .map_err(|e| anyhow::anyhow!("flush stdout: {e}"))?;
    } else {
        let html = fw
            .render(&path_str, input)
            .await
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        outln!(w, "{html}");
    }
    Ok(0)
}
