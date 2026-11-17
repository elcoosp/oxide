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
            .map(percent_decode)
            .unwrap_or_default();
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
