//! `oxide mcp` (spec §19.3): an MCP server exposing project facts — routes,
//! cache, graph, diagnostics — so coding agents query accurate state.
//!
//! Transport: stdio. Stateless — each tool re-reads the relevant artifact, so
//! the agent always sees fresh facts. Every tool returns structured JSON as a
//! text content block.

use anyhow::Result;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::{ErrorData, ServerHandler, tool, tool_router};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The MCP server. Holds only the project root; all state is read on demand.
#[derive(Clone)]
pub struct OxideMcp {
    pub root: PathBuf,
}

/// No-argument tool input (rmcp requires a named parameter type).
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NoArgs {}

// ---------------------------------------------------------------------------
// Response shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RouteInfo {
    pub pattern: String,
    pub kind: String,
    pub render_mode: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoutesReport {
    pub root: String,
    pub count: usize,
    pub routes: Vec<RouteInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheReport {
    pub root: String,
    pub cache_dir: Option<String>,
    pub disk_bytes: u64,
    pub blob_count: u64,
}

/// §5-A: compact view of the last build's `.oxide/graph.json`. The full
/// node list is large for a real project — the MCP consumer wants counts
/// and a per-kind breakdown, not 5000 module rows over the wire.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GraphSummary {
    pub version: String,
    pub build_id: String,
    pub generated_at_unix_ms: u64,
    pub node_count: usize,
    pub recomputed_count: usize,
    pub previous_node_count: usize,
    /// Count of nodes by kind (`module` / `chunk` / `manifest` / `typegen`).
    pub nodes_by_kind: BTreeMap<String, usize>,
}

impl GraphSummary {
    fn from_snapshot(snap: &crate::build_graph::GraphSnapshot) -> Self {
        let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
        for n in &snap.nodes {
            *by_kind.entry(n.kind.clone()).or_insert(0) += 1;
        }
        Self {
            version: snap.version.clone(),
            build_id: snap.build_id.clone(),
            generated_at_unix_ms: snap.generated_at_unix_ms,
            node_count: snap.node_count,
            recomputed_count: snap.recomputed_count,
            previous_node_count: snap.previous_node_count,
            nodes_by_kind: by_kind,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GraphReport {
    pub root: String,
    pub server_manifest: Option<String>,
    pub routes: BTreeMap<String, String>,
    pub chunks: BTreeMap<String, usize>,
    /// §5-A: the last build's DemandGraph snapshot, when one exists. `None`
    /// before a first `oxide build`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<GraphSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StateReport {
    pub root: String,
    pub source: Option<String>,
    pub waterfall_count: usize,
    pub waterfalls: Vec<WaterfallEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WaterfallEntry {
    pub request_id: String,
    /// Ordered (stage, ms) pairs — mirrors the Server-Timing header.
    pub stages: Vec<(String, f64)>,
    /// §20 / O3: the inbound W3C trace id, when the request carried one.
    /// Lets the MCP client join this row to spans in Jaeger/Tempo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorsReport {
    pub root: String,
    pub source: Option<String>,
    pub entries: Vec<ErrorEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorEntry {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub fix: Option<String>,
}

/// Serialize a tool's response into a text content block. Public so tests
/// and downstream consumers can reuse it.
pub fn json_result<T: Serialize>(v: &T) -> Result<CallToolResult, ErrorData> {
    let body = serde_json::to_string_pretty(v)
        .map_err(|e| ErrorData::internal_error(format!("serialize failed: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(body)]))
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[tool_router]
impl OxideMcp {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[tool(description = "List every route in the project: pattern, kind \
                       (page|endpoint|layout), inferred render mode, and cache tags.")]
    pub async fn oxide_routes(
        &self,
        Parameters(NoArgs {}): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let cfg = super::load_config_or_default(&self.root, "development");
        let tree = super::scan_routes(&self.root, &cfg, None)
            .await
            .map_err(|e| ErrorData::internal_error(format!("scan failed: {e}"), None))?;

        // Join modes + tags from the machine-readable manifest (only exists
        // after `oxide routes` or `oxide build`).
        let routes_json = self.root.join(".oxide/types/routes.json");
        let mut modes: BTreeMap<String, String> = BTreeMap::new();
        let mut tags: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Ok(bytes) = std::fs::read(&routes_json)
            && let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes) {
                for row in rows {
                    if let Some(p) = row.get("pattern").and_then(|v| v.as_str()) {
                        if let Some(m) = row.get("renderMode").and_then(|v| v.as_str()) {
                            modes.insert(p.to_string(), m.to_string());
                        }
                        if let Some(ts) = row.get("tags").and_then(|v| v.as_array()) {
                            tags.insert(
                                p.to_string(),
                                ts.iter()
                                    .filter_map(|t| t.as_str().map(String::from))
                                    .collect(),
                            );
                        }
                    }
                }
            }

        let routes: Vec<RouteInfo> = tree
            .table()
            .into_iter()
            .map(|(pattern, kind, _)| RouteInfo {
                render_mode: modes
                    .get(&pattern)
                    .cloned()
                    .unwrap_or_else(|| "unknown".into()),
                tags: tags.remove(&pattern).unwrap_or_default(),
                pattern,
                kind,
            })
            .collect();
        json_result(&RoutesReport {
            root: self.root.to_string_lossy().into_owned(),
            count: routes.len(),
            routes,
        })
    }

    #[tool(
        description = "Cache statistics: persistent cache directory, total disk bytes, \
                       and blob count."
    )]
    pub async fn oxide_fw_cache(
        &self,
        Parameters(NoArgs {}): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let cache_dir = self.root.join(".oxide/cache");
        let mut report = CacheReport {
            root: self.root.to_string_lossy().into_owned(),
            cache_dir: None,
            disk_bytes: 0,
            blob_count: 0,
        };
        if !cache_dir.is_dir() {
            return json_result(&report);
        }
        report.cache_dir = Some(cache_dir.to_string_lossy().into_owned());

        let blobs = cache_dir.join("blobs");
        if blobs.is_dir()
            && let Ok(rd) = std::fs::read_dir(&blobs) {
                for shard in rd.flatten() {
                    if let Ok(files) = std::fs::read_dir(shard.path()) {
                        for f in files.flatten() {
                            if let Ok(m) = f.metadata()
                                && m.is_file() {
                                    report.disk_bytes += m.len();
                                    report.blob_count += 1;
                                }
                        }
                    }
                }
            }
        json_result(&report)
    }

    #[tool(
        description = "Module graph summary: every route's entry chunk plus per-chunk \
                       module counts. Requires a prior `oxide build`."
    )]
    pub async fn oxide_fw_graph(
        &self,
        Parameters(NoArgs {}): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let sm_path = self.root.join("dist/server/server.manifest.json");
        // §5-A: load the DemandGraph snapshot the last build wrote. Done
        // BEFORE the manifest check so a consumer still sees the graph
        // even if the manifest read fails (both are written in the same
        // build, but a partial copy shouldn't lose the graph report too).
        let graph = {
            let snap = crate::build_graph::load_snapshot(&self.root);
            if snap.node_count > 0 {
                Some(GraphSummary::from_snapshot(&snap))
            } else {
                None
            }
        };
        let mut report = GraphReport {
            root: self.root.to_string_lossy().into_owned(),
            server_manifest: None,
            routes: BTreeMap::new(),
            chunks: BTreeMap::new(),
            graph,
        };
        let Ok(bytes) = std::fs::read(&sm_path) else {
            return json_result(&report);
        };
        report.server_manifest = Some(sm_path.to_string_lossy().into_owned());
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(routes) = v.get("routes").and_then(|r| r.as_object()) {
                for (id, row) in routes {
                    if let Some(m) = row.get("module").and_then(|m| m.as_str()) {
                        report.routes.insert(id.clone(), m.to_string());
                    }
                }
            }
            for m in report.routes.values() {
                *report.chunks.entry(m.clone()).or_insert(0) += 1;
            }
        }
        json_result(&report)
    }

    #[tool(
        description = "Live request waterfalls from the running `oxide dev` server: \
                       the last 50 requests, each with per-stage timings \
                       (middleware / load / render ms). Read from the dev \
                       server's persistent `.oxide/last-waterfalls.json`. \
                       Returns an empty list when no dev server is running."
    )]
    pub async fn oxide_state(
        &self,
        Parameters(NoArgs {}): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let path = self.root.join(".oxide/last-waterfalls.json");
        let mut report = StateReport {
            root: self.root.to_string_lossy().into_owned(),
            source: None,
            waterfall_count: 0,
            waterfalls: Vec::new(),
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return json_result(&report);
        };
        report.source = Some(path.to_string_lossy().into_owned());
        // The recorder writes `Vec<RequestTimingsRow>`: each row is
        // `{ request_id, waterfall_ms: [(stage, ms), ...] }`.
        if let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes) {
            for row in rows {
                let rid = row
                    .get("request_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let stages = row
                    .get("waterfall_ms")
                    .and_then(|w| w.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|pair| {
                                let arr = pair.as_array()?;
                                let name = arr.first()?.as_str()?.to_string();
                                let ms = arr.get(1)?.as_f64()?;
                                Some((name, ms))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                report.waterfalls.push(WaterfallEntry {
                    request_id: rid,
                    stages,
                    trace_id: row
                        .get("trace_id")
                        .and_then(|v| v.as_str().map(String::from)),
                });
            }
        }
        report.waterfall_count = report.waterfalls.len();
        json_result(&report)
    }

    #[tool(
        description = "Diagnostics from the last `oxide build` run: error code, severity, \
                       message, and the suggested fix."
    )]
    pub async fn oxide_errors(
        &self,
        Parameters(NoArgs {}): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let diag_path = self.root.join(".oxide/last-build-diagnostics.json");
        let mut report = ErrorsReport {
            root: self.root.to_string_lossy().into_owned(),
            source: None,
            entries: Vec::new(),
        };
        let Ok(bytes) = std::fs::read(&diag_path) else {
            return json_result(&report);
        };
        report.source = Some(diag_path.to_string_lossy().into_owned());
        if let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes) {
            for row in rows {
                report.entries.push(ErrorEntry {
                    code: row
                        .get("code")
                        .and_then(|v| v.as_str())
                        .unwrap_or("UNKNOWN")
                        .to_string(),
                    severity: row
                        .get("severity")
                        .and_then(|v| v.as_str())
                        .unwrap_or("error")
                        .to_string(),
                    message: row
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    fix: row.get("fix").and_then(|v| v.as_str()).map(String::from),
                });
            }
        }
        json_result(&report)
    }
}

// ---------------------------------------------------------------------------
// ServerHandler impl + entry point
// ---------------------------------------------------------------------------

impl ServerHandler for OxideMcp {
    fn get_info(&self) -> ServerInfo {
        // ServerInfo is `#[non_exhaustive]`; build via Default + field mutation
        // rather than a struct literal.
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::LATEST;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::from_build_env();
        info.instructions = Some(
            "Oxide MCP server — query project facts (routes, cache, graph, diagnostics, \
             live request waterfalls) for the project rooted at the path passed to \
             `oxide mcp --root`."
                .into(),
        );
        info
    }
}

/// Run the MCP server on stdio until EOF.
pub async fn serve_stdio(root: PathBuf) -> Result<()> {
    use rmcp::ServiceExt as _;
    let service = OxideMcp::new(root)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("MCP serve failed: {e}"))?;
    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("MCP session ended with error: {e}"))?;
    Ok(())
}
