//! Bridge from `oxide_jshost::LoaderHost` to `oxide_server::LoaderRunner`.
//!
//! Same layering as `framework_shim::AdapterPageRenderer`: the server trait
//! is framework-agnostic and testable with a fake; this shim owns the JSON
//! conversion and the concrete `LoaderHost` binding.
//!
//! The conversion functions (`input_to_payload`, `output_from_raw`) are
//! pure and unit tested without Node; the wiring function
//! (`build_loader_runner`) creates the host and wraps it.

use async_trait::async_trait;
use oxide_jshost::LoaderHost;
use oxide_server::{LoaderOutput, LoaderRunInput, LoaderRunner};
use serde_json::Value;
use std::sync::Arc;

/// Convert a typed `LoaderRunInput` into the JSON payload the worker op
/// expects. Pure; tested directly.
pub fn input_to_payload(input: &LoaderRunInput) -> Value {
    serde_json::json!({
        "route": input.route,
        "params": input.params,
        "url": input.url,
        "method": input.method,
        "headers": input.headers,
        // §8.1 middleware.ts / §9.1: request-scoped locals injected by
        // middleware. The worker forwards this as `event.locals`.
        "locals": input.locals,
        // SPEC-10: resolved locale — the worker's loader event sees it.
        "locale": input.locale,
        // §9.2: the pre-resolved outer-layout data for `parent()`.
        "parent_data": input.parent_data,
    })
}

/// Parse the worker's `{ ok, status, location, data }` response into a
/// typed `LoaderOutput`. Missing fields fall back to safe defaults.
/// A response missing `status` is treated as a runner error.
pub fn output_from_raw(raw: &Value) -> oxide_server::Result<LoaderOutput> {
    let status = raw
        .get("status")
        .and_then(|v| v.as_u64())
        .map(|n| n as u16)
        .ok_or_else(|| {
            oxide_server::err("JSH-LOADER", "loader:run response missing numeric `status`")
        })?;
    let location = raw
        .get("location")
        .and_then(|v| v.as_str())
        .map(String::from);
    let data = raw
        .get("data")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    // §9.2 / §16.6: side effects the load produced. The worker returns
    // `setCookies` as an array of raw `Set-Cookie` values (built from
    // `event.cookies.set(...)`) and `headers` as an array of `[k, v]` pairs
    // (from `event.setHeaders(...)`). Absent fields are an empty list, so an
    // older worker response still parses.
    let set_cookies = raw
        .get("setCookies")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let headers = raw
        .get("headers")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|pair| {
                    let k = pair.get(0)?.as_str()?;
                    let v = pair.get(1)?.as_str()?;
                    Some((k.to_string(), v.to_string()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // §7.1 / §9.2: resolved deferred data. The worker returns `{id: value}`
    // (keys are stringified numeric ids). Absent field → empty map.
    let deferred = raw
        .get("deferred")
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<std::collections::BTreeMap<String, serde_json::Value>>()
        })
        .unwrap_or_default();
    Ok(LoaderOutput {
        status,
        location,
        data,
        set_cookies,
        headers,
        deferred,
    })
}

/// A `LoaderRunner` backed by the Node worker. Construct with
/// `build_loader_runner`; the runner reuses the pool for many requests.
pub struct AdapterLoader {
    inner: Arc<LoaderHost>,
}

impl AdapterLoader {
    pub fn new(inner: Arc<LoaderHost>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl LoaderRunner for AdapterLoader {
    fn name(&self) -> &str {
        "node"
    }
    async fn run(&self, input: LoaderRunInput) -> oxide_server::Result<LoaderOutput> {
        let payload = input_to_payload(&input);
        let raw = self
            .inner
            .run(&input.module_path, payload)
            .await
            .map_err(|e| e.0)?;
        output_from_raw(&raw)
    }
}

/// Convenience constructor for the `cmd_*` paths: given a shared pool,
/// wrap it in a `LoaderRunner`.
pub fn build_loader_runner(pool: Arc<dyn oxide_jshost::Pool>) -> Arc<dyn LoaderRunner> {
    Arc::new(AdapterLoader::new(Arc::new(LoaderHost::new(pool))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn input_to_payload_serializes_all_fields() {
        let mut input =
            LoaderRunInput::new("/abs/page.server.ts", "routes/blog/[slug]", "/blog/hi");
        input.params.insert("slug".into(), "hi".into());
        input.headers.push(("cookie".into(), "a=b".into()));
        let v = input_to_payload(&input);
        assert_eq!(v["route"], "routes/blog/[slug]");
        assert_eq!(v["url"], "/blog/hi");
        assert_eq!(v["method"], "GET");
        assert_eq!(v["params"]["slug"], "hi");
        assert_eq!(v["headers"][0][0], "cookie");
        // §8.1 / §9.1: locals + locale must survive the shim. Regression
        // guard — the shim used to drop both.
        assert_eq!(v["locals"], serde_json::json!({}), "empty locals default");
        assert_eq!(v["locale"], "", "empty locale default");
    }

    #[test]
    fn input_to_payload_forwards_middleware_locals() {
        let mut input = LoaderRunInput::new("/x/page.server.ts", "routes/x", "/x");
        input.locals = serde_json::json!({ "user": "ada", "role": "admin" });
        input.locale = "en".into();
        let v = input_to_payload(&input);
        assert_eq!(v["locals"]["user"], "ada");
        assert_eq!(v["locals"]["role"], "admin");
        assert_eq!(v["locale"], "en");
    }

    #[test]
    fn output_from_raw_parses_cookies_and_headers() {
        // §9.2 / §16.6: the worker returns `setCookies` (raw Set-Cookie values)
        // and `headers` (k/v pairs); the shim lifts them onto LoaderOutput.
        let raw = json!({
            "ok": true,
            "status": 200,
            "location": null,
            "data": {"visits": "42"},
            "setCookies": ["visits=42; Path=/; HttpOnly; SameSite=Lax"],
            "headers": [["x-oxide-visits", "42"]],
        });
        let out = output_from_raw(&raw).unwrap();
        assert_eq!(out.status, 200);
        assert_eq!(out.set_cookies.len(), 1);
        assert!(out.set_cookies[0].starts_with("visits=42"));
        assert_eq!(
            out.headers,
            vec![("x-oxide-visits".to_string(), "42".to_string())]
        );
    }

    #[test]
    fn output_from_raw_tolerates_missing_side_effect_fields() {
        // An older worker without the fields parses to empty lists, not an error.
        let raw = json!({ "ok": true, "status": 200, "location": null, "data": {} });
        let out = output_from_raw(&raw).unwrap();
        assert!(out.set_cookies.is_empty());
        assert!(out.headers.is_empty());
    }

    #[test]
    fn output_from_raw_success() {
        let raw = json!({ "ok": true, "status": 200, "location": null, "data": {"title": "x"} });
        let out = output_from_raw(&raw).unwrap();
        assert_eq!(out.status, 200);
        assert!(out.location.is_none());
        assert_eq!(out.data["title"], "x");
    }

    #[test]
    fn output_from_raw_redirect() {
        let raw = json!({ "ok": true, "status": 303, "location": "/login", "data": {} });
        let out = output_from_raw(&raw).unwrap();
        assert_eq!(out.status, 303);
        assert_eq!(out.location.as_deref(), Some("/login"));
    }

    #[test]
    fn output_from_raw_error_status() {
        let raw =
            json!({ "ok": true, "status": 404, "location": null, "data": {"message":"missing"} });
        let out = output_from_raw(&raw).unwrap();
        assert_eq!(out.status, 404);
        assert_eq!(out.data["message"], "missing");
    }

    #[test]
    fn output_from_raw_missing_status_is_error() {
        let raw = json!({ "ok": true, "data": {} });
        let err = output_from_raw(&raw).unwrap_err();
        assert_eq!(err.0.code, "JSH-LOADER");
        assert!(err.0.message.contains("missing numeric `status`"));
    }

    #[test]
    fn output_from_raw_missing_data_defaults_to_empty_object() {
        let raw = json!({ "status": 200 });
        let out = output_from_raw(&raw).unwrap();
        assert_eq!(out.status, 200);
        assert!(out.data.is_object());
        assert!(out.data.as_object().unwrap().is_empty());
    }
}
