//! Bridge from `oxide_jshost`'s framework renderer to the server's
//! framework-agnostic `oxide_server::PageRenderer` trait (spec §7).
//!
//! Layering:
//!   - `oxide_server::PageRenderer` — what `Pipeline`/dispatch consumes
//!   - `oxide_jshost::FrameworkRenderer` — what talks to the Node worker
//!   - `AdapterPageRenderer` (this module) — impls the former around the
//!     latter, converting input shapes and the error type
//!
//! `build_adapter_renderer` is the concrete constructor used by `cmd_dev`:
//! it spins up a `NodeHost` worker, eagerly loads the project's
//! `oxide.config.{ts,mjs,js}` through `FrameworkHostRenderer::new`, and
//! returns an `Arc<dyn PageRenderer>` ready to hand to the pipeline.

use async_trait::async_trait;
use oxide_jshost::{FrameworkRenderInput, FrameworkRenderer};
use oxide_server::{PageRenderInput, PageRenderer};
use std::sync::Arc;

/// Wraps any `FrameworkRenderer` as an `oxide_server::PageRenderer`.
///
/// The only translation is input-shape: `PageRenderInput.params` is a typed
/// `BTreeMap<String, String>`, while `FrameworkRenderInput.params` is a
/// JSON `Value`. The rest (`route`, `url`, `data`, `head`) passes through
/// unchanged. Errors propagate with the same `OxideError` type.
pub struct AdapterPageRenderer {
    inner: Arc<dyn FrameworkRenderer>,
}

impl AdapterPageRenderer {
    /// Construct from an already-loaded framework renderer. `cmd_dev` uses
    /// `build_adapter_renderer` to produce the inner value; tests can
    /// substitute a fake.
    pub fn new(inner: Arc<dyn FrameworkRenderer>) -> Self {
        Self { inner }
    }

    /// Access to the wrapped renderer (for tests and introspection).
    pub fn inner(&self) -> &Arc<dyn FrameworkRenderer> {
        &self.inner
    }
}

#[async_trait]
impl PageRenderer for AdapterPageRenderer {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn render(
        &self,
        input: PageRenderInput,
    ) -> oxide_server::Result<String> {
        // `BTreeMap<String, String>` → `serde_json::Value::Object`. The
        // serialization can't fail for this input shape (string keys and
        // values always map to JSON), but we degrade to `{}` on the
        // impossible path rather than panicking.
        let params = serde_json::to_value(&input.params)
            .unwrap_or_else(|_| serde_json::json!({}));

        let fw_input = FrameworkRenderInput {
            route: input.route,
            url: input.url,
            params,
            data: input.data,
            head: input.head,
        };

        self.inner.render(fw_input).await
    }
}

/// Build a `PageRenderer` for a project's `oxide.config.{ts,mjs,js}`.
///
/// Spins up a Node worker, loads the config through `FrameworkHostRenderer`
/// (which eagerly evaluates the config and extracts its framework adapter),
/// and wraps the result in `AdapterPageRenderer`.
///
/// Errors surface with the worker's `JSH-FRAMEWORK` code as an `anyhow`
/// error (never fatal here — `cmd_dev` warns and falls back to the IR path).
pub async fn build_adapter_renderer(
    config_path: &std::path::Path,
) -> anyhow::Result<Arc<dyn PageRenderer>> {
    use oxide_jshost::{FrameworkHostRenderer, JsHost, NodeHost, PoolConfig};

    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool = host
        .tooling_pool(PoolConfig::default())
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    let host_renderer = FrameworkHostRenderer::new(
        pool,
        config_path.to_string_lossy().to_string(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    let inner: Arc<dyn FrameworkRenderer> = Arc::new(host_renderer);
    Ok(Arc::new(AdapterPageRenderer::new(inner)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_jshost::FrameworkRenderInput;

    /// A framework renderer that echoes what it received — lets tests assert
    /// the input conversion (typed `params` → JSON) without Node.
    struct EchoFramework;

    #[async_trait]
    impl FrameworkRenderer for EchoFramework {
        fn name(&self) -> &str {
            "echo-fw"
        }
        async fn render(
            &self,
            input: FrameworkRenderInput,
        ) -> oxide_jshost::Result<String> {
            let params_json = serde_json::to_string(&input.params).unwrap_or_default();
            Ok(format!(
                "<!doctype html><html><body><h1>route={}</h1><pre>{}</pre></body></html>",
                input.route, params_json
            ))
        }
    }

    /// A renderer that always errors — verifies the error path.
    struct BoomFramework;

    #[async_trait]
    impl FrameworkRenderer for BoomFramework {
        fn name(&self) -> &str {
            "boom-fw"
        }
        async fn render(
            &self,
            _input: FrameworkRenderInput,
        ) -> oxide_jshost::Result<String> {
            Err(oxide_jshost::err(
                "JSH-FRAMEWORK",
                "simulated render failure",
            ))
        }
    }

    #[test]
    fn adapter_reports_inner_name() {
        let r = AdapterPageRenderer::new(Arc::new(EchoFramework));
        assert_eq!(r.name(), "echo-fw");
    }

    #[tokio::test]
    async fn adapter_forwards_input_and_returns_html() {
        let r = AdapterPageRenderer::new(Arc::new(EchoFramework));
        let mut input = PageRenderInput::new("routes/blog/[slug]", "/blog/hello");
        input.params.insert("slug".into(), "hello".into());
        input.data = serde_json::json!({ "title": "Post" });

        let html = r.render(input).await.expect("render ok");
        assert!(html.contains("route=routes/blog/[slug]"), "{html}");
        assert!(html.contains(r#""slug":"hello""#), "params converted: {html}");
    }

    #[tokio::test]
    async fn adapter_converts_empty_params_to_json_object() {
        let r = AdapterPageRenderer::new(Arc::new(EchoFramework));
        let html = r
            .render(PageRenderInput::new("routes/index", "/"))
            .await
            .unwrap();
        assert!(html.contains("<pre>{}</pre>"), "empty params → `{{}}`: {html}");
    }

    #[tokio::test]
    async fn adapter_propagates_render_error_unchanged() {
        let r = AdapterPageRenderer::new(Arc::new(BoomFramework));
        let err = r
            .render(PageRenderInput::new("routes/x", "/x"))
            .await
            .expect_err("must error");
        assert_eq!(err.0.code, "JSH-FRAMEWORK");
        assert!(err.0.message.contains("simulated render failure"));
    }

    #[tokio::test]
    async fn adapter_is_object_safe_for_arc_dyn() {
        let r: Arc<dyn PageRenderer> = Arc::new(AdapterPageRenderer::new(Arc::new(EchoFramework)));
        assert_eq!(r.name(), "echo-fw");
        let html = r
            .render(PageRenderInput::new("routes/index", "/"))
            .await
            .unwrap();
        assert!(html.contains("route=routes/index"));
    }
}
