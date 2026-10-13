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
}
