//! Tooling pool (§4.7): batches JS-compiler/plugin invocations and dispatches
//! them in parallel across a `Pool`. Every result is CAS-cacheable at the
//! pipeline layer; this module provides the batch primitive and the merge
//! policy for out-of-order results.

use crate::{CallInput, EntryRef, Pool, PoolConfig, Result};
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// A single batched tooling request.
#[derive(Debug, Clone)]
pub struct ToolingRequest {
    /// Unique id so callers can match responses back to requests.
    pub id: u64,
    pub entry: EntryRef,
    pub input: CallInput,
}

/// The result of a batched tooling call.
pub struct ToolingResult {
    pub id: u64,
    pub body: Result<Bytes>,
}

/// Batched tooling pool: dispatches N requests concurrently against a `Pool`,
/// bounded by `concurrency`. Preserves request order in the returned vector.
pub struct ToolingPool {
    inner: Arc<dyn Pool>,
    concurrency: usize,
}

impl ToolingPool {
    pub fn new(inner: Arc<dyn Pool>, cfg: &PoolConfig) -> Self {
        Self {
            inner,
            concurrency: cfg.max_in_flight_per_worker.max(1),
        }
    }

    /// Execute a batch. Returns results **in request order**, regardless of
    /// completion order.
    pub async fn run_batch(&self, requests: Vec<ToolingRequest>) -> Vec<ToolingResult> {
        if requests.is_empty() {
            return Vec::new();
        }
        let sem = Arc::new(Semaphore::new(self.concurrency));
        let mut handles = Vec::with_capacity(requests.len());

        for req in requests {
            let pool = Arc::clone(&self.inner);
            let permit = Arc::clone(&sem);
            handles.push(tokio::spawn(async move {
                let _g = permit.acquire_owned().await;
                let id = req.id;
                let mut stream = match pool.call(req.entry, req.input).await {
                    Ok(s) => s,
                    Err(e) => {
                        return ToolingResult { id, body: Err(e) };
                    }
                };
                let body = stream.collect().await;
                ToolingResult { id, body }
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for h in handles {
            match h.await {
                Ok(r) => results.push(r),
                Err(e) => results.push(ToolingResult {
                    id: u64::MAX,
                    body: Err(crate::err("JSH-TOOLING", format!("task join failed: {e}"))),
                }),
            }
        }
        results.sort_by_key(|r| r.id);
        results
    }

    /// Pool stats passthrough for the dev inspector (§13.4).
    pub fn stats(&self) -> crate::PoolStats {
        self.inner.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallStream, EntryRef, JsHost, PoolConfig, ScriptedHost};
    use bytes::Bytes;
    use serde_json::json;
    use std::sync::Arc;

    fn input(v: serde_json::Value) -> CallInput {
        CallInput {
            payload: v,
            headers: Default::default(),
            has_body: false,
            request_id: "batch".into(),
            route: None,
        }
    }
    fn entry() -> EntryRef {
        EntryRef {
            kind: "tooling:compile".into(),
            module: "m".into(),
            export: "e".into(),
        }
    }

    #[tokio::test]
    async fn batch_preserves_request_order() {
        let host = ScriptedHost::with_handler(Arc::new(|_e, i| {
            // Echo the request's `n` as the body so we can check ordering.
            let n = i.payload.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
            Ok(CallStream::Complete(Bytes::from(format!("n={n}"))))
        }));
        let pool = host.tooling_pool(PoolConfig::default()).unwrap();
        let tp = ToolingPool::new(pool, &PoolConfig::default());
        let reqs: Vec<ToolingRequest> = (0..16u64)
            .map(|n| ToolingRequest {
                id: n,
                entry: entry(),
                input: input(json!({"n": n})),
            })
            .collect();
        let out = tp.run_batch(reqs).await;
        assert_eq!(out.len(), 16);
        for (i, r) in out.iter().enumerate() {
            assert_eq!(r.id, i as u64);
            assert_eq!(
                r.body.as_ref().unwrap().as_ref(),
                format!("n={i}").as_bytes()
            );
        }
    }

    #[tokio::test]
    async fn batch_reports_per_request_errors() {
        let host = ScriptedHost::with_handler(Arc::new(|_e, i| {
            let n = i.payload.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
            if n % 2 == 0 {
                Ok(CallStream::Complete(Bytes::from("ok")))
            } else {
                Err(crate::err("JSH-TOOLING", "odd"))
            }
        }));
        let pool = host.tooling_pool(PoolConfig::default()).unwrap();
        let tp = ToolingPool::new(pool, &PoolConfig::default());
        let reqs: Vec<ToolingRequest> = (0..4u64)
            .map(|n| ToolingRequest {
                id: n,
                entry: entry(),
                input: input(json!({"n": n})),
            })
            .collect();
        let out = tp.run_batch(reqs).await;
        assert!(out[0].body.is_ok());
        assert!(out[1].body.is_err());
        assert!(out[2].body.is_ok());
        assert!(out[3].body.is_err());
    }

    #[tokio::test]
    async fn empty_batch_is_empty() {
        let host = ScriptedHost::default();
        let pool = host.tooling_pool(PoolConfig::default()).unwrap();
        let tp = ToolingPool::new(pool, &PoolConfig::default());
        assert!(tp.run_batch(Vec::new()).await.is_empty());
    }
}
