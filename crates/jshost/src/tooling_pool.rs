//! Tooling pool (§4.7): invoke real upstream JS compilers and plugins.
//!
//! The spec's §6.3/§6.6 route Vue (`@vue/compiler-sfc`), Svelte
//! (`svelte/compiler`), Solid (`babel-preset-solid`) and Tailwind
//! (`@tailwindcss/oxide`) through the tooling pool rather than reimplementing
//! them. This module provides the generic mechanism:
//!
//! - `ToolingModules` declares the npm packages to preload in the worker.
//! - `call_module` dispatches `(module, fn, args)` to the worker, which
//!   `require`s the module and calls the named export with the JSON args.
//! - Results are JSON; binary outputs are base64-wrapped.
//!
//! On the Rust side this is a thin wrapper over `Pool::call` with
//! `EntryRef::kind = "tooling:module"`.

use crate::{CallInput, EntryRef, Pool, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// A call into a named npm module's export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleCall {
    /// npm package name, e.g. `"@vue/compiler-sfc"`.
    pub module: String,
    /// Named export, e.g. `"parse"` or `"compile"`.
    pub export: String,
    /// JSON-serialized arguments.
    pub args: Vec<Value>,
}

/// Bridge to real upstream JS compilers running in the tooling pool.
pub struct ToolingModules {
    pool: Arc<dyn Pool>,
}

impl ToolingModules {
    pub fn new(pool: Arc<dyn Pool>) -> Self {
        Self { pool }
    }

    /// Call `module.export(...args)` in the worker; return the JSON result.
    pub async fn call_module(&self, call: ModuleCall) -> Result<Value> {
        let req = CallInput {
            payload: serde_json::to_value(&call)
                .map_err(|e| crate::err("JSH-TOOLING", format!("encode ModuleCall: {e}")))?,
            headers: Default::default(),
            has_body: false,
            request_id: format!("tooling:{}:{}", call.module, call.export),
            route: None,
        };
        let entry = EntryRef {
            kind: "tooling:module".into(),
            module: call.module.clone(),
            export: call.export.clone(),
        };
        let mut stream = self.pool.call(entry, req).await?;
        let bytes = stream.collect().await?;
        // The worker returns either a raw JSON value (as UTF-8 bytes) or a
        // diagnostic object with a `error` field.
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| crate::err("JSH-TOOLING", format!("worker returned invalid JSON: {e}")))?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-TOOLING", err_msg.to_string()));
        }
        Ok(v)
    }

    /// Convenience: Vue SFC compiler (`@vue/compiler-sfc`).
    pub async fn vue_compile_sfc(&self, source: &str, filename: &str) -> Result<Value> {
        self.call_module(ModuleCall {
            module: "@vue/compiler-sfc".into(),
            export: "parse".into(),
            args: vec![Value::String(source.into()), Value::String(filename.into())],
        })
        .await
    }

    /// Convenience: Svelte compiler (`svelte/compiler`).
    pub async fn svelte_compile(
        &self,
        source: &str,
        filename: &str,
        generate: &str,
    ) -> Result<Value> {
        self.call_module(ModuleCall {
            module: "svelte/compiler".into(),
            export: "compile".into(),
            args: vec![
                Value::String(source.into()),
                serde_json::json!({ "filename": filename, "generate": generate }),
            ],
        })
        .await
    }

    /// Convenience: Solid JSX transform (`babel-preset-solid` via `babel`).
    pub async fn solid_transform(&self, source: &str, filename: &str) -> Result<Value> {
        self.call_module(ModuleCall {
            module: "babel-preset-solid".into(),
            export: "transform".into(),
            args: vec![
                Value::String(source.into()),
                serde_json::json!({ "filename": filename }),
            ],
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallStream, JsHost, PoolConfig, ScriptedHost};
    use bytes::Bytes;
    use serde_json::json;

    fn entry_echo() -> Arc<dyn Pool> {
        let host = ScriptedHost::with_handler(Arc::new(|_e, i| {
            // Echo the arguments back so ordering/shape can be verified.
            let v: Value = serde_json::from_value(i.payload).unwrap_or(Value::Null);
            let echo = json!({ "echo": v });
            let bytes = serde_json::to_vec(&echo).unwrap();
            Ok(CallStream::Complete(Bytes::from(bytes)))
        }));
        host.tooling_pool(PoolConfig::default()).unwrap()
    }

    fn entry_error() -> Arc<dyn Pool> {
        let host = ScriptedHost::with_handler(Arc::new(|_e, _i| {
            let bytes =
                serde_json::to_vec(&json!({ "__oxide_error": "simulated failure" })).unwrap();
            Ok(CallStream::Complete(Bytes::from(bytes)))
        }));
        host.tooling_pool(PoolConfig::default()).unwrap()
    }

    #[tokio::test]
    async fn call_module_roundtrips() {
        let tp = ToolingModules::new(entry_echo());
        let out = tp
            .call_module(ModuleCall {
                module: "example".into(),
                export: "fn".into(),
                args: vec![json!(1), json!("two")],
            })
            .await
            .unwrap();
        assert_eq!(out["echo"]["module"], "example");
        assert_eq!(out["echo"]["export"], "fn");
        assert_eq!(out["echo"]["args"][0], 1);
        assert_eq!(out["echo"]["args"][1], "two");
    }

    #[tokio::test]
    async fn error_payload_becomes_err() {
        let tp = ToolingModules::new(entry_error());
        let e = tp
            .call_module(ModuleCall {
                module: "x".into(),
                export: "y".into(),
                args: vec![],
            })
            .await
            .unwrap_err();
        assert_eq!(e.0.code, "JSH-TOOLING");
        assert!(e.0.message.contains("simulated failure"));
    }

    #[tokio::test]
    async fn vue_convenience_sends_expected_shape() {
        let tp = ToolingModules::new(entry_echo());
        let out = tp.vue_compile_sfc("<template/>", "App.vue").await.unwrap();
        let call = &out["echo"];
        assert_eq!(call["module"], "@vue/compiler-sfc");
        assert_eq!(call["export"], "parse");
        assert_eq!(call["args"][0], "<template/>");
        assert_eq!(call["args"][1], "App.vue");
    }
}

// tailwind tooling delegates to @oxide-fw/tailwind/runtime.mjs
impl ToolingModules {
    pub async fn tailwind_scan(&self, files: Vec<Value>) -> Result<Vec<String>> {
        let out = self
            .call_module(ModuleCall {
                module: "@oxide-fw/tailwind/runtime.mjs".into(),
                export: "scan".into(),
                args: vec![serde_json::json!({ "sources": [], "files": files })],
            })
            .await?;
        let arr = out.as_array().ok_or_else(|| {
            crate::err(
                "JSH-TOOLING",
                format!("tailwind_scan: expected array result, got {out}"),
            )
        })?;
        Ok(arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect())
    }

    pub async fn tailwind_build(
        &self,
        css: &str,
        base: &str,
        candidates: &[String],
    ) -> Result<String> {
        let out = self
            .call_module(ModuleCall {
                module: "@oxide-fw/tailwind/runtime.mjs".into(),
                export: "build".into(),
                args: vec![serde_json::json!({
                    "css": css,
                    "base": base,
                    "candidates": candidates,
                })],
            })
            .await?;
        out.get("css")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| {
                crate::err(
                    "JSH-TOOLING",
                    format!("tailwind_build: missing 'css' field, got {out}"),
                )
            })
    }
}
