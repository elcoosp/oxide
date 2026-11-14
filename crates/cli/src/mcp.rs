//! `oxide mcp` (spec §19.3 + V2-E07): an MCP server exposing project facts —
//! routes, cache, graph, diagnostics, resolved config — so coding agents
//! query accurate state.
//!
//! V2-E07's seam is the [`McpTool`] registry: every tool is an object with a
//! name, a description, a JSON Schema input, a `writes()` flag and an async
//! `call(&McpCx, args)`. The server (`OxideMcp`) implements the rmcp
//! `ServerHandler` manually over the registry — `tools/list` renders the
//! registry, `tools/call` dispatches by name — so a new tool is a new
//! `impl McpTool` plus one registry line, never a server edit.
//!
//! Read-only by default: a tool with `writes() == true` is refused with a
//! named refusal unless the server was started with `--allow-write` (the
//! SEC-09 dev-token pattern's CLI-flag shape). Secrets never leave the
//! process: `config.get` redacts every key matching
//! `oxide_fw_server::is_secret_key`.
//!
//! Transport: stdio. Stateless — each tool re-reads the relevant artifact,
//! so the agent always sees fresh facts. Every tool returns structured JSON
//! as a text content block.

use anyhow::Result;
use rmcp::model::{
    CallToolResult, Content, ErrorCode, Implementation, ListToolsResult, ProtocolVersion,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// The registry seam (V2-E07)
// ---------------------------------------------------------------------------

/// The context a tool call runs in: the project root plus the write policy
/// the server was started with.
#[derive(Clone, Debug)]
pub struct McpCx {
    pub root: PathBuf,
    /// `false` (the default) refuses every `writes() == true` tool.
    pub allow_write: bool,
}

/// One MCP tool: the registry seam the server dispatches over.
#[async_trait::async_trait]
pub trait McpTool: Send + Sync {
    /// The tool's protocol name (`routes.list`, `cache.inspect`, ...).
    fn name(&self) -> &'static str;
    /// One-line description rendered into `tools/list`.
    fn description(&self) -> &'static str;
    /// The JSON Schema of the `arguments` object.
    fn input_schema(&self) -> serde_json::Value;
    /// `true` when the tool mutates the project — refused unless the server
    /// runs with `--allow-write`.
    fn writes(&self) -> bool {
        false
    }
    /// Run the tool. A refusal the CALLER should read (a missing route, a
    /// write attempt without `--allow-write`) is an `Ok(CallToolResult::
    /// error(..))`; only a broken request or a server fault is an
    /// `Err(ErrorData)`.
    async fn call(&self, cx: &McpCx, args: serde_json::Value) -> Result<CallToolResult, ErrorData>;
}

/// Extract a string argument from the tool-call arguments object.
fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

/// Every tool the server serves, in the stable `tools/list` order (spec
/// names first, the dev-observability extras after).
pub fn default_registry() -> Vec<Box<dyn McpTool>> {
    vec![
        Box::new(RoutesListTool),
        Box::new(RouteExplainTool),
        Box::new(ErrorsRecentTool),
        Box::new(CacheInspectTool),
        Box::new(GraphWhyTool),
        Box::new(ConfigGetTool),
        Box::new(StateRecentTool),
        Box::new(BuildRunTool),
    ]
}

// ---------------------------------------------------------------------------
// Response shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RouteInfo {
    pub id: String,
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

/// V2-E07 `route.explain`: one route's dispatch facts — the layout chain the
/// render walks, the middleware chain the request passes, the cache tags and
/// the render mode (the "why is /dashboard dynamic?" answer, joined with the
/// F16 dynamic reasons when the routes manifest carries them).
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RouteExplainReport {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    pub kind: String,
    pub render_mode: String,
    pub tags: Vec<String>,
    pub has_server_module: bool,
    pub layout_chain: Vec<String>,
    pub middleware_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheReport {
    pub root: String,
    pub cache_dir: Option<String>,
    pub disk_bytes: u64,
    pub blob_count: u64,
    /// V2-E07: when the call named a tag (or a route — the `path:<route>`
    /// tag convention), the entry keys that carry it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
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

/// V2-E07 `config.get`: the resolved config with every secret-shaped key
/// redacted (the `is_secret_key` rule the request log uses).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConfigGetReport {
    pub root: String,
    /// `true` when at least one key was redacted — the caller knows the
    /// value existed and was withheld.
    pub redacted_count: usize,
    pub config: serde_json::Value,
}

/// Serialize a tool's response into a text content block. Public so tests
/// and downstream consumers can reuse it.
pub fn json_result<T: Serialize>(v: &T) -> Result<CallToolResult, ErrorData> {
    let body = serde_json::to_string_pretty(v)
        .map_err(|e| ErrorData::internal_error(format!("serialize failed: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(body)]))
}

// ---------------------------------------------------------------------------
// The tools
// ---------------------------------------------------------------------------

/// `routes.list` — every route in the project: id, pattern, kind
/// (page|endpoint|layout), inferred render mode, and cache tags.
struct RoutesListTool;

#[async_trait::async_trait]
impl McpTool for RoutesListTool {
    fn name(&self) -> &'static str {
        "routes.list"
    }

    fn description(&self) -> &'static str {
        "List every route in the project: id, pattern, kind (page|endpoint|layout), \
         inferred render mode, and cache tags."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {}, "required": [] })
    }

    async fn call(
        &self,
        cx: &McpCx,
        _args: serde_json::Value,
    ) -> Result<CallToolResult, ErrorData> {
        let cfg = super::load_config_or_default(&cx.root, "development");
        let tree = super::scan_routes(&cx.root, &cfg, None)
            .await
            .map_err(|e| ErrorData::internal_error(format!("scan failed: {e}"), None))?;

        // Join modes + tags from the machine-readable manifest (only exists
        // after `oxide routes` or `oxide build`).
        let routes_json = cx.root.join(".oxide/types/routes.json");
        let mut modes: BTreeMap<String, String> = BTreeMap::new();
        let mut tags: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Ok(bytes) = std::fs::read(&routes_json)
            && let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes)
        {
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
                id: pattern.clone(),
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
            root: cx.root.to_string_lossy().into_owned(),
            count: routes.len(),
            routes,
        })
    }
}

/// `route.explain` — one route's dispatch facts: layout chain, middleware
/// chain, render mode, cache tags and the F16 dynamic reason when the
/// routes manifest carries one.
struct RouteExplainTool;

#[async_trait::async_trait]
impl McpTool for RouteExplainTool {
    fn name(&self) -> &'static str {
        "route.explain"
    }

    fn description(&self) -> &'static str {
        "Explain one route: layout chain, middleware chain, inferred render mode \
         (+ why-dynamic reason when recorded), cache tags, and whether a server \
         data module exists. `route` is the route id/pattern as `routes.list` shows it."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "route": { "type": "string", "description": "route id or pattern" } },
            "required": ["route"]
        })
    }

    async fn call(&self, cx: &McpCx, args: serde_json::Value) -> Result<CallToolResult, ErrorData> {
        let Some(route) = arg_str(&args, "route") else {
            return Err(ErrorData::invalid_params("missing `route` argument", None));
        };
        let cfg = super::load_config_or_default(&cx.root, "development");
        let tree = super::scan_routes(&cx.root, &cfg, None)
            .await
            .map_err(|e| ErrorData::internal_error(format!("scan failed: {e}"), None))?;

        // Match by id (the routes.json spelling) or by the table's pattern
        // spelling — both are `routes.list` output.
        let entry = tree
            .entries
            .iter()
            .find(|e| e.id.0 == route || tree_pattern(&tree, &e.id.0).as_deref() == Some(route))
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    format!("unknown route `{route}` — routes.list names the valid ids"),
                    None,
                )
            })?;

        // Layout chain: the render-walk order (the CLI's one definition
        // site); middleware chain: the §9.1 prefix index.
        let layout = super::compute_layout_chain(&tree, &entry.id.0);
        let mw = tree
            .middleware_files_for(&entry.id.0)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();

        // The F16 dynamic reason rides the routes manifest when the build
        // wrote one (`.oxide/types/routes.json`).
        let manifest_row = routes_manifest_row(&cx.root, &entry.id.0);
        let mut report = RouteExplainReport {
            id: entry.id.0.clone(),
            pattern: tree_pattern(&tree, &entry.id.0),
            kind: format!("{:?}", entry.kind),
            render_mode: entry
                .render_mode
                .clone()
                .or_else(|| manifest_render_mode(&cx.root, &entry.id.0))
                .unwrap_or_else(|| "unknown".into()),
            tags: entry.tags.clone(),
            has_server_module: entry.has_server_module,
            layout_chain: layout,
            middleware_chain: mw,
            dynamic_reason: manifest_row.as_ref().and_then(|r| {
                r.get("dynamicReason")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            }),
        };
        if report.dynamic_reason.is_none() {
            // The scan's own signal: a load that reads something time- or
            // request-dependent is recorded by the why-dynamic engine at
            // build; without a build, `dynamic` implies at least the
            // server-module read the scan saw.
            if entry.render_mode.as_deref() == Some("dynamic") {
                report.dynamic_reason = Some("inferred dynamic at scan (no build manifest)".into());
            }
        }
        json_result(&report)
    }
}

/// The table's pattern spelling for a route id, when the entry exists.
fn tree_pattern(tree: &oxide_fw_router::RouteTree, id: &str) -> Option<String> {
    tree.entries.iter().find(|e| e.id.0 == id).map(|e| {
        // The §19.1 manifest spells segments with `:param`; the pattern
        // row the routes.json carries is authoritative when present.
        format!("/{}", e.segments.join("/"))
    })
}

fn routes_manifest_row(root: &PathBuf, id: &str) -> Option<serde_json::Value> {
    let bytes = std::fs::read(root.join(".oxide/types/routes.json")).ok()?;
    let rows = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes).ok()?;
    rows.into_iter()
        .find(|r| r.get("id").and_then(|v| v.as_str()) == Some(id))
}

fn manifest_render_mode(root: &PathBuf, id: &str) -> Option<String> {
    routes_manifest_row(root, id).and_then(|r| {
        r.get("renderMode")
            .and_then(|v| v.as_str())
            .map(String::from)
    })
}

/// `errors.recent` — diagnostics from the last `oxide build` run.
struct ErrorsRecentTool;

#[async_trait::async_trait]
impl McpTool for ErrorsRecentTool {
    fn name(&self) -> &'static str {
        "errors.recent"
    }

    fn description(&self) -> &'static str {
        "Diagnostics from the last `oxide build` run: error code, severity, \
         message, and the suggested fix."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {}, "required": [] })
    }

    async fn call(
        &self,
        cx: &McpCx,
        _args: serde_json::Value,
    ) -> Result<CallToolResult, ErrorData> {
        let diag_path = cx.root.join(".oxide/last-build-diagnostics.json");
        let mut report = ErrorsReport {
            root: cx.root.to_string_lossy().into_owned(),
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

/// `cache.inspect` — persistent cache statistics, plus the entry keys that
/// carry a tag (or a route's `path:<route>` tag).
struct CacheInspectTool;

#[async_trait::async_trait]
impl McpTool for CacheInspectTool {
    fn name(&self) -> &'static str {
        "cache.inspect"
    }

    fn description(&self) -> &'static str {
        "Cache statistics: persistent cache directory, total disk bytes, and blob \
         count. With `tag` (or `route`, which queries the route's `path:<route>` \
         tag), also lists the entry keys that carry it."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "tag": { "type": "string", "description": "cache tag to list keys for" },
                "route": { "type": "string", "description": "route path — queried as its `path:<route>` tag" }
            },
            "required": []
        })
    }

    async fn call(&self, cx: &McpCx, args: serde_json::Value) -> Result<CallToolResult, ErrorData> {
        let cache_dir = crate::perf_bisect::cache_dir_for(&cx.root);
        let mut report = CacheReport {
            root: cx.root.to_string_lossy().into_owned(),
            cache_dir: None,
            disk_bytes: 0,
            blob_count: 0,
            tag: None,
            keys: Vec::new(),
        };
        if !cache_dir.is_dir() {
            return json_result(&report);
        }
        report.cache_dir = Some(cache_dir.to_string_lossy().into_owned());

        let blobs = cache_dir.join("blobs");
        if blobs.is_dir()
            && let Ok(rd) = std::fs::read_dir(&blobs)
        {
            for shard in rd.flatten() {
                if let Ok(files) = std::fs::read_dir(shard.path()) {
                    for f in files.flatten() {
                        if let Ok(m) = f.metadata()
                            && m.is_file()
                        {
                            report.disk_bytes += m.len();
                            report.blob_count += 1;
                        }
                    }
                }
            }
        }

        // The tag query: the disk tier's tag multimap (V2-B06). A route
        // argument is the `path:<route>` tag convention.
        let tag = arg_str(&args, "tag")
            .map(String::from)
            .or_else(|| arg_str(&args, "route").map(|r| oxide_fw_cache::tiers::path_tag(r)));
        if let Some(tag) = tag {
            report.tag = Some(tag.clone());
            if let Ok(tier) = oxide_fw_cache::DiskTier::open(cache_dir)
                && let Ok(keys) = tier.tag_keys(&tag)
            {
                report.keys = keys;
            }
        }
        json_result(&report)
    }
}

/// `graph.why` — module graph summary: every route's entry chunk plus
/// per-chunk module counts, and the last build's DemandGraph snapshot.
struct GraphWhyTool;

#[async_trait::async_trait]
impl McpTool for GraphWhyTool {
    fn name(&self) -> &'static str {
        "graph.why"
    }

    fn description(&self) -> &'static str {
        "Module graph summary: every route's entry chunk plus per-chunk module \
         counts, and the last build's demand-graph snapshot. Requires a prior \
         `oxide build`."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {}, "required": [] })
    }

    async fn call(
        &self,
        cx: &McpCx,
        _args: serde_json::Value,
    ) -> Result<CallToolResult, ErrorData> {
        let sm_path = cx.root.join("dist/server/server.manifest.json");
        // §5-A: load the DemandGraph snapshot the last build wrote. Done
        // BEFORE the manifest check so a consumer still sees the graph
        // even if the manifest read fails (both are written in the same
        // build, but a partial copy shouldn't lose the graph report too).
        let graph = {
            let snap = crate::build_graph::load_snapshot(&cx.root);
            if snap.node_count > 0 {
                Some(GraphSummary::from_snapshot(&snap))
            } else {
                None
            }
        };
        let mut report = GraphReport {
            root: cx.root.to_string_lossy().into_owned(),
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
}

/// `config.get` — the resolved config, secrets redacted.
struct ConfigGetTool;

#[async_trait::async_trait]
impl McpTool for ConfigGetTool {
    fn name(&self) -> &'static str {
        "config.get"
    }

    fn description(&self) -> &'static str {
        "The resolved oxide.config for the project (defaults filled in), with \
         every secret-shaped key redacted — keys matching the request-log's \
         is_secret_key rule never leave the process."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {}, "required": [] })
    }

    async fn call(
        &self,
        cx: &McpCx,
        _args: serde_json::Value,
    ) -> Result<CallToolResult, ErrorData> {
        let cfg = super::load_config_or_default(&cx.root, "development");
        let mut value = serde_json::to_value(&cfg)
            .map_err(|e| ErrorData::internal_error(format!("serialize failed: {e}"), None))?;
        let redacted = redact_secrets(&mut value);
        json_result(&ConfigGetReport {
            root: cx.root.to_string_lossy().into_owned(),
            redacted_count: redacted,
            config: value,
        })
    }
}

/// Redact every secret-shaped key in a config JSON tree, in place. Returns
/// the number of redacted values. Keys match the same rule the request log
/// uses (`oxide_fw_server::is_secret_key`): cookie/authorization headers and
/// anything naming a secret, token, password or api key.
pub fn redact_secrets(v: &mut serde_json::Value) -> usize {
    use serde_json::Value;
    let mut count = 0usize;
    match v {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                if oxide_fw_server::is_secret_key(&k) {
                    map.insert(k, Value::String("***".into()));
                    count += 1;
                } else if let Some(child) = map.get_mut(&k) {
                    count += redact_secrets(child);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                count += redact_secrets(item);
            }
        }
        _ => {}
    }
    count
}

/// `state.recent` — live request waterfalls from the running dev server.
struct StateRecentTool;

#[async_trait::async_trait]
impl McpTool for StateRecentTool {
    fn name(&self) -> &'static str {
        "state.recent"
    }

    fn description(&self) -> &'static str {
        "Live request waterfalls from the running `oxide dev` server: the last \
         50 requests, each with per-stage timings (middleware / load / render \
         ms). Returns an empty list when no dev server is running."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {}, "required": [] })
    }

    async fn call(
        &self,
        cx: &McpCx,
        _args: serde_json::Value,
    ) -> Result<CallToolResult, ErrorData> {
        let path = cx.root.join(".oxide/last-waterfalls.json");
        let mut report = StateReport {
            root: cx.root.to_string_lossy().into_owned(),
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
}

/// `build.run` — a WRITE tool: runs `oxide build` for the project. Refused
/// unless the server was started with `--allow-write`.
struct BuildRunTool;

#[async_trait::async_trait]
impl McpTool for BuildRunTool {
    fn name(&self) -> &'static str {
        "build.run"
    }

    fn description(&self) -> &'static str {
        "Run `oxide build` for the project (a WRITE tool: mutates dist/ and \
         .oxide/). Refused unless the server was started with --allow-write."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {}, "required": [] })
    }

    fn writes(&self) -> bool {
        true
    }

    async fn call(
        &self,
        cx: &McpCx,
        _args: serde_json::Value,
    ) -> Result<CallToolResult, ErrorData> {
        if !cx.allow_write {
            return Ok(CallToolResult::error(vec![Content::text(
                "build.run is a write tool: start the server with `oxide mcp \
                 --allow-write` to enable it (read-only by default).",
            )]));
        }
        let root = cx.root.to_string_lossy().into_owned();
        let result = super::cmd_build(&root, false, false, false, None, &mut std::io::sink()).await;
        match result {
            Ok(exit) => json_result(&json!({ "root": root, "exit": exit })),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "build failed: {e}"
            ))])),
        }
    }
}

// ---------------------------------------------------------------------------
// The server: manual ServerHandler over the registry
// ---------------------------------------------------------------------------

/// The MCP server. Holds the call context and the tool registry; all state
/// is read on demand.
#[derive(Clone)]
pub struct OxideMcp {
    pub cx: McpCx,
    tools: Arc<Vec<Box<dyn McpTool>>>,
}

impl OxideMcp {
    pub fn new(root: PathBuf) -> Self {
        Self::with_policy(McpCx {
            root,
            allow_write: false,
        })
    }

    pub fn with_policy(cx: McpCx) -> Self {
        Self {
            cx,
            tools: Arc::new(default_registry()),
        }
    }

    /// `tools/list` payload: the registry rendered into rmcp `Tool`s, in the
    /// registry's stable order.
    pub fn registry_tools(&self) -> Vec<Tool> {
        self.tools
            .iter()
            .map(|t| {
                let schema = t.input_schema();
                let obj = schema.as_object().cloned().unwrap_or_default();
                Tool::new(t.name(), t.description(), obj)
            })
            .collect()
    }

    /// Dispatch one tool call by name. Unknown names are
    /// `ErrorCode::METHOD_NOT_FOUND`; a `writes()` tool without the flag is
    /// an error RESULT (the caller reads the refusal), not a protocol error.
    pub async fn dispatch(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(tool) = self.tools.iter().find(|t| t.name() == name) else {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown tool `{name}` — tools/list names the registry"),
                None,
            ));
        };
        if tool.writes() && !self.cx.allow_write {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "tool `{}` is a write tool: start the server with `oxide mcp \
                 --allow-write` to enable it (read-only by default).",
                tool.name()
            ))]));
        }
        tool.call(&self.cx, args).await
    }
}

impl ServerHandler for OxideMcp {
    fn get_info(&self) -> ServerInfo {
        // ServerInfo is `#[non_exhaustive]`; build via Default + field mutation
        // rather than a struct literal.
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::LATEST;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::from_build_env();
        info.instructions = Some(
            "Oxide MCP server — query project facts (routes, route explanations, \
             cache, graph, diagnostics, resolved config, live request waterfalls) \
             for the project rooted at the path passed to `oxide mcp --root`. \
             Write tools need `--allow-write`."
                .into(),
        );
        info
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.registry_tools())))
    }

    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, ErrorData>> + Send + '_ {
        let name = request.name.to_string();
        let args = request
            .arguments
            .map(|obj| serde_json::Value::Object(obj))
            .unwrap_or_else(|| json!({}));
        let this = self.clone();
        async move { this.dispatch(&name, args).await }
    }

    /// The framework's task-support pre-validation consults this before
    /// `tools/call`; the registry is the source of truth here too.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|t| {
            let obj = t.input_schema().as_object().cloned().unwrap_or_default();
            Tool::new(t.name(), t.description(), obj)
        })
    }
}

/// Run the MCP server on stdio until EOF.
pub async fn serve_stdio(root: PathBuf, allow_write: bool) -> Result<()> {
    use rmcp::ServiceExt as _;
    let service = OxideMcp::with_policy(McpCx { root, allow_write })
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("MCP serve failed: {e}"))?;
    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("MCP session ended with error: {e}"))?;
    Ok(())
}
