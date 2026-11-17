//! V2-F17 (the server half): `GET /__oxide/cost?src=<path>` — the
//! component-cost dev-plane endpoint the §V2.J map advertises next to the
//! open-in-editor surface (`/__oxide/open, /__oxide/cost` — dev plane,
//! dev-token guarded, D41).
//!
//! The spec's dev overlay (`?ox-cost` / Alt+Shift+O) highlights the
//! hovered component and lists the three heaviest; this endpoint is the
//! wire half that feeds it. The spec's record is
//! `{ bytes, gzip, chunk, renderMsP50, cache }`:
//!
//! - `bytes`/`gzip` — the module's rendered size and its gzip estimate
//!   (the bundler's module map is the origin of truth);
//! - `chunk` — the client chunk the file landed in;
//! - `renderMsP50` — the p50 server render time the adapter measured
//!   (React's `<Profiler>` wrap, Vue's `app.config.performance`); `null`
//!   for the adapters that show bytes only — the overlay renders the
//!   "render time unavailable" note off this;
//! - `cache` — the cache tier the last render answered from, when the
//!   pipeline recorded one.
//!
//! The endpoint is the WIRE CONTRACT only: the data rides the injected
//! [`CostSource`] seam (the build-side adapter that feeds the bundler's
//! module map and the render profiler is the caller's — the recorded
//! remaining half of F17 is the compiler's `data-ox-src` attribute
//! injection and the overlay UI). A source that does not know the path
//! answers the documented 404 — the endpoint never fabricates a record
//! the build did not produce. The endpoint only exists when attached
//! (the inspector's posture): [`crate::Pipeline::cost_endpoint`] is
//! `None` in the default pipeline, the unattached shape falls through to
//! the normal dispatch, and production is byte-identical.

use std::sync::Arc;

use crate::{OxideResponse, percent_decode};

/// The §V2.J dev-plane path the endpoint serves (the map row shares the
/// open-in-editor surface).
pub const COST_ENDPOINT_PATH: &str = "/__oxide/cost";

/// One component's cost record — the spec's `{ bytes, gzip, chunk,
/// renderMsP50, cache }` shape, camelCase on the wire (the §14 convention;
/// the render lives in [`CostEndpoint::serve`]'s `json!`).
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentCost {
    /// The module's rendered size in bytes.
    pub bytes: u64,
    /// The gzip estimate (the bundler compresses the rendered code once).
    pub gzip: u64,
    /// The client chunk the file landed in.
    pub chunk: String,
    /// The p50 server render time in ms, when the adapter measured it —
    /// `None` for the bytes-only adapters (Svelte, Solid). Serialized as
    /// `renderMsP50` (the wire shape in `serve`).
    pub render_ms_p50: Option<f64>,
    /// The cache tier the last render answered from, when recorded.
    pub cache: Option<String>,
}

/// The data seam: the build-side feed the dev boot attaches (the
/// production adapter reads the bundler's module map and the render
/// profiler's ledger; tests and embedded hosts inject fakes).
pub trait CostSource: Send + Sync {
    /// The record for one project-relative source path, or `None` when the
    /// build produced none (the endpoint answers the documented 404).
    fn cost(&self, src: &str) -> Option<ComponentCost>;
}

impl ComponentCost {
    /// The bytes-only record — the spec's Svelte/Solid shape ("Svelte and
    /// Solid show bytes only, and the overlay says so"): the render
    /// (`renderMsP50`) and cache halves stay `None`, the overlay renders
    /// its "render time unavailable" note off the null. The shape the
    /// bundler's `source_costs` triple (`bytes`, `gzip`, `chunk` — the
    /// build-side engine carries no render or cache data) maps into.
    #[must_use]
    pub fn bytes_only(bytes: u64, gzip: u64, chunk: impl Into<String>) -> Self {
        Self {
            bytes,
            gzip,
            chunk: chunk.into(),
            render_ms_p50: None,
            cache: None,
        }
    }
}

/// The snapshot-backed production adapter: the build's cost records held
/// in memory, keyed by the root-relative source spelling the `?src=`
/// query and the `data-ox-src` stamp share. The dev boot feeds it from
/// the bundler's `source_costs` snapshot (mapped through
/// [`ComponentCost::bytes_only`] — the render and cache halves belong to
/// the profiler and the tier map, which ride a different adapter), and
/// the endpoint answers hits from it; anything the build did not
/// produce is the documented 404, never a fabricated record.
#[derive(Debug, Clone, Default)]
pub struct SnapshotCostSource {
    records: std::collections::BTreeMap<String, ComponentCost>,
}

impl SnapshotCostSource {
    /// A snapshot over the given records (keyed by the `?src=` spelling).
    #[must_use]
    pub fn new(records: std::collections::BTreeMap<String, ComponentCost>) -> Self {
        Self { records }
    }

    /// A snapshot from `(source, record)` pairs — the bundler's
    /// `Vec<SourceCost>` shape maps one-to-one:
    /// `(sc.source, ComponentCost::bytes_only(sc.bytes, sc.gzip,
    /// sc.chunk))`.
    #[must_use]
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, ComponentCost)>) -> Self {
        Self {
            records: pairs.into_iter().collect(),
        }
    }

    /// The number of records the snapshot carries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the snapshot carries no records (the no-build shape —
    /// every query misses, the endpoint's 404 all the way down).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl CostSource for SnapshotCostSource {
    fn cost(&self, src: &str) -> Option<ComponentCost> {
        self.records.get(src).cloned()
    }
}

/// The profiler + tier-map adapter — the layered production `CostSource`
/// the §V2-F17 record's remaining halves ride: the build's snapshot
/// source (the [`SnapshotCostSource`] shape, its records bytes-only)
/// decorated with the render profiler's per-source p50 (`renderMsP50`)
/// and the pipeline's cache-tier map (`cache`), each keyed by the same
/// root-relative `?src=` spelling.
///
/// Layering rules (the decorator decorates, it never contradicts or
/// fabricates):
/// - the base record is authoritative — a half the snapshot already
///   carries is left exactly as the build wrote it;
/// - a base miss stays a miss — a path only the profiler measured is the
///   endpoint's documented 404, never a synthesized record;
/// - empty maps leave the base's records byte-identical (the bytes-only
///   shape).
#[derive(Clone)]
pub struct ProfiledCostSource {
    base: Arc<dyn CostSource>,
    render_p50: std::collections::BTreeMap<String, f64>,
    tiers: std::collections::BTreeMap<String, String>,
}

impl std::fmt::Debug for ProfiledCostSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The base is an object-safe trait object (no `Debug` supertrait —
        // the injected fakes and adapters carry no debug contract); the
        // maps are the adapter's own state and print in full.
        f.debug_struct("ProfiledCostSource")
            .field("base", &"<dyn CostSource>")
            .field("render_p50", &self.render_p50)
            .field("tiers", &self.tiers)
            .finish()
    }
}

impl ProfiledCostSource {
    /// Decorate `base` with the profiler's per-source p50 map and the
    /// pipeline's cache-tier map.
    #[must_use]
    pub fn new(
        base: Arc<dyn CostSource>,
        render_p50: std::collections::BTreeMap<String, f64>,
        tiers: std::collections::BTreeMap<String, String>,
    ) -> Self {
        Self {
            base,
            render_p50,
            tiers,
        }
    }

    /// The boot shape: `(source, p50)` and `(source, tier)` iterator
    /// pairs — the profiler ledger's and the tier recorder's own rows —
    /// collected into the maps.
    #[must_use]
    pub fn from_maps(
        base: Arc<dyn CostSource>,
        render_p50: impl IntoIterator<Item = (String, f64)>,
        tiers: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self {
            base,
            render_p50: render_p50.into_iter().collect(),
            tiers: tiers.into_iter().collect(),
        }
    }
}

impl CostSource for ProfiledCostSource {
    fn cost(&self, src: &str) -> Option<ComponentCost> {
        let mut record = self.base.cost(src)?;
        if record.render_ms_p50.is_none() {
            record.render_ms_p50 = self.render_p50.get(src).copied();
        }
        if record.cache.is_none() {
            record.cache = self.tiers.get(src).cloned();
        }
        Some(record)
    }
}

/// The dev-side feed: the build's PERSISTED records file (the bundler's
/// `write_source_costs` rows — the `{ source, bytes, gzip, chunk }`
/// `client.costs.json` beside the client manifest), stat-checked per
/// query. The dev boot cannot hold the build's `BundleOutput` across
/// rebuilds, so it attaches this source once and a rebuild's rewrite of
/// the file is picked up by the NEXT query — the endpoint serves the
/// current build's costs without a re-attach. The staleness check is the
/// cargo/make `(mtime, length)` stat pair: an mtime-only check is flaky
/// where the timestamp tick is coarser than a fast rewrite, and a
/// rewrite that moves neither half carries identical content anyway
/// (the cached records ARE the file's records until the next move).
///
/// Honesty rules (the endpoint's contract, unmodified):
///
/// - a hit maps the row one-to-one into [`ComponentCost::bytes_only`] —
///   a `null` chunk is the honest empty string (no chunk claims the
///   module), and the render/cache halves stay `None` (they belong to
///   the profiler and the tier-map adapters);
/// - a missing file is the no-build shape — the documented miss, every
///   query `None`, no log noise (the endpoint's 404 all the way down);
/// - an unreadable or unparsable file warns once per mtime move (the
///   path and the parse failure named) and answers misses until the
///   file is fixed — the dev loop stays up, never a fabricated record;
/// - a vanished file (previously good) warns and clears — stale rows
///   never outlive the file that carried them.
///
/// The steady-state hit is one `stat` plus the cached lookup (the
/// `cost_source_file` bench group measures the per-query shape).
pub struct FileCostsSource {
    path: std::path::PathBuf,
    cache: std::sync::Mutex<Option<FileCostsCache>>,
}

/// The cached load: the file's `(mtime, length)` stat pair at the parse
/// (the cargo/make staleness pair — see `cost`) and the mapped records.
#[derive(Debug)]
struct FileCostsCache {
    stat: (std::time::SystemTime, u64),
    records: std::collections::BTreeMap<String, ComponentCost>,
}

/// One file row — the bundler's `write_source_costs` shape (the strict
/// contract: `source`/`bytes`/`gzip` required, the chunk nullable).
#[derive(serde::Deserialize)]
struct CostRecordJson {
    source: String,
    bytes: u64,
    gzip: u64,
    chunk: Option<String>,
}

impl std::fmt::Debug for FileCostsSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The cache is the adapter's internal state (a mutex over the
        // last load); the path is the identity the boot attaches.
        f.debug_struct("FileCostsSource")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl FileCostsSource {
    /// A source over the records file at `path` (the boot attaches the
    /// `client.costs.json` spelling beside the client manifest; the file
    /// may not exist yet — the first rebuild creates it).
    #[must_use]
    pub fn from_path(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            cache: std::sync::Mutex::new(None),
        }
    }
}

impl CostSource for FileCostsSource {
    fn cost(&self, src: &str) -> Option<ComponentCost> {
        let mut guard = self.cache.lock().expect("costs-file cache mutex poisoned");
        // The staleness check is the cargo/make stat PAIR — mtime AND
        // length. An mtime-only check is flaky on filesystems whose
        // timestamp tick is coarser than a fast rewrite (two writes can
        // land in one tick); the length half catches those, and a
        // rewrite that moves NEITHER (a same-length write inside one
        // tick) carries identical content anyway — the cached records
        // ARE the file's records until the next mtime move.
        let stat = std::fs::metadata(&self.path)
            .ok()
            .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
        if let (Some(cache), Some(s)) = (guard.as_ref(), &stat)
            && cache.stat == *s
        {
            return cache.records.get(src).cloned();
        }
        match stat {
            // The no-build shape (or the file vanished): the documented
            // miss. A previously-good cache is cleared so stale rows
            // never outlive the file that carried them.
            None => {
                if guard.is_some() {
                    tracing::warn!(
                        path = %self.path.display(),
                        "costs file vanished; the cost endpoint answers misses until the next build"
                    );
                }
                *guard = None;
                None
            }
            Some(s) => {
                // The stat pair is captured BEFORE the read: a rewrite
                // racing the load moves the pair again, so the next
                // query reloads instead of serving a half-read file.
                match load_cost_records(&self.path) {
                    Ok(records) => {
                        *guard = Some(FileCostsCache { stat: s, records });
                        guard.as_ref().and_then(|c| c.records.get(src)).cloned()
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %self.path.display(),
                            error = %e,
                            "costs file unreadable; the cost endpoint answers misses until the file is fixed"
                        );
                        *guard = Some(FileCostsCache {
                            stat: s,
                            records: std::collections::BTreeMap::new(),
                        });
                        None
                    }
                }
            }
        }
    }
}

/// Read and map the records file: the caller captured the `(mtime,
/// length)` stat pair BEFORE this read (the race rule — a rewrite racing
/// the load moves the pair again, so the next query reloads); the strict
/// row shape maps one-to-one into the bytes-only records.
fn load_cost_records(
    path: &std::path::Path,
) -> std::io::Result<std::collections::BTreeMap<String, ComponentCost>> {
    let raw = std::fs::read_to_string(path)?;
    let rows: Vec<CostRecordJson> = serde_json::from_str(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cost records parse failed: {e}"),
        )
    })?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.source,
                ComponentCost::bytes_only(row.bytes, row.gzip, row.chunk.unwrap_or_default()),
            )
        })
        .collect())
}

/// The endpoint state: the injected source. The dev boot attaches one via
/// [`crate::Pipeline::with_cost_endpoint`].
pub struct CostEndpoint {
    source: Arc<dyn CostSource>,
}

impl CostEndpoint {
    /// An endpoint over one cost source.
    #[must_use]
    pub fn new(source: Arc<dyn CostSource>) -> Self {
        Self { source }
    }

    /// The total request → response function (everything but the
    /// dev-token guard, which is the pipeline arm's — the token never
    /// reaches this module).
    ///
    /// - non-`GET` → `405` + `allow: GET` (the nav/open contract).
    /// - a missing/empty `src` → `400` with the usage line.
    /// - an unknown `src` → `404` (the source's documented miss).
    /// - a hit → `200` JSON `ComponentCost`.
    #[must_use]
    pub fn serve(&self, method: &str, query: &str) -> OxideResponse {
        if !method.eq_ignore_ascii_case("GET") {
            return error_response(405, "the cost endpoint answers GET", Some(("allow", "GET")));
        }
        let src = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("src="))
            .map_or_else(String::new, percent_decode);
        if src.is_empty() {
            return error_response(
                400,
                "missing or empty `src`: the endpoint is `GET /__oxide/cost?src=<path>`",
                None,
            );
        }
        let Some(cost) = self.source.cost(&src) else {
            return error_response(
                404,
                &format!("no cost record for {src} (the build did not produce one)"),
                None,
            );
        };
        tracing::info!(src = %src, bytes = cost.bytes, gzip = cost.gzip, "cost record served");
        OxideResponse::json(
            200,
            serde_json::json!({
                "bytes": cost.bytes,
                "gzip": cost.gzip,
                "chunk": cost.chunk,
                "renderMsP50": cost.render_ms_p50,
                "cache": cost.cache,
            }),
        )
    }
}

/// The endpoint's error spelling: a plain-text body with the status (the
/// nav/open rejection shape).
fn error_response(status: u16, msg: &str, extra: Option<(&str, &str)>) -> OxideResponse {
    let mut resp = OxideResponse::new(status, bytes::Bytes::from(msg.to_string()))
        .with_header("content-type", "text/plain; charset=utf-8");
    if let Some((k, v)) = extra {
        resp = resp.with_header(k, v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_const_is_the_dev_plane_spelling() {
        assert_eq!(COST_ENDPOINT_PATH, "/__oxide/cost");
    }
}
