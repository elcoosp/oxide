//! Framework-agnostic page renderer hook (spec §7).
//!
//! The HTTP pipeline (`Pipeline::serve_request`) dispatches page renders to
//! whatever `PageRenderer` is attached to it. The trait lives here — in the
//! server crate — so that the pipeline never depends on `oxide-jshost`
//! (which would couple the HTTP layer to V8/Node). The framework-specific
//! adapter (`FrameworkHostRenderer` in `oxide-jshost`) implements this trait
//! via a small shim in the CLI crate that also owns the config-loading
//! decision.
//!
//! Layering:
//!
//! ```text
//!   crates/server          →  trait PageRenderer, PageRenderInput  (this file)
//!        ↑ impl
//!   crates/cli              →  AdapterPageRenderer(Arc<FrameworkHostRenderer>)
//!        ↑ uses
//!   Pipeline::with_page_renderer(...)
//! ```
//!
//! A pipeline with no renderer attached behaves exactly as before (the
//! neutral-IR renderer path the CLI has always used). Attaching a renderer
//! is a pure opt-in.

use crate::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Neutral page-render input (spec §7 RenderInput, stripped of the
/// framework-specific parts). Fields map to the JS-side `RenderInput` in
/// `packages/oxide/src/types.ts`; the CLI shim converts this to whatever
/// shape the underlying host expects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageRenderInput {
    /// Matched route id, e.g. `routes/blog/[slug]`.
    pub route: String,
    /// Request URL (path + query).
    pub url: String,
    /// URL params (path segments). Kept ordered so render output is
    /// deterministic across requests.
    pub params: BTreeMap<String, String>,
    /// Loader data as a JSON tree (ODF-decoded by the pipeline).
    pub data: Value,
    /// Neutral head model (spec §16.3) as a JSON object.
    pub head: Value,
}

impl PageRenderInput {
    /// A minimal input for the given route + url. Params empty, data `{}`,
    /// head is an empty model. Useful in tests.
    pub fn new(route: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            route: route.into(),
            url: url.into(),
            params: BTreeMap::new(),
            data: Value::Object(Default::default()),
            head: serde_json::json!({
                "meta": [], "link": [], "style": [], "script": []
            }),
        }
    }
}

/// Object-safe trait the pipeline holds behind `Arc<dyn PageRenderer>`.
///
/// Implementations live where the framework dependency is available
/// (`crates/cli` shims `oxide_jshost::FrameworkHostRenderer`; tests use a
/// pure-Rust fake). The pipeline only sees the trait, so it stays
/// framework- and Node-agnostic.
#[async_trait]
pub trait PageRenderer: Send + Sync {
    /// Human-readable name (`react` / `vue` / `svelte` / `solid` / `fake`).
    /// Surfaces in the inspector (§13.4) and log lines.
    fn name(&self) -> &str;

    /// Render one route to a full HTML document. `Ok` is a complete
    /// `<!doctype html>...` string ready to be sent as the response body.
    /// Errors surface as `OxideError` with a code the pipeline maps to a
    /// status (§10.2 step 5).
    async fn render(&self, input: PageRenderInput) -> Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A trivial renderer that echoes the route back. No Node, no framework.
    struct EchoRenderer;

    #[async_trait]
    impl PageRenderer for EchoRenderer {
        fn name(&self) -> &str {
            "echo"
        }
        async fn render(&self, input: PageRenderInput) -> Result<String> {
            Ok(format!(
                "<!doctype html><html><body><h1>route={}</h1></body></html>",
                input.route
            ))
        }
    }

    #[tokio::test]
    async fn input_new_defaults_shape() {
        let i = PageRenderInput::new("routes/index", "/");
        assert_eq!(i.route, "routes/index");
        assert_eq!(i.url, "/");
        assert!(i.params.is_empty());
        assert_eq!(i.data, serde_json::json!({}));
        assert_eq!(i.head["meta"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn trait_is_object_safe() {
        let r: Arc<dyn PageRenderer> = Arc::new(EchoRenderer);
        assert_eq!(r.name(), "echo");
        let html = r.render(PageRenderInput::new("routes/a", "/a")).await.unwrap();
        assert!(html.contains("route=routes/a"), "{html}");
    }

    #[tokio::test]
    async fn input_round_trips_through_serde() {
        let mut i = PageRenderInput::new("routes/blog/[slug]", "/blog/hi");
        i.params.insert("slug".into(), "hi".into());
        i.data = serde_json::json!({ "author": "Ada" });
        let s = serde_json::to_string(&i).unwrap();
        let back: PageRenderInput = serde_json::from_str(&s).unwrap();
        assert_eq!(back, i);
    }

    #[test]
    fn input_serializes_to_js_shaped_json() {
        let i = PageRenderInput::new("routes/index", "/");
        let v = serde_json::to_value(&i).unwrap();
        // The JS-side RenderInput expects these keys at the top level.
        assert!(v.get("route").is_some());
        assert!(v.get("url").is_some());
        assert!(v.get("params").is_some());
        assert!(v.get("data").is_some());
        assert!(v.get("head").is_some());
    }
}
