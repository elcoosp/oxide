//! V2-A01 phase 3: the per-request stage context (spec WP-A01 "Step 3").
//!
//! The 1,400-line `build_dispatch` closure carried its per-request state as
//! ~60 locals; those locals are the [`RequestCtx`] fields here. The command
//! request and the loaded route tree arrive through [`RequestCtx::new`]; the
//! stage that creates a value hands it to the context at its end, and a later
//! stage re-binds it (`&cx.field`, `&mut cx.field`, `clone`, or `take` for the
//! four fields a stage MOVES out of: `params`, `data`, `action_form`,
//! `page_settles`) so the moved body text stays untouched.

/// The boot handles a demoted generation swaps in per request (the B-04
/// tuple), routed by stage_route through the serving pools.
pub(crate) type RoutedGenerationHandles = (
    Option<std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
    Option<std::sync::Arc<dyn oxide_fw_server::PageRenderer>>,
    Option<std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
    std::path::PathBuf,
);

/// The per-request mutable state threaded through the dispatch stages.
pub(crate) struct RequestCtx {
    pub(crate) req: oxide_fw_server::OxideRequest,
    pub(crate) tree: std::sync::Arc<oxide_fw_router::RouteTree>,
    /// V2-B01: the generation's `RouteContextTable` (set by the prelude
    /// alongside the tree, as a matched pair) — the per-route dispatch
    /// answers come from one HashMap hit instead of per-request scans.
    pub(crate) ctx_table: Option<std::sync::Arc<oxide_fw_router::RouteContextTable>>,
    /// V2-B01: the matched (or unmatched not-found) route's precomputed
    /// context — set by `stage_match` from the table.
    pub(crate) route_ctx: Option<std::sync::Arc<oxide_fw_router::RouteContext>>,
    pub(crate) started: std::time::Instant,
    pub(crate) boot_build_id: Option<String>,
    pub(crate) rewrite_hop: u32,
    pub(crate) normalized: String,
    pub(crate) raw: String,
    pub(crate) req_path: String,
    pub(crate) resolved_locale: String,
    pub(crate) is_data_request: bool,
    pub(crate) matched: Option<oxide_fw_router::MatchResult>,
    pub(crate) status: u16,
    pub(crate) label: String,
    pub(crate) loader_runner: Option<std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
    pub(crate) page_renderer: Option<std::sync::Arc<dyn oxide_fw_server::PageRenderer>>,
    pub(crate) module_path: Option<std::path::PathBuf>,
    pub(crate) route_id: Option<String>,
    pub(crate) params: Option<std::collections::BTreeMap<String, String>>,
    pub(crate) has_server_module: bool,
    pub(crate) render_mode: Option<String>,
    pub(crate) needs_parent: bool,
    pub(crate) is_live: bool,
    pub(crate) endpoint_module_path: Option<std::path::PathBuf>,
    pub(crate) middleware_module_paths: Vec<std::path::PathBuf>,
    pub(crate) endpoint_params_json: Option<serde_json::Map<String, serde_json::Value>>,
    pub(crate) endpoint_body: bytes::Bytes,
    pub(crate) request_nonce: String,
    pub(crate) unmatched_route_id: Option<String>,
    pub(crate) unmatched_layout_ids: Vec<String>,
    pub(crate) error_route_id: Option<String>,
    pub(crate) route_id_for_render: Option<String>,
    pub(crate) layout_ids: Vec<String>,
    /// V2-A06: the layouts' §9.2 load specs — module paths plus the
    /// scanner's per-layout `parent()` verdict the load graph serializes
    /// on (see [`crate::LayoutLoadSpec`]).
    pub(crate) layout_module_paths: Vec<crate::LayoutLoadSpec>,
    pub(crate) action_name: Option<String>,
    pub(crate) action_dispatcher: Option<std::sync::Arc<dyn oxide_fw_server::ActionDispatcher>>,
    pub(crate) endpoint_for_dispatch:
        Option<std::sync::Arc<dyn oxide_fw_server::EndpointDispatcher>>,
    pub(crate) action_params: serde_json::Map<String, serde_json::Value>,
    pub(crate) action_route: String,
    pub(crate) req_recording: Option<crate::record_wire::RequestRecording>,
    pub(crate) waterfall: Option<oxide_fw_dev::WaterfallRecorder>,
    pub(crate) brownout_ceiling: Option<u64>,
    pub(crate) l0_eligible: bool,
    pub(crate) l0_route: String,
    pub(crate) l0_params: String,
    pub(crate) l0_vary: String,
    pub(crate) l0_path: String,
    pub(crate) flight_key: String,
    pub(crate) middleware_locals: serde_json::Value,
    pub(crate) middleware_headers: Vec<(String, String)>,
    pub(crate) middleware_ms: f64,
    pub(crate) session_before: Option<serde_json::Value>,
    pub(crate) action_form: Option<serde_json::Value>,
    pub(crate) flight_owner: Option<oxide_fw_cache::FlightOwnerGuard>,
    pub(crate) data: Option<serde_json::Value>,
    pub(crate) page_settles: Option<tokio::sync::mpsc::Receiver<serde_json::Value>>,
    pub(crate) route_meta: Option<serde_json::Value>,
    pub(crate) loader_set_cookies: Option<Vec<String>>,
    pub(crate) loader_headers: Option<Vec<(String, String)>>,
    pub(crate) deferred_values: std::collections::BTreeMap<String, serde_json::Value>,
    pub(crate) stage_load_start: std::time::Instant,
    pub(crate) boot_script_url_for_render: Option<String>,
    pub(crate) boot_script_integrity_for_render: Option<String>,
    /// V2-F04: the §15.4 evaluation outcome (`stage_experiments`). `None`
    /// when no `experiments` config row was eligible (the pre-F04 shapes);
    /// the pins read the assigned map and the vary suffix off it.
    pub(crate) exp_resolved: Option<oxide_fw_dispatch::experiments::Resolved>,
}

impl RequestCtx {
    /// The stage entry state: the request itself plus the route tree the
    /// prelude loaded through the swappable slot (the tree read MUST stay
    /// sync-side: an `Fn` dispatch closure cannot hold the read guard across
    /// the stage awaits).
    pub(crate) fn new(
        req: oxide_fw_server::OxideRequest,
        tree: std::sync::Arc<oxide_fw_router::RouteTree>,
    ) -> Self {
        Self {
            req,
            tree,
            ctx_table: None,
            route_ctx: None,
            started: std::time::Instant::now(),
            boot_build_id: None,
            rewrite_hop: 0,
            normalized: String::new(),
            raw: String::new(),
            req_path: String::new(),
            resolved_locale: String::new(),
            is_data_request: false,
            matched: None,
            status: 0,
            label: String::new(),
            loader_runner: None,
            page_renderer: None,
            module_path: None,
            route_id: None,
            params: None,
            has_server_module: false,
            render_mode: None,
            needs_parent: false,
            is_live: false,
            endpoint_module_path: None,
            middleware_module_paths: Vec::new(),
            endpoint_params_json: None,
            endpoint_body: bytes::Bytes::new(),
            request_nonce: String::new(),
            unmatched_route_id: None,
            unmatched_layout_ids: Vec::new(),
            error_route_id: None,
            route_id_for_render: None,
            layout_ids: Vec::new(),
            layout_module_paths: Vec::new(),
            action_name: None,
            action_dispatcher: None,
            endpoint_for_dispatch: None,
            action_params: serde_json::Map::new(),
            action_route: String::new(),
            req_recording: None,
            waterfall: None,
            brownout_ceiling: None,
            l0_eligible: false,
            l0_route: String::new(),
            l0_params: String::new(),
            l0_vary: String::new(),
            l0_path: String::new(),
            flight_key: String::new(),
            middleware_locals: serde_json::Value::Null,
            middleware_headers: Vec::new(),
            middleware_ms: 0.0,
            session_before: None,
            action_form: None,
            flight_owner: None,
            data: None,
            page_settles: None,
            route_meta: None,
            loader_set_cookies: None,
            loader_headers: None,
            deferred_values: std::collections::BTreeMap::new(),
            stage_load_start: std::time::Instant::now(),
            boot_script_url_for_render: None,
            boot_script_integrity_for_render: None,
            exp_resolved: None,
        }
    }
}
