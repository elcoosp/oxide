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

impl LoaderHost {
    pub fn new(pool: Arc<dyn Pool>) -> Self {
        Self { pool }
    }

    /// Run the loader at `module_path`. `input` is the JSON payload for the
    /// worker op: `{ route, params, url, method, headers }`. Returns the
    /// raw worker response value, which callers convert to a typed
    /// `LoaderOutput`.
    pub async fn run(&self, module_path: &str, input: Value) -> Result<Value> {
        let payload = serde_json::json!({
            "module_path": module_path,
            "route": input.get("route").cloned().unwrap_or(Value::Null),
            "params": input.get("params").cloned().unwrap_or(Value::Null),
            "url": input.get("url").cloned().unwrap_or(Value::Null),
            "method": input.get("method").cloned().unwrap_or(Value::String("GET".into())),
            "headers": input.get("headers").cloned().unwrap_or(Value::Array(vec![])),
        });
        let call_input = CallInput {
            payload,
            headers: Default::default(),
            has_body: false,
            request_id: format!("loader:run:{module_path}"),
        };
        let entry = EntryRef {
            kind: "loader:run".into(),
            module: module_path.to_string(),
            export: "load".into(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err("JSH-LOADER", format!("loader:run returned invalid JSON: {e}"))
        })?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-LOADER", err_msg.to_string()));
        }
        Ok(v)
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
        let arr = v
            .get("params")
            .and_then(|x| x.as_array())
            .ok_or_else(|| {
                crate::err(
                    "JSH-PARAMS",
                    "generate-params:run response missing `params` array",
                )
            })?;
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            let obj = item.as_object().ok_or_else(|| {
                crate::err("JSH-PARAMS", "params entry not an object")
            })?;
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
        assert!(err.0.message.contains("Cannot find module"), "{}", err.0.message);
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

    #[tokio::test]
    async fn generate_params_success() {
        let pool = mock_pool(json!({
            "params": [
                { "slug": "hello" },
                { "slug": "world" }
            ]
        }));
        let h = LoaderHost::new(pool);
        let out = h
            .generate_params("/abs/page.server.ts")
            .await
            .unwrap();
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
        assert!(err.0.message.contains("no such module"), "{}", err.0.message);
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
    pub async fn run(
        &self,
        fn_id: &str,
        args_json: &[u8],
        entry_path: &str,
    ) -> Result<Value> {
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
        };
        let entry = EntryRef {
            kind: "server-fn:run".into(),
            module: entry_path.to_string(),
            export: fn_id.to_string(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err("JSH-FN", format!("server-fn:run returned invalid JSON: {e}"))
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
    ) -> Result<ActionOutcome> {
        let payload = serde_json::json!({
            "module_path": module_path,
            "action": action,
            "form_json": form_json,
            "params": params,
            "url": url,
            "method": method,
            "headers": headers,
        });
        let call_input = CallInput {
            payload,
            headers: Default::default(),
            has_body: false,
            request_id: format!("action:run:{action}"),
        };
        let entry = EntryRef {
            kind: "action:run".into(),
            module: module_path.to_string(),
            export: action.to_string(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err("JSH-ACTION", format!("action:run returned invalid JSON: {e}"))
        })?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-ACTION", err_msg.to_string()));
        }
        serde_json::from_value(v).map_err(|e| {
            crate::err("JSH-ACTION", format!("action:run result shape: {e}"))
        })
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
