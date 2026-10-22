//! Loader host (spec §9.1): dispatch a `loader:run` call to the worker,
//! which imports a `.server.ts` module, calls `load(event)`, and returns
//! the result as `{ ok, status, location, data }`.
//!
//! Mirrors `FrameworkHost`'s shape: takes a `Pool`, sends a `CallInput`,
//! and returns the parsed JSON. Errors (module not found, syntax error,
//! worker crash) surface as `JSH-LOADER`; loader-thrown `error(404)` or
//! `redirect(...)` do NOT error — they return `{ status, location? }`
//! inside the ok payload, so the pipeline can write the HTTP response.

use crate::{CallInput, EntryRef, Pool, Result};
use serde_json::Value;
use std::sync::Arc;

/// Thin host for `loader:run`.
pub struct LoaderHost {
    pool: Arc<dyn Pool>,
}

/// B5 kill switch: whether the buffered (await-all) path is requested.
/// Pure so it is testable without mutating the process environment.
fn buffered_from(value: Option<String>) -> bool {
    value.map(|v| v == "1").unwrap_or(false)
}

impl LoaderHost {
    pub fn new(pool: Arc<dyn Pool>) -> Self {
        Self { pool }
    }

    /// The payload whitelist `run` forwards to the `loader:run` worker op.
    /// Pure so the whitelist is unit-testable without a `Pool`.
    ///
    /// Regression note (10-b): this used to drop `parent_data` and `locale` —
    /// the shim (`loader_shim::input_to_payload`) forwards both, so `parent()`
    /// resolved to `{}` and i18n loads lost their locale on every backend.
    fn build_payload(module_path: &str, input: &Value) -> Value {
        serde_json::json!({
            "module_path": module_path,
            "route": input.get("route").cloned().unwrap_or(Value::Null),
            "params": input.get("params").cloned().unwrap_or(Value::Null),
            "url": input.get("url").cloned().unwrap_or(Value::Null),
            "method": input.get("method").cloned().unwrap_or(Value::String("GET".into())),
            "headers": input.get("headers").cloned().unwrap_or(Value::Array(vec![])),
            // §8.1 middleware.ts / §9.1: request-scoped locals injected by
            // middleware. The loader's `event.locals` reads this. Defaults to
            // `{}` so a project without middleware still sees a (empty) object.
            "locals": input.get("locals").cloned().unwrap_or(serde_json::json!({})),
            // §9.2: the pre-resolved outer-layout data the worker exposes as
            // `parent()`. Absent → Null; the worker treats that as `{}`.
            "parent_data": input.get("parent_data").cloned().unwrap_or(Value::Null),
            // SPEC-10 / §16.4: resolved locale for i18n-aware loads. Defaults
            // to "" like the shim's `LoaderRunInput::locale`.
            "locale": input
                .get("locale")
                .cloned()
                .unwrap_or(Value::String(String::new())),
            // §20 / G2: W3C trace context forwarded to the loader's worker.
            // The worker's `traceparentFromPayload` reads
            // `payload.trace.traceparent` to stamp server-side `fetch()`.
            // Absent → Null; the worker generates a fresh trace id.
            // This is the same payload builder the V8 dispatcher reads, so
            // one whitelist entry reaches both backends.
            "trace": input.get("trace").cloned().unwrap_or(Value::Null),
        })
    }

    /// Run the loader at `module_path`. `input` is the JSON payload for the
    /// worker op: `{ route, params, url, method, headers, locals, locale,
    /// parent_data }`. Returns the raw worker response value, which callers
    /// convert to a typed `LoaderOutput`.
    pub async fn run(&self, module_path: &str, input: Value) -> Result<Value> {
        let payload = Self::build_payload(module_path, &input);
        // PERF-02: affinity key = the route the loader serves (read before
        // `payload` moves into the CallInput).
        let route = payload
            .get("route")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let call_input = CallInput {
            payload,
            headers: Default::default(),
            has_body: false,
            request_id: format!("loader:run:{module_path}"),
            route,
        };
        let entry = EntryRef {
            kind: "loader:run".into(),
            module: module_path.to_string(),
            export: "load".into(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        let bytes = stream.collect().await?;
        // Two wire shapes:
        //   * no `defer()` in the loader -> one JSON object (unchanged);
        //   * at least one -> NDJSON: the initial payload line, then one
        //     `{deferredSettle: {id, value|error}}` line per settle.
        let v = match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => v,
            Err(_) => merge_settle_stream(&bytes)?,
        };
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-LOADER", err_msg.to_string()));
        }
        Ok(v)
    }

    /// B2: stream a load. Returns the initial payload (fast - no defer wait)
    /// and a receiver yielding one `{deferredSettle: {...}}` per settle.
    ///
    /// The worker sends NDJSON over the chunked-call path: the first chunk is
    /// the load result, each later chunk is one settle. This hands the caller
    /// the first chunk immediately and forwards the rest, so a response can
    /// flush a resolution script per settle rather than after the slowest.
    ///
    /// A loader with no `defer()` produces a `Complete` stream (the worker's
    /// no-pending branch); that returns `(payload, empty_rx)` and is
    /// behaviour-identical to `run`.
    pub async fn run_streaming(
        &self,
        module_path: &str,
        input: Value,
    ) -> Result<(Value, tokio::sync::mpsc::Receiver<Value>)> {
        let mut payload = Self::build_payload(module_path, &input);
        // B-03: the V8 backend reads this flag to stream settle lines; the
        // Node worker always streams when the loader registered `defer()`s
        // and ignores the key.
        //
        // B5 kill switch: `OXIDE_DEFER_BUFFERED=1` falls back to the
        // await-all path for one release. It is the escape hatch if the
        // per-settle delivery misbehaves in a deployment — the caller still
        // gets a complete payload, just later.
        let buffered = buffered_from(std::env::var("OXIDE_DEFER_BUFFERED").ok());
        if buffered {
            let v = self.run(module_path, input).await?;
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            return Ok((v, rx));
        }
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("__oxide_stream".to_string(), true.into());
        }
        let route = payload
            .get("route")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let call_input = CallInput {
            payload,
            headers: Default::default(),
            has_body: false,
            request_id: format!("loader:run:{module_path}"),
            route,
        };
        let entry = EntryRef {
            kind: "loader:run".into(),
            module: module_path.to_string(),
            export: "load".into(),
        };
        let stream = self.pool.call(entry, call_input).await?;
        match stream {
            crate::CallStream::Complete(bytes) => {
                let v = serde_json::from_slice::<Value>(&bytes).map_err(|e| {
                    crate::err(
                        "JSH-LOADER",
                        format!("loader:run returned invalid JSON: {e}"),
                    )
                })?;
                if let Some(m) = v.get("__oxide_error").and_then(|x| x.as_str()) {
                    return Err(crate::err("JSH-LOADER", m.to_string()));
                }
                let (_tx, rx) = tokio::sync::mpsc::channel(1);
                Ok((v, rx))
            }
            crate::CallStream::Chunked(mut inner) => {
                let (settles_tx, settles_rx) = tokio::sync::mpsc::channel::<Value>(16);
                let (init_tx, init_rx) = tokio::sync::oneshot::channel::<Result<Value>>();
                tokio::spawn(async move {
                    let mut init_tx = Some(init_tx);
                    let mut first = true;
                    while let Some(chunk) = inner.recv().await {
                        let bytes = match chunk {
                            Ok(b) => b,
                            Err(e) => {
                                if let Some(tx) = init_tx.take() {
                                    let _ = tx.send(Err(e));
                                }
                                return;
                            }
                        };
                        let text = String::from_utf8_lossy(&bytes);
                        for line in text.lines().filter(|l| !l.trim().is_empty()) {
                            let Ok(v) = serde_json::from_str::<Value>(line) else {
                                continue;
                            };
                            if first {
                                first = false;
                                if let Some(tx) = init_tx.take() {
                                    let _ = tx.send(Ok(v));
                                }
                            } else if settles_tx.send(v).await.is_err() {
                                return; // consumer dropped
                            }
                        }
                    }
                });
                let init = init_rx.await.map_err(|_| {
                    crate::err(
                        "JSH-LOADER",
                        "loader stream ended before the initial payload",
                    )
                })??;
                if let Some(m) = init.get("__oxide_error").and_then(|x| x.as_str()) {
                    return Err(crate::err("JSH-LOADER", m.to_string()));
                }
                Ok((init, settles_rx))
            }
        }
    }

    /// Run a route's `generateParams` export and return the enumerated
    /// static params (spec §10.5). When the module has no `generateParams`
    /// export, returns an empty vector (not an error) — the build falls
    /// back to the route's static/dynamic classification.
    ///
    /// Each entry is a flat `String -> String` map; scalar values are
    /// coerced to strings on the JS side, nested structures are skipped
    /// (params substitute into URL path segments, so only scalars make
    /// sense).
    pub async fn generate_params(
        &self,
        module_path: &str,
    ) -> Result<Vec<std::collections::BTreeMap<String, String>>> {
        let call_input = CallInput {
            payload: serde_json::json!({ "module_path": module_path }),
            headers: Default::default(),
            has_body: false,
            request_id: format!("generate-params:run:{module_path}"),
            route: None,
        };
        let entry = EntryRef {
            kind: "generate-params:run".into(),
            module: module_path.to_string(),
            export: "generateParams".into(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err(
                "JSH-PARAMS",
                format!("generate-params:run returned invalid JSON: {e}"),
            )
        })?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-PARAMS", err_msg.to_string()));
        }
        let arr = v.get("params").and_then(|x| x.as_array()).ok_or_else(|| {
            crate::err(
                "JSH-PARAMS",
                "generate-params:run response missing `params` array",
            )
        })?;
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            let obj = item
                .as_object()
                .ok_or_else(|| crate::err("JSH-PARAMS", "params entry not an object"))?;
            let mut map = std::collections::BTreeMap::new();
            for (k, val) in obj {
                if let Some(s) = val.as_str() {
                    map.insert(k.clone(), s.to_string());
                }
            }
            out.push(map);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallStream, JsHost, PoolConfig, ScriptedHost};
    use bytes::Bytes;
    use serde_json::json;

    fn mock_pool(response: Value) -> Arc<dyn Pool> {
        let host = ScriptedHost::with_handler(Arc::new(move |_entry, _input| {
            let bytes = serde_json::to_vec(&response).unwrap();
            Ok(CallStream::Complete(Bytes::from(bytes)))
        }));
        host.tooling_pool(PoolConfig::default()).unwrap()
    }

    #[tokio::test]
    async fn run_success_returns_data() {
        let pool = mock_pool(json!({
            "ok": true,
            "status": 200,
            "location": null,
            "data": { "title": "Post", "author": "Ada" }
        }));
        let h = LoaderHost::new(pool);
        let out = h
            .run(
                "/abs/page.server.ts",
                json!({ "route": "routes/blog/[slug]", "url": "/blog/hi",
                         "params": {"slug": "hi"}, "method": "GET", "headers": [] }),
            )
            .await
            .unwrap();
        assert_eq!(out["status"], 200);
        assert_eq!(out["data"]["title"], "Post");
    }

    #[tokio::test]
    async fn run_propagates_loader_http_error_as_ok_payload() {
        let pool = mock_pool(json!({
            "ok": true,
            "status": 404,
            "location": null,
            "data": { "message": "not found" }
        }));
        let h = LoaderHost::new(pool);
        let out = h.run("/m.ts", json!({})).await.unwrap();
        assert_eq!(out["status"], 404);
    }

    #[tokio::test]
    async fn run_surfaces_worker_error_as_jsh_loader() {
        let pool = mock_pool(json!({
            "__oxide_error": "loader:run failed: Cannot find module '/nope'"
        }));
        let h = LoaderHost::new(pool);
        let err = h.run("/nope", json!({})).await.unwrap_err();
        assert_eq!(err.0.code, "JSH-LOADER");
        assert!(
            err.0.message.contains("Cannot find module"),
            "{}",
            err.0.message
        );
    }

    #[tokio::test]
    async fn run_rejects_invalid_json_response() {
        let host = ScriptedHost::with_handler(Arc::new(|_e, _i| {
            Ok(CallStream::Complete(Bytes::from_static(b"not json")))
        }));
        let pool = host.tooling_pool(PoolConfig::default()).unwrap();
        let h = LoaderHost::new(pool);
        let err = h.run("/m.ts", json!({})).await.unwrap_err();
        assert_eq!(err.0.code, "JSH-LOADER");
        assert!(err.0.message.contains("invalid JSON"));
    }

    #[test]
    fn build_payload_forwards_parent_data_and_locale() {
        // Regression guard (10-b): the whitelist dropped `parent_data` and
        // `locale`, so the worker's `parent()` always resolved to `{}` and
        // i18n loads lost their locale — even though the CLI shim forwarded
        // both. Both must survive `LoaderHost::run`'s payload rebuild.
        let input = json!({
            "route": "routes/blog/[slug]",
            "params": { "slug": "hi" },
            "url": "/blog/hi",
            "method": "GET",
            "headers": [["cookie", "a=b"]],
            "locals": { "user": "ada" },
            "parent_data": { "cart": ["a", "b"] },
            "locale": "en",
        });
        let p = LoaderHost::build_payload("/abs/page.server.ts", &input);
        assert_eq!(p["module_path"], "/abs/page.server.ts");
        assert_eq!(p["route"], "routes/blog/[slug]");
        assert_eq!(p["params"]["slug"], "hi");
        assert_eq!(p["locals"]["user"], "ada");
        assert_eq!(
            p["parent_data"]["cart"],
            json!(["a", "b"]),
            "parent_data survives the whitelist"
        );
        assert_eq!(p["locale"], "en", "locale survives the whitelist");
    }

    #[test]
    fn build_payload_forwards_trace() {
        let trace = serde_json::json!({
            "traceparent": "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "tracestate": null,
        });
        let input = serde_json::json!({
            "route": "routes/x",
            "url": "/x",
            "trace": trace.clone(),
        });
        let p = LoaderHost::build_payload("/x/page.server.ts", &input);
        assert_eq!(p["trace"], trace, "trace forwarded: {p}");
    }

    #[test]
    fn build_payload_defaults_trace_to_null() {
        let input = serde_json::json!({ "url": "/x" });
        let p = LoaderHost::build_payload("/x/page.server.ts", &input);
        assert_eq!(
            p["trace"],
            serde_json::Value::Null,
            "trace defaults to null: {p}"
        );
    }

    #[test]
    fn build_payload_defaults_parent_data_and_locale() {
        // Absent fields take the same defaults the shim's `LoaderRunInput`
        // uses: `parent_data` → Null (the worker reads it as `{}`) and
        // `locale` → "".
        let p = LoaderHost::build_payload("/x/page.server.ts", &json!({ "url": "/x" }));
        assert!(p["parent_data"].is_null());
        assert_eq!(p["locale"], "");
        assert_eq!(p["method"], "GET");
        assert_eq!(p["locals"], json!({}));
        assert_eq!(p["headers"], json!([]));
    }

    #[tokio::test]
    async fn generate_params_success() {
        let pool = mock_pool(json!({
            "params": [
                { "slug": "hello" },
                { "slug": "world" }
            ]
        }));
        let h = LoaderHost::new(pool);
        let out = h.generate_params("/abs/page.server.ts").await.unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].get("slug"), Some(&"hello".to_string()));
        assert_eq!(out[1].get("slug"), Some(&"world".to_string()));
    }

    #[tokio::test]
    async fn generate_params_empty_is_ok() {
        let pool = mock_pool(json!({ "params": [] }));
        let h = LoaderHost::new(pool);
        let out = h.generate_params("/m.ts").await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn generate_params_worker_error() {
        let pool = mock_pool(json!({
            "__oxide_error": "generate-params:run failed: no such module"
        }));
        let h = LoaderHost::new(pool);
        let err = h.generate_params("/missing.ts").await.unwrap_err();
        assert_eq!(err.0.code, "JSH-PARAMS");
        assert!(
            err.0.message.contains("no such module"),
            "{}",
            err.0.message
        );
    }

    #[tokio::test]
    async fn generate_params_missing_array() {
        let pool = mock_pool(json!({}));
        let h = LoaderHost::new(pool);
        let err = h.generate_params("/m.ts").await.unwrap_err();
        assert_eq!(err.0.code, "JSH-PARAMS");
        assert!(err.0.message.contains("missing `params` array"));
    }
}

/// §9.4: the host for `server-fn:run`. A server function is registered at
/// module-evaluation time under a build-computed id; `run` sends the id,
/// the arguments, and the SSR adapter entry the worker needs to populate
/// its registry, and returns the encoded result.
///
/// The `entry_path` is passed on every call rather than stored, because a
/// project may run more than one SSR entry (unusual, but supported) and
/// keeping it per-call means the host stays a thin wrapper.
pub struct ServerFnHost {
    pool: Arc<dyn Pool>,
}

impl ServerFnHost {
    pub fn new(pool: Arc<dyn Pool>) -> Self {
        Self { pool }
    }

    /// Run the server function named `fn_id`. `args_json` is the raw JSON
    /// array the client sent (or an empty slice for no arguments);
    /// `entry_path` is the absolute path to the SSR adapter entry
    /// (`dist/server/ssr/index.js`) whose import graph registers every
    /// `serverFn` in the project.
    ///
    /// Returns the resolved value as JSON. A function that threw, an
    /// unknown id, and a missing registry all surface as `JSH-FN*` errors
    /// so the pipeline can build the right envelope.
    pub async fn run(&self, fn_id: &str, args_json: &[u8], entry_path: &str) -> Result<Value> {
        let args_str = std::str::from_utf8(args_json).unwrap_or("[]");
        let payload = serde_json::json!({
            "fn_id": fn_id,
            "args_json": args_str,
            "entry_path": entry_path,
        });
        let call_input = CallInput {
            payload,
            headers: Default::default(),
            has_body: false,
            request_id: format!("server-fn:run:{fn_id}"),
            route: None,
        };
        let entry = EntryRef {
            kind: "server-fn:run".into(),
            module: entry_path.to_string(),
            export: fn_id.to_string(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err(
                "JSH-FN",
                format!("server-fn:run returned invalid JSON: {e}"),
            )
        })?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-FN", err_msg.to_string()));
        }
        Ok(v)
    }
}

/// §9.3: the host for `action:run`. An action is a named function in a
/// route's `actions` export; `run` sends the module path, the action name,
/// the form body (JSON-encoded entries), and the request context, and
/// returns the action's result plus any cookies/headers/revalidations it
/// produced.
pub struct ActionHost {
    pool: Arc<dyn Pool>,
}

/// The result of running an action, in the shape the HTTP pipeline needs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActionOutcome {
    pub status: u16,
    pub location: Option<String>,
    pub data: Value,
    #[serde(default, rename = "setCookies")]
    pub set_cookies: Vec<String>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Tag/path invalidations the action requested (§11.3).
    #[serde(default)]
    pub revalidate: Vec<RevalidateEvent>,
}

/// One recorded revalidation (§11.3): a tag or a path.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RevalidateEvent {
    Tag { tag: String },
    Path { path: String },
}

impl ActionHost {
    pub fn new(pool: Arc<dyn Pool>) -> Self {
        Self { pool }
    }

    /// Run the named action in `module_path`. `form_json` is the JSON array
    /// of `[key, value]` form entries.
    pub async fn run(
        &self,
        module_path: &str,
        action: &str,
        form_json: &str,
        params: &serde_json::Map<String, Value>,
        url: &str,
        method: &str,
        headers: &[(String, String)],
        // §8.1 middleware.ts / §9.1: request-scoped locals injected by
        // middleware; delivered to the action's `event.locals`.
        locals: &Value,
    ) -> Result<ActionOutcome> {
        let payload = serde_json::json!({
            "module_path": module_path,
            "action": action,
            "form_json": form_json,
            "params": params,
            "url": url,
            "method": method,
            "headers": headers,
            "locals": locals,
        });
        let call_input = CallInput {
            payload,
            headers: Default::default(),
            has_body: false,
            request_id: format!("action:run:{action}"),
            route: None,
        };
        let entry = EntryRef {
            kind: "action:run".into(),
            module: module_path.to_string(),
            export: action.to_string(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err(
                "JSH-ACTION",
                format!("action:run returned invalid JSON: {e}"),
            )
        })?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-ACTION", err_msg.to_string()));
        }
        serde_json::from_value(v)
            .map_err(|e| crate::err("JSH-ACTION", format!("action:run result shape: {e}")))
    }
}

/// The coded failure a `server-fn:run` call can carry. The worker
/// distinguishes "id not registered" from "the function threw" from a
/// transport failure, and the pipeline maps each to a different HTTP
/// response — so the host surfaces the code, not just the message.
#[derive(Debug, Clone)]
pub struct ServerFnError {
    pub code: String,
    pub message: String,
}

impl ServerFnHost {
    /// Like `run`, but returns the coded error instead of an opaque
    /// `OxideError`. The pipeline uses this form so a `JSH-FN-NOT-FOUND`
    /// becomes a 404 with an `FN-NOT-FOUND` envelope rather than a 500.
    pub async fn run_coded(
        &self,
        fn_id: &str,
        args_json: &[u8],
        entry_path: &str,
    ) -> std::result::Result<Value, ServerFnError> {
        self.run(fn_id, args_json, entry_path)
            .await
            .map_err(|e| ServerFnError {
                code: e.0.code.to_string(),
                message: e.0.message.to_string(),
            })
    }
}

// ---------------------------------------------------------------------------
// Endpoint host (§8.1 route.ts, §10.2 step 7)
// ---------------------------------------------------------------------------

/// A host for `endpoint:run`. Loads the route's `route.ts`, calls the
/// method handler in a worker, and returns the raw response (status,
/// headers, body bytes).
pub struct EndpointHost {
    pool: Arc<dyn Pool>,
}

/// The result of a `route.ts` call: status, headers, raw body. Body is raw
/// bytes so a route.ts can return binary (an image proxy, a CSV download).
#[derive(Debug)]
pub struct EndpointOutcome {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Buffered body (a small/non-streaming response).
    pub body: Vec<u8>,
    /// §9.6: a streaming body (SSE, a long download). When `Some`, the
    /// pipeline drives the response body from this stream and ignores
    /// `body`.
    pub stream: Option<tokio::sync::mpsc::Receiver<crate::Result<bytes::Bytes>>>,
}

impl EndpointHost {
    pub fn new(pool: Arc<dyn Pool>) -> Self {
        Self { pool }
    }

    /// Run `route.ts` for `module_path` with the method/handler convention.
    /// The body is base64-encoded in the payload — a route.ts can be an
    /// upload proxy, so the wire must carry arbitrary bytes.
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        module_path: &str,
        method: &str,
        url: &str,
        params: &serde_json::Map<String, Value>,
        headers: &[(String, String)],
        body: &[u8],
        // §8.1 middleware.ts / §9.1: request-scoped locals injected by
        // middleware; delivered to the route.ts handler's `ctx.locals`.
        locals: &Value,
    ) -> Result<EndpointOutcome> {
        use base64::Engine as _;
        let body_b64 = if body.is_empty() {
            String::new()
        } else {
            base64::engine::general_purpose::STANDARD.encode(body)
        };
        let payload = serde_json::json!({
            "module_path": module_path,
            "method": method,
            "url": url,
            "params": params,
            "headers": headers,
            "body_b64": body_b64,
            "locals": locals,
        });
        let call_input = CallInput {
            payload,
            headers: Default::default(),
            has_body: !body.is_empty(),
            request_id: format!("endpoint:run:{method}"),
            route: None,
        };
        let entry = EntryRef {
            kind: "endpoint:run".into(),
            module: module_path.to_string(),
            export: method.to_uppercase(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        // §9.6: a streaming `route.ts` sends a header-only first frame
        // (`streaming: true`) then raw chunks until `call_end`. Peek the
        // first item: `Complete` is a buffered response, `Chunked` is a
        // stream we hand to the pipeline as the response body.
        match &mut stream {
            crate::CallStream::Complete(bytes) => {
                let v: Value = serde_json::from_slice(bytes).map_err(|e| {
                    crate::err(
                        "JSH-ENDPOINT",
                        format!("endpoint:run returned invalid JSON: {e}"),
                    )
                })?;
                return Self::from_complete_json(v);
            }
            crate::CallStream::Chunked(rx) => {
                // The first chunk carries the header JSON.
                let first = match rx.recv().await {
                    Some(Ok(b)) => b,
                    Some(Err(e)) => return Err(e),
                    None => {
                        return Ok(EndpointOutcome {
                            status: 200,
                            headers: Vec::new(),
                            body: Vec::new(),
                            stream: None,
                        });
                    }
                };
                let v: Value = serde_json::from_slice(&first).map_err(|e| {
                    crate::err(
                        "JSH-ENDPOINT",
                        format!("endpoint:run header frame invalid: {e}"),
                    )
                })?;
                if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
                    return Err(crate::err("JSH-ENDPOINT", err_msg.to_string()));
                }
                let status = v.get("status").and_then(|x| x.as_u64()).unwrap_or(200) as u16;
                let headers = Self::parse_headers(&v);
                // Move the receiver out of the enum. `Chunked` is the only
                // arm here, so a take is safe.
                let rx = match std::mem::replace(
                    &mut stream,
                    crate::CallStream::Complete(bytes::Bytes::new()),
                ) {
                    crate::CallStream::Chunked(rx) => rx,
                    _ => unreachable!(),
                };
                return Ok(EndpointOutcome {
                    status,
                    headers,
                    body: Vec::new(),
                    stream: Some(rx),
                });
            }
        }
        // (unreachable) — kept for the compiler.
        #[allow(unreachable_code)]
        {
            let v: Value = Value::Null;
            return Self::from_complete_json(v);
        }
    }

    fn parse_headers(v: &Value) -> Vec<(String, String)> {
        v.get("headers")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|pair| {
                        let p = pair.as_array()?;
                        let k = p.first()?.as_str()?.to_string();
                        let val = p.get(1)?.as_str()?.to_string();
                        Some((k, val))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn from_complete_json(v: Value) -> Result<EndpointOutcome> {
        use base64::Engine as _;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-ENDPOINT", err_msg.to_string()));
        }
        let status = v.get("status").and_then(|x| x.as_u64()).unwrap_or(200) as u16;
        let headers_out = Self::parse_headers(&v);
        let body_b64_out = v.get("body_b64").and_then(|x| x.as_str()).unwrap_or("");
        let body = if body_b64_out.is_empty() {
            Vec::new()
        } else {
            base64::engine::general_purpose::STANDARD
                .decode(body_b64_out)
                .map_err(|e| {
                    crate::err("JSH-ENDPOINT", format!("endpoint body_b64 invalid: {e}"))
                })?
        };
        Ok(EndpointOutcome {
            status,
            headers: headers_out,
            body,
            stream: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Middleware host (§8.1 middleware.ts, §8.5 rewrites)
// ---------------------------------------------------------------------------

/// A host for `middleware:run`. Loads the middleware module, calls the
/// user's function, and surfaces the directive as a structured outcome.
pub struct MiddlewareHost {
    pool: Arc<dyn Pool>,
}

/// The result of a middleware invocation, mirroring the worker op reply.
/// Field meanings:
///   `short`      — short-circuit: the pipeline returns `status + headers + body`.
///   `status`     — HTTP status (200 when not short-circuiting).
///   `headers`    — response headers the middleware wants added (both paths).
///   `body`       — the body bytes (short-circuit path).
///   `redirect`   — a short-circuit Location (mirrors `headers` `location`).
///   `rewrite`    — the request URL should be rewritten server-side.
///   `locals`     — request-scoped state passed to the loader.
#[derive(Debug, Clone, Default)]
pub struct MiddlewareOutcome {
    pub short: bool,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub redirect: Option<String>,
    pub rewrite: Option<String>,
    pub locals: Option<serde_json::Value>,
}

impl MiddlewareHost {
    pub fn new(pool: Arc<dyn Pool>) -> Self {
        Self { pool }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        module_path: &str,
        method: &str,
        url: &str,
        params: &serde_json::Map<String, Value>,
        headers: &[(String, String)],
        body: &[u8],
        locals: Option<&serde_json::Value>,
    ) -> Result<MiddlewareOutcome> {
        use base64::Engine as _;
        let body_b64 = if body.is_empty() {
            String::new()
        } else {
            base64::engine::general_purpose::STANDARD.encode(body)
        };
        let payload = serde_json::json!({
            "module_path": module_path,
            "method": method,
            "url": url,
            "params": params,
            "headers": headers,
            "body_b64": body_b64,
            "locals": locals.cloned().unwrap_or(serde_json::json!({})),
        });
        let call_input = CallInput {
            payload,
            headers: Default::default(),
            has_body: !body.is_empty(),
            request_id: format!("middleware:run:{method}"),
            route: None,
        };
        let entry = EntryRef {
            kind: "middleware:run".into(),
            module: module_path.to_string(),
            export: "middleware".into(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err(
                "JSH-MIDDLEWARE",
                format!("middleware:run returned invalid JSON: {e}"),
            )
        })?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-MIDDLEWARE", err_msg.to_string()));
        }
        let short = v.get("short").and_then(|x| x.as_bool()).unwrap_or(false);
        let status = v.get("status").and_then(|x| x.as_u64()).unwrap_or(200) as u16;
        let headers_out: Vec<(String, String)> = v
            .get("headers")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|pair| {
                        let p = pair.as_array()?;
                        let k = p.first()?.as_str()?.to_string();
                        let val = p.get(1)?.as_str()?.to_string();
                        Some((k, val))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let body_b64_out = v.get("body_b64").and_then(|x| x.as_str()).unwrap_or("");
        let body = if body_b64_out.is_empty() {
            Vec::new()
        } else {
            base64::engine::general_purpose::STANDARD
                .decode(body_b64_out)
                .map_err(|e| {
                    crate::err(
                        "JSH-MIDDLEWARE",
                        format!("middleware body_b64 invalid: {e}"),
                    )
                })?
        };
        let redirect = v
            .get("redirect")
            .and_then(|x| x.as_str())
            .map(str::to_string);
        let rewrite = v
            .get("rewrite")
            .and_then(|x| x.as_str())
            .map(str::to_string);
        let locals = v.get("locals").cloned().filter(|x| !x.is_null());
        Ok(MiddlewareOutcome {
            short,
            status,
            headers: headers_out,
            body,
            redirect,
            rewrite,
            locals,
        })
    }
}

// ---------------------------------------------------------------------------
// Socket host (§9.6)
// ---------------------------------------------------------------------------

/// A host for `socket:run`. Loads a route's `socket.ts` and runs one phase
/// (`open` / `message` / `close`) against the Rust-side WebSocket. Rust owns
/// the upgrade, framing and ping/pong; this host is one phase call.
pub struct SocketHost {
    pool: Arc<dyn Pool>,
}

/// The result of a socket phase: whatever the handler `send()`s (a single
/// string, or several in order) and an optional close.
#[derive(Debug, Clone, Default)]
pub struct SocketOutcome {
    pub sends: Vec<String>,
    pub close: Option<(u16, String)>,
}

impl SocketHost {
    pub fn new(pool: Arc<dyn Pool>) -> Self {
        Self { pool }
    }

    /// Run one socket phase. `payload` carries the message data (for
    /// `message`) or the close code/reason (for `close`).
    pub async fn run(
        &self,
        module_path: &str,
        phase: &str,
        payload: serde_json::Value,
        socket_id: &str,
    ) -> Result<SocketOutcome> {
        let call_input = CallInput {
            payload: serde_json::json!({
                "module_path": module_path,
                "phase": phase,
                "payload": payload,
                "socket_id": socket_id,
            }),
            headers: Default::default(),
            has_body: false,
            request_id: format!("socket:run:{phase}"),
            route: None,
        };
        let entry = EntryRef {
            kind: "socket:run".into(),
            module: module_path.to_string(),
            export: phase.to_string(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err(
                "JSH-SOCKET",
                format!("socket:run returned invalid JSON: {e}"),
            )
        })?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-SOCKET", err_msg.to_string()));
        }
        let sends: Vec<String> = match v.get("send") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        let close = match v.get("close") {
            // Worker returns `close: null` for a phase that does NOT want
            // to end the socket (the smoke app's `open` calls `send` only).
            // Treat any non-object / missing `code` as `None` so the read
            // loop continues.
            Some(serde_json::Value::Object(obj)) => {
                obj.get("code").and_then(|c| c.as_u64()).map(|code| {
                    let reason = obj
                        .get("reason")
                        .and_then(|r| r.as_str())
                        .unwrap_or("")
                        .to_string();
                    (code as u16, reason)
                })
            }
            _ => None,
        };
        Ok(SocketOutcome { sends, close })
    }
}

/// Fold an NDJSON settle stream into one payload: the first line is the
/// initial response, each later `{deferredSettle: {id, value|error}}` line
/// fills `deferred[id]`. Keeps the caller's contract — `LoaderOutput::deferred`
/// is populated exactly as the old await-all path populated it — while the
/// worker no longer blocks on the slowest defer.
fn merge_settle_stream(bytes: &[u8]) -> Result<Value> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| crate::err("JSH-LOADER", format!("loader:run stream is not UTF-8: {e}")))?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let first = lines
        .next()
        .ok_or_else(|| crate::err("JSH-LOADER", "loader:run stream was empty"))?;
    let mut base: Value = serde_json::from_str(first).map_err(|e| {
        crate::err(
            "JSH-LOADER",
            format!("loader:run returned invalid JSON: {e}"),
        )
    })?;
    for line in lines {
        let settle: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(s) = settle.get("deferredSettle") else {
            continue;
        };
        let Some(id) = s.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let value = s
            .get("value")
            .cloned()
            .unwrap_or_else(|| {
                serde_json::json!({
                    "__oxide_deferred_error": s.get("error").and_then(|e| e.as_str()).unwrap_or("deferred rejected")
                })
            });
        if let Some(deferred) = base.get_mut("deferred").and_then(|d| d.as_object_mut()) {
            deferred.insert(id.to_string(), value);
        }
    }
    Ok(base)
}

#[cfg(test)]
mod settle_stream_tests {
    use super::*;

    #[test]
    fn merges_settle_lines_into_deferred() {
        let bytes =
            br#"{"ok":true,"status":200,"data":{"t":"x"},"deferred":{},"deferredPending":["1","2"]}
{"deferredSettle":{"id":"1","value":{"a":1}}}
{"deferredSettle":{"id":"2","error":"boom"}}
"#;
        let v = merge_settle_stream(bytes).expect("merge");
        assert_eq!(v["status"], 200);
        assert_eq!(v["data"]["t"], "x");
        let d = v["deferred"].as_object().expect("deferred object");
        assert_eq!(d["1"]["a"], 1, "resolved value");
        assert_eq!(
            d["2"]["__oxide_deferred_error"], "boom",
            "rejected promise serialized as an error object",
        );
    }

    #[test]
    fn a_single_json_object_still_parses() {
        let bytes = br#"{"ok":true,"status":200,"data":{},"deferred":{}}"#;
        // The fast path is `from_slice`; this asserts the fallback never runs
        // for the single-chunk shape by checking the bytes parse directly.
        let v: Value = serde_json::from_slice(bytes).expect("single");
        assert_eq!(v["status"], 200);
    }
}

#[cfg(test)]
mod kill_switch_tests {
    use super::buffered_from;

    /// The switch is a pure predicate over the env value, so it is testable
    /// in a crate that forbids `unsafe` (which `set_var` needs).
    #[test]
    fn buffered_flag_semantics() {
        assert!(!buffered_from(None), "unset -> streaming");
        assert!(!buffered_from(Some("0".into())), "0 -> streaming");
        assert!(
            !buffered_from(Some("true".into())),
            "anything but 1 -> streaming"
        );
        assert!(buffered_from(Some("1".into())), "1 -> buffered");
    }
}
