//! The F05 live-loaders wiring (spec v0.2 §V2-F05 steps 2/5/6, §V2-F06):
//! the server-side state the dispatch arm composes into `GET /__oxide/live`
//! and the rerun worker the revalidation hooks feed.
//!
//! The substrate (`oxide_fw_server::live`) holds the hub, the SSE framing
//! and the rerun runner; this module owns the CLI-side handle:
//!
//! - [`LiveState`]: the hub + the enqueue channel. The endpoint arm
//!   subscribes through it (`endpoint_response` — the 503/keepalive/stale
//!   assembly is the substrate's `live_endpoint_response`); the revalidation
//!   hooks (`notify_tag` / `notify_path`) fan out to the affected
//!   subscribers; the boot spawns the runner once (`spawn_runner`) with the
//!   injected closure that dispatches the SAME data-route path as `/__data`.
//! - The runner itself is the substrate's [`Runner`](oxide_fw_server::live::Runner)
//!   (debounce + rate cap + the F06 delta policy on the pushed payload).
//!
//! Tag dependency note (recorded as a wiring delta in `docs/deviations.md`):
//! the substrate's `Sub.tags` accepts the "tags recorded by the last run",
//! but the loader/cache surfaces carry no per-route tag declarations to
//! read — so the wiring seeds every subscriber with its own `path:<url>`
//! tag (`oxide_fw_cache::tiers::path_tag`), which covers `revalidate.path`
//! events, and the `tag` fan-out covers any tags future retagging records.
//! The hub's `retag` seam stays for that.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use oxide_fw_server::OxideResponse;
use oxide_fw_server::live::{Rerun, Runner, Sub, live_endpoint_response, rerun_request};

/// The dispatch-side live state (§V2-F05 steps 4/5/6). Created once per
/// server boot when the transport is `sse`; rides [`StartedServer`] so the
/// e2e pins and the inspector can read the hub.
pub struct LiveState {
    /// The subscriber index (`affected(tag)` — the revalidation fan-out).
    pub hub: Arc<oxide_fw_server::live::Hub>,
    /// The SSE keepalive period the endpoint bodies use (`live.keepaliveSeconds`).
    keepalive: Duration,
    /// The rerun notifications (revalidate hooks + the connect-time first
    /// snapshot). Bounded: a full queue drops the rerun (the subscriber
    /// catches the next revalidate) instead of stalling the responder.
    enqueue: mpsc::Sender<Arc<Sub>>,
    /// The runner's receiver, taken once by [`LiveState::spawn_runner`].
    rx: std::sync::Mutex<Option<mpsc::Receiver<Arc<Sub>>>>,
}

impl LiveState {
    /// A live state with the configured capacity (`live.maxSubscribers` /
    /// `live.perIpMax`) and keepalive (`live.keepaliveSeconds`).
    #[must_use]
    pub fn new(max_subscribers: u32, per_ip_max: u32, keepalive_seconds: u64) -> Arc<Self> {
        let (enqueue, rx) = mpsc::channel(256);
        Arc::new(Self {
            hub: oxide_fw_server::live::Hub::new(max_subscribers as usize, per_ip_max as usize),
            keepalive: Duration::from_secs(keepalive_seconds),
            enqueue,
            rx: std::sync::Mutex::new(Some(rx)),
        })
    }

    /// Spawn the rerun worker over the injected closure (§V2-F05 step 5) —
    /// exactly once per boot; `false` when a runner is already running (a
    /// second call would strand its receiver).
    pub fn spawn_runner(
        self: &Arc<Self>,
        debounce: Duration,
        max_reruns_per_second: u32,
        rerun: Rerun,
    ) -> bool {
        let Some(rx) = self.rx.lock().expect("live runner mutex poisoned").take() else {
            return false;
        };
        let runner = Runner::new(rx, debounce, max_reruns_per_second, rerun);
        tokio::spawn(runner.run());
        true
    }

    /// Enqueue one subscriber (the connect-time first snapshot — the runner
    /// re-runs the data route and pushes it; the watch channel means a late
    /// receiver still sees it as its initial frame).
    pub fn enqueue(&self, sub: Arc<Sub>) {
        self.enqueue_sub(sub);
    }

    /// The revalidation hook (§V2-F05 step 6): enqueue every subscriber that
    /// depends on `tag`. Called after the cache work completes (an action's
    /// `revalidate.tag` event, the tiered-cache invalidation sites).
    pub fn notify_tag(&self, tag: &str) {
        for sub in self.hub.affected(tag) {
            self.enqueue_sub(sub);
        }
    }

    /// The `revalidate.path` spelling: fan out under the `path:<url>` tag
    /// convention (`oxide_fw_cache::tiers::path_tag` — the same tag the
    /// tiered cache's `invalidate_path` uses).
    pub fn notify_path(&self, path: &str) {
        self.notify_tag(&oxide_fw_cache::tiers::path_tag(path));
    }

    /// The endpoint's response for one subscription attempt (§V2-F05 steps
    /// 2/4): the SSE head + the snapshot stream (stale-terminated when
    /// `build_id` is present), or the capacity `503` + `retry-after`.
    #[must_use]
    pub fn endpoint_response(&self, sub: Arc<Sub>, build_id: Option<&str>) -> OxideResponse {
        match live_endpoint_response(&self.hub, sub, self.keepalive, build_id) {
            Ok(resp) | Err(resp) => resp,
        }
    }

    /// try_send: a full queue drops the rerun (the subscriber catches the
    /// next revalidate); awaiting would stall the revalidation response.
    fn enqueue_sub(&self, sub: Arc<Sub>) {
        if self.enqueue.try_send(sub).is_err() {
            tracing::warn!("live: rerun queue full; rerun dropped until the next revalidate");
        }
    }
}

/// The rerun closure (§V2-F05 step 5): dispatch the SAME data-route path as
/// `/__data` for the subscriber (`rerun_request` carries the stored header
/// snapshot — middleware, locals, layout loads and gating behave
/// identically; no second implementation). A `200` with a body is the new
/// loader snapshot (the F06 delta policy patches it downstream);
/// a redirect, an error or an empty body is the stale signal (`None`).
pub fn rerun_closure(
    dispatch: Arc<dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponseLike + Send + Sync>,
) -> Rerun {
    Arc::new(move |sub| {
        let dispatch = Arc::clone(&dispatch);
        Box::pin(async move {
            let resp = dispatch(rerun_request(&sub)).await;
            if resp.status == 200 && !resp.body.is_empty() {
                Some(resp.body)
            } else {
                None
            }
        })
    })
}

/// The dispatch shape the rerun closure accepts (the CLI's `BoxedResponse`
/// alias — restated here so this module needs no `crate::` import).
pub type BoxedResponseLike =
    std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>;
