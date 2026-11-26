//! V2-A01 phase 3: the dispatch deps (spec WP-A01 "Step 3" sketch).
//!
//! The sketch's 18-field `DispatchDeps` — extended with the fields the
//! pipeline grew since the sketch (F07/F10/F12/F15/A04 handles) and the three
//! request-path runtime handles the old `build_dispatch_swappable` setup
//! block built per boot (the PERF-04 single-flight map, the §8.5
//! self-redispatch lock and the F07 stream-id counter). The stage functions
//! take `&DispatchDeps`; the dispatch closure captures one `Arc<DispatchDeps>`,
//! which is what keeps it an `Fn`.

use crate::*;

/// PERF-04: the single-flight map (one per process, shared across every
/// dispatch call).
pub(crate) type FlightMap = dashmap::DashMap<
    String,
    std::sync::Arc<tokio::sync::broadcast::Sender<std::sync::Arc<oxide_fw_cache::Entry>>>,
>;

/// §8.5: the self-redispatch slot the middleware `{ rewrite }` arm reads.
pub(crate) type SelfDispatch = std::sync::OnceLock<
    std::sync::Arc<dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync>,
>;

/// The boot-time dispatch dependencies shared by every request.
pub(crate) struct DispatchDeps {
    pub swappable: SwappableRouteTree,
    pub metrics: std::sync::Arc<oxide_fw_server::Metrics>,
    pub proxy_rules: Vec<oxide_fw_dev::ProxyRule>,
    pub page_renderer: Option<std::sync::Arc<dyn oxide_fw_server::PageRenderer>>,
    pub loader_runner: Option<std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
    pub routes_root: std::path::PathBuf,
    pub boot_script_url: Option<String>,
    /// V2-E04: the manifest's integrity digest for the boot chunk (None when
    /// `security.sri` is off or the manifest predates the field).
    pub boot_script_integrity: Option<String>,
    pub l0: Option<std::sync::Arc<l0_cache::L0Cache>>,
    /// F36: the typed half of the SAME shared tier `l0` wraps — the erased
    /// `Arc<dyn CacheHandler>` cannot name `record_coalesced`, so the render
    /// stage's flight owner records the publish count through this handle
    /// (the F33 emitter's live call site). `Some` exactly when `l0` is
    /// `Some` (a recording half for a plane with no flights is never
    /// fabricated); `None` in dev and the test-helper default — the coalesce
    /// rows join the §20 channel only on a wired boot.
    pub l0_tiered: Option<std::sync::Arc<oxide_fw_cache::TieredCache>>,
    pub actions: Option<std::sync::Arc<dyn oxide_fw_server::ActionDispatcher>>,
    pub archived: Option<std::sync::Arc<oxide_fw_router::ArchivedTree>>,
    pub pool: Option<std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
    pub i18n_cfg: Option<oxide_fw_assets::i18n::I18nConfig>,
    pub endpoint_dispatcher: Option<std::sync::Arc<dyn oxide_fw_server::EndpointDispatcher>>,
    pub middleware_dispatcher: Option<std::sync::Arc<dyn oxide_fw_server::MiddlewareDispatcher>>,
    pub vary_headers: Vec<String>,
    pub waterfall: Option<oxide_fw_dev::WaterfallRecorder>,
    pub session_codec: Option<std::sync::Arc<oxide_fw_server::SessionCodec>>,
    pub ppr_shells: Option<PprShells>,
    pub esm_state: Option<std::sync::Arc<crate::esm_dev::EsmDevState>>,
    /// V2-F15: the recorder capture (`record:` config key) — `None` when
    /// off, and the pipeline is byte-identical.
    pub record: Option<std::sync::Arc<crate::record_wire::RecordCapture>>,
    /// V2-F15: the in-flight captures, keyed by request id.
    pub recordings: std::sync::Arc<crate::record_wire::Recordings>,
    /// V2-A04 + F10: the generations the data plane answers for — the
    /// bounded skew pool (the boot build id plus up to
    /// `deploy.generations` demoted builds, each accepted for
    /// `deploy.skewWindow` from its own demotion). `Some` on every real
    /// server boot (the stage runs); `None` keeps the pre-A04 byte shapes
    /// (the test helper's default). The stage install and the advertise
    /// stamp live in [`build_dispatch_swappable`]'s wrapper, driven
    /// through the same `AcceptedGenerations` seam the 1-deep set uses.
    pub generations:
        Option<std::sync::Arc<std::sync::RwLock<oxide_fw_dispatch::skew::GenerationPool>>>,
    /// V2-F05: the live hub + rerun channel (`serve::live::LiveState`) —
    /// `Some` on every real server boot with `live.transport = "sse"` (the
    /// endpoint arm runs, the revalidation hooks fan out); `None` for the
    /// `poll` transport and the test helper (the arm is inert).
    pub live: Option<std::sync::Arc<crate::serve::live::LiveState>>,
    /// V2-F10 (the serving half): a pool + loader runner + frozen loader
    /// source root per generation. `Some` on every production serve boot
    /// with a worker pool; `None` in dev and the test helper (the route
    /// step is inert and the boot handles serve every generation, the
    /// pre-pool byte shapes).
    pub serving_pools: Option<std::sync::Arc<crate::serve::generations::ServingPools>>,
    /// V2-F07: the resumable-stream wiring (`serve::streams::StreamWiring`
    /// — the replay store + the per-IP reattach limiter). `Some` on every
    /// real server boot (the reattach arm runs and resumable endpoint
    /// responses tee); `None` for the test helper and any arm without a
    /// store (no stream was ever teed, so the arm would answer every id
    /// `Gone` — the pre-F07 fall-through is the honest shape there).
    pub streams: Option<std::sync::Arc<crate::serve::streams::StreamWiring>>,
    /// V2-F12: the brownout wiring (`serve::brownout::BrownoutWiring` —
    /// the governor the sampler drives). `Some` on every real server boot
    /// with `brownout.enabled` (the enforcement stage runs, the ops
    /// endpoints report the level); `None` for the test helper and every
    /// `enabled: false` boot (the stage is inert, the pre-F12 shapes).
    pub brownout: Option<std::sync::Arc<crate::serve::brownout::BrownoutWiring>>,
    /// P0-2 (the response-envelope side): the largest `/__data` envelope
    /// the pipeline answers (`Limits::max_data_envelope`, the request
    /// `max_body` twin). The data-route stage threads it into
    /// `RouteDataArgs::max_envelope`; an envelope past it refuses with
    /// the minted `DSP-117` instead of riding the wire.
    pub max_data_envelope: usize,
    /// PERF-04: single-flight coalescing. Concurrent misses for the same L0
    /// key run one render; the rest await the owner's entry.
    pub flight_map: std::sync::Arc<FlightMap>,
    /// §8.5: published once after the wrapper exists; a middleware
    /// `{ rewrite }` re-enters through it (the hop header bounds the loop).
    pub self_dispatch: std::sync::Arc<SelfDispatch>,
    /// V2-F07: the per-process stream-id counter the endpoint tee mints
    /// `stream_id(request_id, counter)` values with.
    pub stream_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// V2-F04: the configured `experiments` (`crates/config`'s
    /// `ExperimentConfig` rows converted to the engine's shapes at boot —
    /// the conversion is wiring code, the config crate must not depend on
    /// dispatch). Empty for the test helper and every experiment-free
    /// config: the stage short-circuits and every L0 key stays
    /// byte-identical to the pre-F04 baseline.
    pub experiments: Vec<oxide_fw_dispatch::experiments::Experiment>,
    /// V2-F09: the boot's search mode — `Some("server")` when the
    /// built-in engine serves (`search.collections` configured AND the
    /// build's index opened, feature `search`); `None` (the inert
    /// default) keeps the client on its static-export fallback: the
    /// render stage emits no flag script and `window.__oxide.search`
    /// stays unset.
    pub search_flag: Option<&'static str>,
    /// V2-F17 / SEC-09: the dev session token the `/__oxide` dev plane is
    /// guarded with, minted at the serve boot when the inspector is
    /// attached (`crates/cli/src/serve/start.rs`). The render stage presents
    /// it to the page through `window.__oxide.devToken` (the
    /// `application/oxide-dev-token` tag) so the cost overlay can speak the
    /// dev-token-guarded `/__oxide/cost` endpoint. `None` — production, or
    /// a dev boot with the inspector off — emits no tag and every page
    /// stays byte-identical.
    pub dev_token: Option<String>,
}
