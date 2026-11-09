//! The live-loaders substrate (spec v0.2, §V2-F05 steps 3–5).
//!
//! A route marked `live` re-runs its loaders when a cache tag it depends on
//! is revalidated, and pushes the new data to open tabs with no hand-written
//! socket code. This module owns the V8-free pieces:
//!
//! - [`sse_frame`]: the SSE framing (kept pure so it is trivially testable).
//! - [`Sub`]: one open live connection — route id, URL, a header snapshot
//!   taken at connect, the tags it depends on, and the `watch` channel the
//!   latest loader snapshot lands in (latest wins; slow clients skip
//!   intermediate states).
//! - [`Hub`]: the tag index over subscribers. `affected(tag)` answers
//!   "which open connections depend on this tag" (exact tags plus
//!   `prefix:*` wildcards); `subscribe` enforces the global and per-peer
//!   capacity the endpoint turns into `503` + `retry-after`; [`Guard`]'s
//!   `Drop` unsubscribes.
//! - [`live_stream`]: the response body — the current snapshot first (a
//!   fresh connection never needs a replay), then one `data` frame per new
//!   snapshot, `ping` frames on the keepalive cadence, and a clean end when
//!   the subscriber goes away.
//! - [`Runner`]: the rerun worker — debounces notification bursts,
//!   enforces the per-subscriber rerun rate cap, calls the injected
//!   `rerun` closure and pushes the result into the subscriber's channel.
//!   A `None` result (redirect/error) clears the snapshot: the wiring
//!   watches for the cleared channel to emit `event: stale` and close.
//! - [`delta_payload`] (F06, spec §V2-F06 / D30): the sending policy that
//!   turns a rerun's full snapshot into an `ODP1` patch when the diff is
//!   small enough — the payload the client's `decodeOdfOrPatch` sniffs.
//!
//! The endpoint itself (`GET /__oxide/live`: route lookup, same-origin
//! enforcement via [`same_origin`], the build check via
//! `oxide_fw_dispatch::build::stage_build_check`, and the rerun closure
//! that calls the CLI's data-route path) is the A01-phase-3 CLI wiring —
//! see `docs/deviations.md`.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use tokio::sync::{mpsc, watch};

use crate::OxideResponse;

/// A retry-after hint (seconds) for the capacity `503` — short, because a
/// dropped tab frees its slot within one keepalive period.
pub const CAPACITY_RETRY_AFTER_SECS: u64 = 5;

/// The keepalive/ping event name (§V2-F05 step 3).
pub const PING_EVENT: &str = "ping";

/// The stale event name: the build changed; the client reloads once (A04).
pub const STALE_EVENT: &str = "stale";

/// The data event name: an ODF1 full snapshot or — since F06 — an `ODP1`
/// patch chained from the subscriber's previous full snapshot (§V2-F06).
pub const DATA_EVENT: &str = "data";

/// The F06 sending rule (D30): ship a patch only while it stays under half
/// the full payload's size and the diff produces at most this many ops.
pub const MAX_PATCH_OPS: usize = oxide_fw_odf::patch::PATCH_MAX_OPS;

// ------------------------------------------------------------------- deltas

/// The F06 sending policy (§V2-F06 / D30): diff the previous full snapshot
/// against the new one and encode the patch — shipped only while
/// `patch.len() * 2 < full.len()` and `ops.len() <= MAX_PATCH_OPS`.
/// Everything else ships the full payload: no base yet, undecodable bytes
/// (the rerun closure may return an error page), oversized diffs, or an
/// encode failure. Total — it always returns something sendable.
#[must_use]
pub fn delta_payload(prev: Option<&[u8]>, next: &[u8]) -> Bytes {
    let full = || Bytes::copy_from_slice(next);
    let Some(prev) = prev else {
        return full();
    };
    let (Ok(old), Ok(new)) = (oxide_fw_odf::decode(prev), oxide_fw_odf::decode(next)) else {
        return full();
    };
    let ops = oxide_fw_odf::patch::diff(&old, &new);
    if ops.len() > MAX_PATCH_OPS {
        return full();
    }
    let Ok(patch) = oxide_fw_odf::patch::encode_patch(&oxide_fw_odf::patch::base_id(prev), &ops)
    else {
        return full();
    };
    if patch.len() * 2 < next.len() {
        Bytes::from(patch)
    } else {
        full()
    }
}

// ------------------------------------------------------------------- framing

/// Frame one SSE event (§V2-F05 step 3). Multi-line data becomes one
/// `data:` line per input line; the event terminator is a blank line.
#[must_use]
pub fn sse_frame(event: &str, id: Option<usize>, data: &str) -> String {
    let mut s = String::new();
    // `write!`/`writeln!` into a `String` is infallible; the `Result` is
    // discarded.
    let _ = writeln!(s, "event: {event}");
    if let Some(i) = id {
        let _ = writeln!(s, "id: {i}");
    }
    for line in data.split('\n') {
        let _ = writeln!(s, "data: {line}");
    }
    s.push('\n');
    s
}

/// The `200` SSE response head (§V2-F05 step 2): `text/event-stream`,
/// `no-store` (every connection has its own snapshot stream) and
/// `x-accel-buffering: no` so proxies do not buffer the stream.
#[must_use]
pub fn live_response(stream: crate::BodyStream) -> OxideResponse {
    OxideResponse::stream(200, stream)
        .with_header("content-type", "text/event-stream")
        .with_header("cache-control", "no-store")
        .with_header("x-accel-buffering", "no")
}

/// The capacity `503` (§V2-F05 step 4: "Err when over capacity: the
/// endpoint answers 503 with retry-after").
#[must_use]
pub fn live_capacity_response() -> OxideResponse {
    OxideResponse::new(503, Bytes::from_static(b"live capacity exceeded\n"))
        .with_header("retry-after", CAPACITY_RETRY_AFTER_SECS.to_string())
}

// ----------------------------------------------------------- origin checks

/// Same-origin gate for the live endpoint (§V2-F05 step 2: "Same-origin
/// only (`Origin`/`Sec-Fetch-Site`)").
///
/// - `sec_fetch_site` is the strongest signal when present: only
///   `same-origin` passes.
/// - Otherwise the `Origin` authority is compared against the `Host`
///   header (host case-insensitively, port exactly; a port-less host
///   header takes the origin's scheme-default port).
/// - No evidence at all (curl, tests, a direct socket) is denied — the
///   endpoint is a browser-facing surface.
#[must_use]
pub fn same_origin(origin: Option<&str>, sec_fetch_site: Option<&str>, host: Option<&str>) -> bool {
    if let Some(site) = sec_fetch_site {
        return site.eq_ignore_ascii_case("same-origin");
    }
    let (Some(origin), Some(host)) = (origin, host) else {
        return false;
    };
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    let Some(origin_host) = url.host_str() else {
        return false;
    };
    let (host_name, host_port) = split_authority(host);
    let origin_port = url.port_or_known_default().unwrap_or(0);
    origin_host.eq_ignore_ascii_case(host_name) && origin_port == host_port.unwrap_or(origin_port)
}

/// Split a `Host`-header authority into name and optional numeric port.
fn split_authority(host: &str) -> (&str, Option<u16>) {
    match host.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
            (h, p.parse::<u16>().ok())
        }
        _ => (host, None),
    }
}

// -------------------------------------------------------------- subscribers

/// One open live connection (§V2-F05 step 4). The header snapshot
/// (`headers`) lives only as long as the connection; the `watch` channel
/// keeps the latest loader snapshot (latest wins — slow clients skip
/// intermediate states).
pub struct Sub {
    /// The route id the connection re-runs (`routes/blog/[slug]`).
    pub route: String,
    /// The page URL the connection was opened from (path + query).
    pub url: String,
    /// Request-header snapshot taken at connect (cookies included); never
    /// logged (`is_secret_key` guards the class, see the spec's security
    /// note). Used by the wiring to replay the data-route path.
    pub headers: Vec<(String, String)>,
    /// Route config tags + the tags recorded by the last rerun. `BTreeSet`
    /// (not the spec's `HashSet`) so `affected` fan-out order is
    /// deterministic for the same tag set.
    pub tags: std::sync::Mutex<BTreeSet<String>>,
    /// The latest loader snapshot; `None` = no snapshot yet (or cleared by
    /// a failed rerun — the stale signal).
    pub tx: watch::Sender<Option<Bytes>>,
    /// The always-alive primary receiver. `watch::Sender::send` fails when
    /// no receiver exists, so the owner keeps one; the endpoint subscribes
    /// its own receivers per connection (`tx.subscribe()`). Never read
    /// directly — it exists so the channel never has zero receivers.
    _rx: watch::Receiver<Option<Bytes>>,
    /// Monotonic per-connection data-event counter (the SSE `id:` line).
    pub seq: AtomicUsize,
    /// The peer address for the per-IP subscriber cap (the spec's security
    /// bullet); `None` where the wiring has no peer (unix socket, tests).
    pub ip: Option<IpAddr>,
    /// The last FULL snapshot bytes (F06): the patch base the next rerun
    /// diffs against — the client hashes the payload it last decoded, so
    /// every shipped patch chains from this. The channel itself may hold
    /// patches; this always holds fulls.
    last_full: std::sync::Mutex<Option<Bytes>>,
}

impl Sub {
    /// A fresh subscriber: no snapshot yet, tags as given.
    pub fn new(
        route: impl Into<String>,
        url: impl Into<String>,
        headers: Vec<(String, String)>,
        tags: impl IntoIterator<Item = String>,
        ip: Option<IpAddr>,
    ) -> Self {
        let (tx, rx) = watch::channel(None);
        Self {
            route: route.into(),
            url: url.into(),
            headers,
            tags: std::sync::Mutex::new(tags.into_iter().collect()),
            tx,
            _rx: rx,
            seq: AtomicUsize::new(0),
            ip,
            last_full: std::sync::Mutex::new(None),
        }
    }

    /// The currently-held snapshot, if any.
    #[must_use]
    pub fn snapshot_now(&self) -> Option<Bytes> {
        self.tx.borrow().clone()
    }

    /// The last FULL snapshot (the F06 patch base for the next rerun).
    #[must_use]
    pub fn last_full_snapshot(&self) -> Option<Bytes> {
        self.last_full
            .lock()
            .expect("patch base mutex poisoned")
            .clone()
    }

    /// Push a new loader snapshot (the connect path and direct pushes).
    /// The full bytes seed the patch base: a fresh connection hashes them,
    /// so the first rerun can already ship a patch (F06).
    pub fn push_snapshot(&self, snapshot: Bytes) {
        *self.last_full.lock().expect("patch base mutex poisoned") = Some(snapshot.clone());
        let _ = self.tx.send(Some(snapshot));
    }

    /// Compute the payload for a rerun result WITHOUT sending it (the
    /// runner sends — one send point, as before). `Some(full)` becomes the
    /// F06 delta payload (patch-or-full per [`delta_payload`]) and stores
    /// the full as the next patch base; `None` (the stale signal) passes
    /// through untouched.
    #[must_use]
    pub fn prepare_rerun(&self, full: Option<Bytes>) -> Option<Bytes> {
        let full = full?;
        let prev = self.last_full_snapshot();
        let payload = delta_payload(prev.as_deref(), &full);
        *self.last_full.lock().expect("patch base mutex poisoned") = Some(full);
        Some(payload)
    }

    /// Clear the snapshot (the rerun worker's redirect/error path — the
    /// stale signal). The patch base stays: the reconnecting client hashes
    /// a fresh full snapshot either way.
    pub fn clear_snapshot(&self) {
        let _ = self.tx.send(None);
    }
}

/// Why [`Hub::subscribe`] refused a subscriber. The endpoint maps both to
/// `503` + `retry-after` (capacity is global, the per-IP cap is the
/// spec's per-peer abuse bound).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscribeError {
    /// The hub is at `max` subscribers.
    Capacity,
    /// The peer already holds `per_ip_max` subscribers.
    PerIp,
}

/// A live subscription handle: dropping it unsubscribes (`Guard::drop` →
/// `Hub::remove`, the spec's own pin).
pub struct Guard {
    hub: Arc<Hub>,
    sub: Arc<Sub>,
}

impl Guard {
    /// The guarded subscriber.
    #[must_use]
    pub fn sub(&self) -> &Arc<Sub> {
        &self.sub
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.hub.remove(self.sub.as_ref());
    }
}

/// The tag index over open live connections (§V2-F05 step 4). Indexes live
/// in two maps: exact tags and wildcard prefixes (`comments:*` registers
/// under the `comments:` prefix; `affected` checks `starts_with`).
pub struct Hub {
    by_tag: dashmap::DashMap<String, Vec<std::sync::Weak<Sub>>>,
    by_prefix: dashmap::DashMap<String, Vec<std::sync::Weak<Sub>>>,
    per_ip: dashmap::DashMap<IpAddr, usize>,
    count: AtomicUsize,
    max: usize,
    per_ip_max: usize,
}

impl Hub {
    /// A hub with the given global subscriber capacity and per-peer cap.
    /// (The spec's `new(max)` plus the per-IP bound the config surface
    /// carries — `live.perIpMax`.)
    #[must_use]
    pub fn new(max: usize, per_ip_max: usize) -> Arc<Self> {
        Arc::new(Self {
            by_tag: dashmap::DashMap::new(),
            by_prefix: dashmap::DashMap::new(),
            per_ip: dashmap::DashMap::new(),
            count: AtomicUsize::new(0),
            max,
            per_ip_max,
        })
    }

    /// The number of live subscribers.
    #[must_use]
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    /// Register a subscriber. `Err` when over capacity (global or
    /// per-peer) — the endpoint answers `503` + `retry-after`.
    ///
    /// # Errors
    ///
    /// [`SubscribeError::Capacity`] at the global limit,
    /// [`SubscribeError::PerIp`] at the peer's limit; both roll the
    /// internal counters back so a retry after a `drop` succeeds.
    pub fn subscribe(self: &Arc<Self>, sub: Arc<Sub>) -> Result<Guard, SubscribeError> {
        if self.count.fetch_add(1, Ordering::AcqRel) >= self.max {
            self.count.fetch_sub(1, Ordering::AcqRel);
            return Err(SubscribeError::Capacity);
        }
        if let Some(ip) = sub.ip {
            let exceeded = {
                let mut c = self.per_ip.entry(ip).or_insert(0);
                *c += 1;
                *c > self.per_ip_max
            };
            if exceeded {
                self.ip_release(ip);
                self.count.fetch_sub(1, Ordering::AcqRel);
                return Err(SubscribeError::PerIp);
            }
        }
        self.index(&sub);
        Ok(Guard {
            hub: Arc::clone(self),
            sub,
        })
    }

    /// Remove a subscriber from every index (idempotent — `Guard::drop`
    /// is the only caller).
    fn remove(&self, sub: &Sub) {
        self.count.fetch_sub(1, Ordering::AcqRel);
        if let Some(ip) = sub.ip {
            self.ip_release(ip);
        }
        self.sweep(sub);
    }

    /// Release one per-peer slot (dropping the counter entry at zero so
    /// the map cannot grow with churn).
    fn ip_release(&self, ip: IpAddr) {
        if let Some(mut c) = self.per_ip.get_mut(&ip) {
            let remaining = c.saturating_sub(1);
            *c = remaining;
            drop(c);
            if remaining == 0 {
                self.per_ip.remove(&ip);
            }
        }
    }

    /// Add the sub's current tag set to the indexes.
    fn index(self: &Arc<Self>, sub: &Arc<Sub>) {
        let tags = sub.tags.lock().expect("tag mutex poisoned").clone();
        for tag in tags {
            if let Some(prefix) = tag.strip_suffix('*') {
                self.by_prefix
                    .entry(prefix.to_string())
                    .or_default()
                    .push(Arc::downgrade(sub));
            } else {
                self.by_tag
                    .entry(tag)
                    .or_default()
                    .push(Arc::downgrade(sub));
            }
        }
    }

    /// Remove the sub from every index entry (scans both maps — `retag`
    /// may have moved it between tags).
    fn sweep(&self, sub: &Sub) {
        for mut v in self.by_tag.iter_mut() {
            v.retain(|w| w.upgrade().is_none_or(|s| !std::ptr::eq(s.as_ref(), sub)));
        }
        for mut v in self.by_prefix.iter_mut() {
            v.retain(|w| w.upgrade().is_none_or(|s| !std::ptr::eq(s.as_ref(), sub)));
        }
    }

    /// Subscribers that depend on `tag` (exact + wildcard, deduplicated).
    /// The caller schedules their reruns.
    #[must_use]
    pub fn affected(&self, tag: &str) -> Vec<Arc<Sub>> {
        let mut out: Vec<Arc<Sub>> = Vec::new();
        if let Some(v) = self.by_tag.get(tag) {
            for w in v.iter() {
                if let Some(s) = w.upgrade() {
                    push_unique(&mut out, s);
                }
            }
        }
        for e in &self.by_prefix {
            if tag.starts_with(e.key()) {
                for w in e.value() {
                    if let Some(s) = w.upgrade() {
                        push_unique(&mut out, s);
                    }
                }
            }
        }
        out
    }

    /// Point the subscriber at a new tag set (the rerun worker records the
    /// tags the load actually read): reindex.
    ///
    /// # Panics
    ///
    /// Panics only if the subscriber's tag mutex was poisoned by a panic
    /// in another thread while it held the lock (same contract as every
    /// `std::sync::Mutex` accessor here).
    pub fn retag(self: &Arc<Self>, sub: &Arc<Sub>, tags: BTreeSet<String>) {
        {
            let mut t = sub.tags.lock().expect("tag mutex poisoned");
            *t = tags;
        }
        self.sweep(sub.as_ref());
        self.index(sub);
    }
}

fn push_unique(out: &mut Vec<Arc<Sub>>, s: Arc<Sub>) {
    if !out.iter().any(|o| Arc::ptr_eq(o, &s)) {
        out.push(s);
    }
}

/// The injected rerun closure (§V2-F05 step 5): the CLI supplies one that
/// calls the same data-route path as `/__data` with the stored headers, so
/// middleware, locals, layout loads and gating behave identically. `None`
/// = the rerun redirected or errored (the stale signal).
pub type Rerun =
    Arc<dyn Fn(Arc<Sub>) -> futures::future::BoxFuture<'static, Option<Bytes>> + Send + Sync>;

/// The rerun worker (§V2-F05 step 5): debounces notification bursts,
/// enforces the per-subscriber rate cap, runs the injected closure, and
/// pushes the result into the subscriber's `watch` channel.
///
/// Substrate note: the debounce window is per received batch (the spec's
/// "debounces per subscriber by `debounceMs`" — with one notification
/// stream a batch window collapses the same burst into one rerun; the
/// per-subscriber rate cap below carries the per-connection bound).
pub struct Runner {
    rx: mpsc::Receiver<Arc<Sub>>,
    debounce: Duration,
    min_interval: Duration,
    rerun: Rerun,
}

impl Runner {
    /// A runner over the hub's enqueue channel: `debounce` collapses
    /// bursts, `max_reruns_per_second` caps each subscriber's rerun rate.
    #[must_use]
    pub fn new(
        rx: mpsc::Receiver<Arc<Sub>>,
        debounce: Duration,
        max_reruns_per_second: u32,
        rerun: Rerun,
    ) -> Self {
        // Rate 0 is rejected at config load (DSP-111); treat it as 1 here
        // so a hand-built runner cannot divide by zero.
        let rate = u64::from(max_reruns_per_second.max(1));
        Self {
            rx,
            debounce,
            min_interval: Duration::from_millis(1000 / rate),
            rerun,
        }
    }

    /// Drain the channel until it closes. Each notification burst inside
    /// one debounce window becomes one rerun per distinct subscriber;
    /// reruns for the same subscriber closer together than the rate cap
    /// are dropped.
    pub async fn run(mut self) {
        // Subscriber identity (the `Arc` pointer as usize — the map must
        // stay `Send` across the rerun await) → the instant of its last
        // rerun; the rate cap compares against `min_interval`.
        let mut last: HashMap<usize, std::time::Instant> = HashMap::new();
        while let Some(first) = self.rx.recv().await {
            let mut batch = vec![first];
            // Collapse everything already queued plus anything arriving
            // within the debounce window.
            let window = tokio::time::sleep(self.debounce);
            tokio::pin!(window);
            loop {
                tokio::select! {
                    maybe = self.rx.recv() => match maybe {
                        Some(next) => batch.push(next),
                        None => break,
                    },
                    () = &mut window => break,
                }
            }
            // Dedup by identity, first-seen order.
            let mut ordered: Vec<Arc<Sub>> = Vec::new();
            for s in batch {
                let key = Arc::as_ptr(&s) as usize;
                if !ordered.iter().any(|o| Arc::as_ptr(o) as usize == key) {
                    ordered.push(s);
                }
            }
            for s in ordered {
                let key = Arc::as_ptr(&s) as usize;
                let now = std::time::Instant::now();
                if last
                    .get(&key)
                    .is_some_and(|t| now.duration_since(*t) < self.min_interval)
                {
                    continue; // rate cap: drop the excess rerun
                }
                last.insert(key, now);
                let out = (self.rerun)(Arc::clone(&s)).await;
                // F06: the rerun returns the full snapshot; the wire payload
                // is the delta policy's output (ODP1 patch or the full).
                let payload = s.prepare_rerun(out);
                let _ = s.tx.send(payload);
            }
        }
    }
}

// -------------------------------------------------------------- the stream

/// The live response body (§V2-F05 step 3): the snapshot the channel
/// already holds first (a fresh connection immediately receives a full
/// `data` snapshot, so reconnect after a drop never needs replay), then
/// one `event: data` frame per new snapshot (`id:` = the per-connection
/// sequence), `event: ping` frames on the keepalive cadence, and a clean
/// end when the subscriber's sender is dropped.
///
/// Payloads are base64 (the spec's `data: <base64 ODF>` line) so the SSE
/// line-based framing is byte-safe for any snapshot. Since F06 the payload
/// bytes are either an `ODF1` full snapshot or an `ODP1` patch chained from
/// this connection's previous full ([`delta_payload`]) — the client sniffs
/// the magic and applies accordingly.
#[must_use = "the stream is the response body — dropping it ends the SSE response"]
pub fn live_stream(
    mut rx: watch::Receiver<Option<Bytes>>,
    keepalive: Duration,
) -> crate::BodyStream {
    // The first frame: whatever the channel holds right now.
    let initial = rx.borrow_and_update().clone();
    // One keepalive period after connect, not immediately (interval's
    // first tick completes at once — a fresh connection must not be
    // ping-spammed before its first data frame).
    let start = tokio::time::Instant::now() + keepalive;
    let tick = tokio::time::interval_at(start, keepalive);
    let stream = futures::stream::unfold(
        (rx, tick, initial, 0usize),
        |(mut rx, mut tick, mut initial, mut seq)| async move {
            loop {
                if let Some(snapshot) = initial.take() {
                    let frame = sse_frame(DATA_EVENT, Some(seq), &b64(&snapshot));
                    seq += 1;
                    return Some((Ok(Bytes::from(frame)), (rx, tick, initial, seq)));
                }
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            // The subscriber (sender) is gone: end the body.
                            return None;
                        }
                        // Bind the snapshot first: the `Ref` guard must not
                        // outlive this statement (the `rx` below is moved
                        // into the next unfold state).
                        let snapshot = rx.borrow_and_update().clone();
                        if let Some(snapshot) = snapshot {
                            let frame = sse_frame(DATA_EVENT, Some(seq), &b64(&snapshot));
                            seq += 1;
                            return Some((Ok(Bytes::from(frame)), (rx, tick, initial, seq)));
                        }
                        // A cleared snapshot (failed rerun): the wiring
                        // turns this into `event: stale` + close; the
                        // substrate keeps the connection silent.
                    }
                    _ = tick.tick() => {
                        return Some((
                            Ok(Bytes::from(sse_frame(PING_EVENT, None, "1"))),
                            (rx, tick, initial, seq),
                        ));
                    }
                }
            }
        },
    );
    Box::pin(stream)
}

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}
