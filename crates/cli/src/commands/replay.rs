//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// V2-F15: the replay executor the [`ReplayRunner`] drives — one HTTP
/// request against the replay server's loopback address, built from the
/// record's request line-set. The worker pool the stack spawned runs with
/// `OXIDE_REPLAY=<record>` (set before the stack builds), so the loader
/// code's outbound `fetch` calls and non-deterministic sources play back
/// from the record instead of the network.
pub(crate) struct ReplayHttpExecutor {
    addr: std::net::SocketAddr,
}

#[async_trait::async_trait]
impl oxide_fw_dispatch::replay::ReplayExecutor for ReplayHttpExecutor {
    async fn execute(
        &self,
        record: &oxide_fw_dispatch::record::RequestRecord,
    ) -> Result<oxide_fw_dispatch::replay::ExecutedResponse, oxide_fw_diagnostics::OxideError> {
        use oxide_fw_diagnostics::Diagnostic;
        let client = reqwest::Client::new();
        let url = format!("http://{}{}", self.addr, record.request.url);
        let method =
            reqwest::Method::from_bytes(record.request.method.to_ascii_uppercase().as_bytes())
                .map_err(|e| {
                    OxideError(Box::new(Diagnostic::error(
                        "RPL-001",
                        format!(
                            "the record's method `{}` is not an HTTP method: {e}",
                            record.request.method
                        ),
                    )))
                })?;
        // The transport headers are per-connection state, not recorded
        // request identity — the client mints its own.
        const SKIP_REQUEST_HEADERS: [&str; 6] = [
            "host",
            "content-length",
            "connection",
            "transfer-encoding",
            "keep-alive",
            "upgrade",
        ];
        let mut req = client.request(method, &url);
        for (k, v) in &record.request.headers {
            if SKIP_REQUEST_HEADERS
                .iter()
                .any(|s| s.eq_ignore_ascii_case(k))
            {
                continue;
            }
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().await.map_err(|e| {
            OxideError(Box::new(
                Diagnostic::error(
                    "RPL-001",
                    format!("the replay request to {url} failed: {e}"),
                )
                .with_fix("is the app the record was made from still buildable?"),
            ))
        })?;
        // The transport-added headers are per-response state the diff
        // already treats as re-derived or would misread (a reqwest body
        // length is not the app's answer).
        const SKIP_RESPONSE_HEADERS: [&str; 5] = [
            "date",
            "content-length",
            "transfer-encoding",
            "connection",
            "keep-alive",
        ];
        let status = resp.status().as_u16();
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .filter(|(k, _)| {
                !SKIP_RESPONSE_HEADERS
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(k.as_str()))
            })
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let body = resp.bytes().await.map_err(|e| {
            OxideError(Box::new(Diagnostic::error(
                "RPL-001",
                format!("the replay response body could not be read: {e}"),
            )))
        })?;
        Ok(oxide_fw_dispatch::replay::ExecutedResponse {
            status,
            headers,
            body: body.to_vec(),
        })
    }
}

/// The replay result the CLI prints and the MCP tool serves (§V2-F15:
/// "`--json` emits `{ ok, divergences: [..] }`").
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplayReport {
    pub ok: bool,
    pub divergences: Vec<oxide_fw_dispatch::replay::Divergence>,
    /// The record was made on an older build id (`RPL-003`; the replay ran
    /// anyway).
    pub stale_build: bool,
}

/// The one replay definition the `oxide replay` command and the MCP
/// `replay.run` tool drive: load the record, point the worker pool at it
/// (`OXIDE_REPLAY`), boot the dev-mode stack on an ephemeral port,
/// re-execute the captured request through the real pipeline over
/// loopback, and diff against the record.
pub(crate) async fn run_replay(
    root: &str,
    id: &str,
) -> anyhow::Result<Result<ReplayReport, oxide_fw_diagnostics::OxideError>> {
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let store = oxide_fw_dispatch::record::DirRecordStore::new(&rootp);
    // The load errors surface unchanged (RPL-001 missing/unreadable,
    // RPL-005 foreign version) — the same doors the runner re-runs below,
    // checked here so a bad record refuses before any stack is built.
    let _record = match oxide_fw_dispatch::record::RecordSource::load(&store, id) {
        Ok(r) => r,
        Err(e) => return Ok(Err(e)),
    };
    // The build id the record is replayed against — the same derivation
    // the dev stack uses (the client manifest's build id, `dev` without
    // one), so an RPL-003 warn only fires for a genuinely older build.
    let current_build_id = std::fs::read_to_string(rootp.join("dist/client/client.manifest.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("build_id").and_then(|b| b.as_str()).map(String::from))
        .unwrap_or_else(|| "dev".to_string());
    // The worker pool the stack spawns inherits this env: the workers
    // install `globalThis.__oxide_replay` and play the record's fetch ops
    // and sequences into the re-execution (the V2-F15 worker half).
    let record_file = rootp
        .join(oxide_fw_dispatch::record::RECORDS_DIR)
        .join(format!("{id}.json"));
    // SAFETY: one-shot command startup, before the stack spawns any
    // worker that reads it (the established startup-env window).
    unsafe { std::env::set_var("OXIDE_REPLAY", &record_file) };

    let config_path = rootp.join("oxide.config.ts");
    let config_arg = if config_path.is_file() {
        Some(config_path)
    } else {
        None
    };
    let stack = crate::framework_shim::build_framework_stack(
        config_arg.as_deref(),
        oxide_fw_config::JsBackend::Node,
        None,
        None,
        // §V2-A09: replay renders keep importing source modules.
        oxide_fw_jshost::ModuleSource::Source,
    )
    .await;
    let started = crate::start_dev_server_full(
        root,
        Some(0),
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        None,
        Vec::new(),
        stack,
        None,
    )
    .await?;

    let executor = ReplayHttpExecutor { addr: started.addr };
    let runner =
        oxide_fw_dispatch::replay::ReplayRunner::new(std::sync::Arc::new(store), &current_build_id);
    let outcome = runner.run(id, &executor).await;
    // The one-shot server goes down with the command.
    let _ = started.shutdown.send(true);
    match outcome {
        Ok(outcome) => Ok(Ok(ReplayReport {
            ok: outcome.diff.ok,
            divergences: outcome.diff.divergences,
            stale_build: outcome.stale_build,
        })),
        Err(e) => Ok(Err(e)),
    }
}

/// `oxide replay <id> [--diff] [--json]` (§V2-F15): prints the replayed
/// status, headers and body; `--diff` compares against the record
/// (nonce-normalized); `--json` emits `{ ok, divergences: [..] }`. Exit
/// codes: 0 when the replay matched (or no `--diff` was asked and the
/// replay ran), 1 when `--diff` found divergences, 2 when the record
/// could not be loaded or the replay refused.
pub(crate) async fn cmd_replay(
    root: &str,
    id: &str,
    diff: bool,
    json: bool,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    match run_replay(root, id).await? {
        Err(e) => {
            if json {
                outln!(
                    w,
                    "{}",
                    serde_json::json!({"ok": false, "error": {"code": e.0.code, "message": e.0.message}})
                );
            } else {
                outln!(w, "{} {}", e.0.code, e.0.message);
                if let Some(fix) = &e.0.fix {
                    outln!(w, "  fix: {fix}");
                }
            }
            Ok(2)
        }
        Ok(report) => {
            if json {
                // The spec's agent shape, verbatim: `{ ok, divergences: [..] }`.
                outln!(
                    w,
                    "{}",
                    serde_json::json!({"ok": report.ok, "divergences": report.divergences})
                );
                return Ok(if report.ok { 0 } else { 1 });
            }
            if report.stale_build {
                outln!(
                    w,
                    "warning RPL-003: the record was made on an older build id; the replay ran anyway"
                );
            }
            if report.divergences.is_empty() {
                outln!(w, "replay {id}: the response matches the record");
            } else {
                outln!(w, "replay {id}: {} divergence(s)", report.divergences.len());
            }
            for d in &report.divergences {
                outln!(w, "  - [{}] {}", d.kind, d.detail);
            }
            // The `--diff` verdict decides the exit; without `--diff` a
            // clean run is a clean exit.
            if diff {
                Ok(if report.ok { 0 } else { 1 })
            } else {
                Ok(0)
            }
        }
    }
}
