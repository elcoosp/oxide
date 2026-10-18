//! Framework-agnostic loader runner hook (spec §9.1).
//!
//! The HTTP pipeline uses a `LoaderRunner` to evaluate a route's
//! `page.server.ts` `load(event)` function before dispatching a render. The
//! trait lives here — in the server crate — so that the pipeline never
//! depends on `oxide-jshost` (which would couple the HTTP layer to Node).
//! The concrete `LoaderRunner` is provided by the CLI shim, which wraps
//! `oxide_jshost::LoaderHost`.
//!
//! Layering mirrors `PageRenderer`:
//!
//! ```text
//!   crates/server         →  trait LoaderRunner, LoaderRunInput, LoaderOutput (this file)
//!        ↑ impl
//!   crates/cli             →  AdapterLoader(Arc<LoaderHost>)
//!        ↑ uses
//!   build_dispatch(...)     →  Option<Arc<dyn LoaderRunner>> per request
//! ```
//!
//! Errors and redirects: a `load` that throws `error(404)` or
//! `redirect('/login')` returns `LoaderOutput` with a non-200 status and
//! (for redirects) a `location`. The pipeline short-circuits on non-200
//! and writes the corresponding response.

use crate::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Per-request input to a loader (spec §9.1 LoadEvent subset).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoaderRunInput {
    /// Absolute filesystem path to the module exporting `load`.
    pub module_path: String,
    /// Matched route id (e.g. `routes/blog/[slug]`).
    pub route: String,
    /// Request URL (path + query).
    pub url: String,
    /// HTTP method.
    pub method: String,
    /// URL params (path segments).
    pub params: BTreeMap<String, String>,
    /// Request headers (ordered; case-insensitive lookup by consumers).
    pub headers: Vec<(String, String)>,
    /// SPEC-10: the resolved locale (path prefix → Accept-Language → default).
    /// Empty when the project has no `i18n` config.
    #[serde(default)]
    pub locale: String,
    /// §8.1 middleware.ts / §9.1: request-scoped locals injected by
    /// middleware. Absent when no middleware runs (the loader sees `{}`).
    #[serde(default)]
    pub locals: serde_json::Value,
}

impl LoaderRunInput {
    /// A minimal input for the given module + route + url. Method is GET,
    /// params empty, headers empty.
    pub fn new(
        module_path: impl Into<String>,
        route: impl Into<String>,
        url: impl Into<String>,
    ) -> Self {
        Self {
            module_path: module_path.into(),
            route: route.into(),
            url: url.into(),
            method: "GET".into(),
            params: BTreeMap::new(),
            headers: Vec::new(),
            locale: String::new(),
            locals: serde_json::json!({}),
        }
    }
}

/// Result of running a loader. `status` is 200 on success, 3xx on redirect,
/// 4xx/5xx on `error(...)`. `location` is set only for redirects. `data`
/// is the (JSON) loader return value; on error/redirect it carries at most
/// a `message` for diagnostics.
///
/// `set_cookies` and `headers` carry side effects a load produced through the
/// §9.2 event surface (`event.cookies.set(...)`, `event.setHeaders(...)`). They
/// are applied to the eventual HTTP response by the pipeline; keeping them on
/// the output (rather than threading a mutable response through the runner)
/// is what lets the `LoaderRunner` trait stay a pure input → output call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LoaderOutput {
    pub status: u16,
    pub location: Option<String>,
    pub data: Value,
    /// Raw `Set-Cookie` header values, in the order the load set them.
    #[serde(default)]
    pub set_cookies: Vec<String>,
    /// Headers the load added via `setHeaders`, in insertion order.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// §7.1 / §9.2: resolved `defer(promise)` values, keyed by the numeric id
    /// the loader's `defer()` assigned. The CLI injects `<script data-ox-d>`
    /// resolution scripts so client `DeferredClient` slots resolve. Empty
    /// when the load had no deferred data.
    #[serde(default)]
    pub deferred: std::collections::BTreeMap<String, serde_json::Value>,
}

impl LoaderOutput {
    /// Success with the given data and no side-effect headers.
    pub fn ok(data: Value) -> Self {
        Self {
            status: 200,
            data,
            deferred: std::collections::BTreeMap::new(),
            ..Default::default()
        }
    }
    /// Whether this output should short-circuit the render (non-200).
    pub fn short_circuits(&self) -> bool {
        self.status < 200 || self.status >= 300
    }
}

/// Object-safe trait the pipeline holds behind `Arc<dyn LoaderRunner>`.
#[async_trait]
pub trait LoaderRunner: Send + Sync {
    /// Backend name (`node` for the worker-backed runner; `fake` in tests).
    fn name(&self) -> &str;
    /// Run the loader for one request. `Ok(LoaderOutput)` even for
    /// loader-thrown `error()` / `redirect()` — those are captured in the
    /// output's status/location, not surfaced as `Err`. `Err` means the
    /// runner itself failed (module not found, syntax error, worker crash).
    async fn run(&self, input: LoaderRunInput) -> Result<LoaderOutput>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct OkLoader;
    #[async_trait]
    impl LoaderRunner for OkLoader {
        fn name(&self) -> &str { "ok" }
        async fn run(&self, input: LoaderRunInput) -> Result<LoaderOutput> {
            Ok(LoaderOutput::ok(serde_json::json!({
                "route": input.route,
                "slug": input.params.get("slug").cloned().unwrap_or_default(),
            })))
        }
    }

    struct HttpErrLoader;
    #[async_trait]
    impl LoaderRunner for HttpErrLoader {
        fn name(&self) -> &str { "http-err" }
        async fn run(&self, _input: LoaderRunInput) -> Result<LoaderOutput> {
            Ok(LoaderOutput { status: 404, data: serde_json::json!({"message":"not found"}), ..Default::default() })
        }
    }

    #[test]
    fn input_new_defaults_shape() {
        let i = LoaderRunInput::new("/abs/page.server.ts", "routes/index", "/");
        assert_eq!(i.module_path, "/abs/page.server.ts");
        assert_eq!(i.route, "routes/index");
        assert_eq!(i.url, "/");
        assert_eq!(i.method, "GET");
        assert!(i.params.is_empty());
        assert!(i.headers.is_empty());
    }

    #[test]
    fn ok_short_circuits_is_false() {
        assert!(!LoaderOutput::ok(serde_json::json!({})).short_circuits());
    }

    #[test]
    fn non_200_short_circuits() {
        assert!(LoaderOutput { status: 404, data: serde_json::json!({}), ..Default::default() }.short_circuits());
        assert!(LoaderOutput { status: 303, location: Some("/".into()), data: serde_json::json!({}), ..Default::default() }.short_circuits());
        assert!(LoaderOutput { status: 500, data: serde_json::json!({}), ..Default::default() }.short_circuits());
    }

    #[tokio::test]
    async fn trait_is_object_safe() {
        let r: Arc<dyn LoaderRunner> = Arc::new(OkLoader);
        assert_eq!(r.name(), "ok");
        let mut input = LoaderRunInput::new("/m.ts", "routes/blog/[slug]", "/blog/hi");
        input.params.insert("slug".into(), "hi".into());
        let out = r.run(input).await.unwrap();
        assert_eq!(out.status, 200);
        assert_eq!(out.data["route"], "routes/blog/[slug]");
        assert_eq!(out.data["slug"], "hi");
    }

    #[tokio::test]
    async fn error_and_redirect_are_ok_not_err() {
        let r: Arc<dyn LoaderRunner> = Arc::new(HttpErrLoader);
        let out = r.run(LoaderRunInput::new("/m.ts", "routes/x", "/x")).await.unwrap();
        assert_eq!(out.status, 404);
        assert!(out.short_circuits());
    }

    #[test]
    fn loader_output_round_trips_side_effect_fields() {
        // §9.2 / §16.6: cookies and headers a load set survive serde, so the
        // CLI shim can hand them from the worker to the response builder.
        let out = LoaderOutput {
            status: 303,
            location: Some("/login".into()),
            data: serde_json::json!({"message": "redirect"}),
            set_cookies: vec![
                "sid=abc; Path=/; HttpOnly; SameSite=Lax".into(),
                "theme=dark; Path=/".into(),
            ],
            headers: vec![("x-oxide-visits".into(), "7".into())],
            deferred: std::collections::BTreeMap::new(),
        };
        let s = serde_json::to_string(&out).unwrap();
        let back: LoaderOutput = serde_json::from_str(&s).unwrap();
        assert_eq!(back, out);
        assert_eq!(back.set_cookies.len(), 2);
        assert_eq!(back.headers[0].0, "x-oxide-visits");
    }

    #[test]
    fn loader_output_defaults_have_no_side_effects() {
        // `ok` and `Default` must not invent cookies/headers.
        let o = LoaderOutput::ok(serde_json::json!({"a": 1}));
        assert!(o.set_cookies.is_empty());
        assert!(o.headers.is_empty());
        // An older worker response without the new fields still deserializes.
        let old = r#"{"status":200,"location":null,"data":{}}"#;
        let parsed: LoaderOutput = serde_json::from_str(old).unwrap();
        assert!(parsed.set_cookies.is_empty() && parsed.headers.is_empty());
    }

    #[test]
    fn input_round_trips_through_serde() {
        let mut i = LoaderRunInput::new("/abs.ts", "routes/blog/[slug]", "/blog/hi");
        i.params.insert("slug".into(), "hi".into());
        i.headers.push(("cookie".into(), "a=b".into()));
        let s = serde_json::to_string(&i).unwrap();
        let back: LoaderRunInput = serde_json::from_str(&s).unwrap();
        assert_eq!(back, i);
    }
}
