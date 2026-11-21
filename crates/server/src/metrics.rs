//! Prometheus metrics endpoint (spec §20). Exposes request rate and latency
//! histograms per route, cache hit ratios, isolate pool saturation, and a
//! process-level counter. Text exposition format; no client library required.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

const BUCKET_BOUNDS_MS: [u64; 10] = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000];

/// WP B03 item 4: the dynamic labels the dispatch stages observe but the
/// boot's `register_routes` caller does not hand in (it passes route
/// patterns, which begin with `/`). `_unmatched` is the 404 arm's label,
/// `_proxy` the prelude's proxy forward, `_data` the `GET /__data` route.
/// Freezing them alongside the route patterns keeps their `observe` lock-free
/// — a 404, a proxy pass and a data-plane request pay the same atomic-only
/// `observe` a registered route does, instead of the fallback write lock.
const DYNAMIC_LABELS: [&str; 3] = ["_unmatched", "_proxy", "_data"];

#[derive(Debug)]
struct RouteMetrics {
    count: AtomicU64,
    sum_us: AtomicU64,
    buckets: [AtomicU64; 10],
}

impl RouteMetrics {
    fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            sum_us: AtomicU64::new(0),
            buckets: Default::default(),
        }
    }
    fn observe(&self, d: Duration) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_us
            .fetch_add(d.as_micros() as u64, Ordering::Relaxed);
        let ms = d.as_millis() as u64;
        for (i, bound) in BUCKET_BOUNDS_MS.iter().enumerate() {
            if ms <= *bound {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// SEC-07: escape `\`, `"` and `\n` in Prometheus label values.
fn prom_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

/// The metric registry. One per server instance.
#[derive(Debug, Default)]
pub struct Metrics {
    inner: RwLock<HashMap<String, RouteMetrics>>,
    /// §V2-B03 item 4: the frozen route-label map `register_routes` fills
    /// before the server starts and nothing mutates afterwards, so a
    /// registered label's `observe` takes NO lock (the fallback
    /// `RwLock` map below stays for the dynamic labels — `_unmatched`,
    /// `_proxy`, `_data` — and any route the pre-start registration missed).
    frozen: std::sync::OnceLock<HashMap<std::sync::Arc<str>, RouteMetrics>>,
    pub requests_total: AtomicU64,
    pub errors_total: AtomicU64,
    pub cache_hits_l0: AtomicU64,
    pub cache_misses_l0: AtomicU64,
    pub isolates_recycled: AtomicU64,
    pub isolates_crashed: AtomicU64,
    /// §20 / PERF-02: the JS pool's live gauges. Updated by the dispatch
    /// delegate from `Pool::stats()`; a gauge, not a counter.
    pub pool_workers: AtomicU64,
    pub pool_in_flight: AtomicU64,
    pub pool_queue_wait_p99_us: AtomicU64,
    /// §V2-F12: the brownout governor's published level (0 Normal … 3
    /// Emergency). Updated by the sampler tick task; a gauge, not a
    /// counter. The enforcement stage and dashboards read the exposition
    /// to correlate shed responses with the level that caused them.
    pub brownout_level: AtomicU64,
    /// §16.6 / §20: Core Web Vitals reported by browsers, keyed by
    /// `"<metric>/<route>"` (e.g. `"LCP//home"`). A count plus a sum of the
    /// raw value (milliseconds for the time metrics, unitless for CLS) — the
    /// `render` output exposes `_count` and `_sum` for a Prometheus summary.
    pub web_vitals: RwLock<HashMap<String, VitalAgg>>,
    /// §V2-F04: the experiment-exposure counters, keyed
    /// `"<experiment>/<variant>"` (the pair the dispatch's experiment stage
    /// assigns). exposition: `oxide_experiment_assignment{experiment,variant}`.
    pub experiments: RwLock<HashMap<String, AtomicU64>>,
    /// F37: the attached cache-plane exposition (`set_cache_plane`) — the
    /// boot's closure renders the cache plane's families per scrape; `None`
    /// (the default) appends nothing.
    cache_plane: CachePlane,
}

/// The F37 slot: a locked option over the attached exposition. The newtype
/// carries the derive pair (Debug names attachment, Default is unattached)
/// the `dyn Fn` field cannot.
#[derive(Debug, Default)]
struct CachePlane(std::sync::RwLock<Option<CachePlaneExposition>>);

/// A reported web-vital aggregate: how many samples, and their summed value.
#[derive(Debug, Default, Clone, Copy)]
pub struct VitalAgg {
    pub count: u64,
    pub sum: f64,
}

/// The attached cache-plane exposition (F37): an `Fn() -> String` the serve
/// boot fills with the cache plane's Prometheus families. The closure is
/// opaque (Debug prints attachment only) and the slot is OPTIONAL (the
/// wrapping `CachePlane` newtype is `Default` through the `RwLock<Option<_>>`
/// — `None` renders nothing) — the registry is byte-identical to the
/// pre-slot shape until a boot attaches a plane.
struct CachePlaneExposition(Arc<dyn Fn() -> String + Send + Sync>);

impl std::fmt::Debug for CachePlaneExposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CachePlaneExposition(attached)")
    }
}

impl Metrics {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The §20 cache-plane exposition slot (F37): the serve boot hands a
    /// closure rendering the cache plane's Prometheus families — the F30
    /// exporter's `oxide_cache_plane_*` text — and every `render()` appends
    /// its rows beside the request-plane rows.
    ///
    /// The slot is deliberately an `Fn() -> String`, not a typed dependency:
    /// the server crate must not reach into the cache plane (the dead
    /// `oxide-fw-cache` dependency stays out), so the host that owns both —
    /// the CLI boot — does the composition. The closure runs per render (a
    /// scrape is a pull: it reads the plane's posture as it stands NOW,
    /// never a value frozen at attach), and the LAST attach wins (a second
    /// `set_cache_plane` replaces the first). An unattached registry renders
    /// byte-identical to the pre-slot shape.
    pub fn set_cache_plane(&self, render: impl Fn() -> String + Send + Sync + 'static) {
        let mut slot = self.cache_plane.0.write().expect("cache plane lock");
        *slot = Some(CachePlaneExposition(Arc::new(render)));
    }

    pub fn observe(&self, route: &str, latency: Duration, status: u16) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        if status >= 500 {
            self.errors_total.fetch_add(1, Ordering::Relaxed);
        }
        // §V2-B03 item 4: the frozen registered map answers without a lock;
        // `RouteMetrics::observe` is atomic-only.
        if let Some(m) = self.frozen.get().and_then(|map| map.get(route)) {
            m.observe(latency);
            return;
        }
        {
            let map = self.inner.read().expect("metrics lock");
            if let Some(m) = map.get(route) {
                m.observe(latency);
                return;
            }
        }
        let mut map = self.inner.write().expect("metrics lock");
        map.entry(route.to_string())
            .or_insert_with(RouteMetrics::new)
            .observe(latency);
    }

    /// §V2-B03 item 4 (test-only): whether `register_routes` froze a label.
    /// The hot path's `observe` answers through the frozen map with no
    /// lock, so a frozen label never contends on the fallback `RwLock`.
    /// The dynamic labels (`_unmatched`, `_proxy`, `_data`) fire on every
    /// unmatched / proxy / data request, so freezing them too (not just the
    /// route patterns the boot hands in) is what keeps those paths lock-free.
    #[cfg(test)]
    pub fn is_frozen_label(&self, label: &str) -> bool {
        self.frozen.get().is_some_and(|map| map.contains_key(label))
    }

    /// §V2-B03 item 4: freeze the route labels BEFORE the server starts.
    /// Every `RouteContext.pattern` the boot built goes in; the map is
    /// never mutated afterwards, so a registered label's `observe` needs
    /// no lock (the fallback `RwLock` map keeps serving anything the
    /// registration missed). A second call after the map is frozen is a
    /// no-op (the first registration wins).
    ///
    /// The three dynamic labels the dispatch stages observe — `_unmatched`
    /// (the 404 arm), `_proxy` (the prelude's proxy forward) and `_data`
    /// (the `GET /__data` route) — are frozen ALONGSIDE the route patterns
    /// so they never reach the fallback write lock either: a 404, a proxy
    /// pass and a data-plane request pay the same atomic-only `observe` a
    /// registered route does. Route patterns begin with `/`; the dynamic
    /// labels never do, so the two sets cannot collide.
    pub fn register_routes(&self, labels: impl IntoIterator<Item = std::sync::Arc<str>>) {
        let mut map: HashMap<std::sync::Arc<str>, RouteMetrics> = labels
            .into_iter()
            .map(|label| (label, RouteMetrics::new()))
            .collect();
        for dynamic in DYNAMIC_LABELS {
            map.entry(std::sync::Arc::from(dynamic))
                .or_insert_with(RouteMetrics::new);
        }
        let _ = self.frozen.set(map);
    }

    /// PERF-02: record the pool's current gauges (worker count, in-flight,
    /// queue-wait p99). Called from the dispatch delegate each request.
    pub fn observe_pool(&self, workers: usize, in_flight: u64, queue_wait_p99_ms: f64) {
        self.pool_workers.store(workers as u64, Ordering::Relaxed);
        self.pool_in_flight.store(in_flight, Ordering::Relaxed);
        self.pool_queue_wait_p99_us.store(
            (queue_wait_p99_ms.max(0.0) * 1000.0) as u64,
            Ordering::Relaxed,
        );
    }

    /// §V2-F12: publish the governor's level after each sampler tick. The
    /// numeric level is the same payload the `x-oxide-brownout` header
    /// carries (0 Normal … 3 Emergency).
    pub fn set_brownout_level(&self, level: u8) {
        self.brownout_level
            .store(u64::from(level), Ordering::Relaxed);
    }

    /// §16.6: record one browser web-vital sample. `name` is the metric
    /// (`LCP`/`CLS`/`INP`/`FCP`/`TTFB`), `route` the page it was measured on,
    /// `value` the raw number (ms for timing metrics, unitless for CLS).
    pub fn observe_vital(&self, name: &str, route: &str, value: f64) {
        if !value.is_finite() {
            return;
        }
        let key = format!("{name}/{route}");
        let mut map = self.web_vitals.write().expect("vitals lock");
        let agg = map.entry(key).or_default();
        agg.count = agg.count.saturating_add(1);
        agg.sum += value;
    }

    /// §V2-F04: count one experiment exposure — the dispatch's experiment
    /// stage calls this per assigned (experiment, variant) pair after the
    /// §15.4 evaluation. Counters are per pair, so the exposition splits
    /// traffic per cohort.
    ///
    /// # Panics
    ///
    /// Only if the experiments lock is poisoned (a prior holder panicked
    /// mid-update) — the same contract the registry's other maps carry.
    pub fn observe_experiment(&self, experiment: &str, variant: &str) {
        let key = format!("{experiment}/{variant}");
        let map = self.experiments.read().expect("experiments lock");
        if let Some(c) = map.get(&key) {
            c.fetch_add(1, Ordering::Relaxed);
            return;
        }
        drop(map);
        let mut map = self.experiments.write().expect("experiments lock");
        map.entry(key)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn render(&self) -> String {
        let mut out = String::with_capacity(1024);
        out.push_str("# HELP oxide_requests_total Total requests served\n");
        out.push_str("# TYPE oxide_requests_total counter\n");
        out.push_str(&format!(
            "oxide_requests_total {}\n",
            self.requests_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP oxide_errors_total Total 5xx responses\n");
        out.push_str("# TYPE oxide_errors_total counter\n");
        out.push_str(&format!(
            "oxide_errors_total {}\n",
            self.errors_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP oxide_cache_l0_hits_total L0 cache hits\n");
        out.push_str("# TYPE oxide_cache_l0_hits_total counter\n");
        out.push_str(&format!(
            "oxide_cache_l0_hits_total {}\n",
            self.cache_hits_l0.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP oxide_cache_l0_misses_total L0 cache misses\n");
        out.push_str("# TYPE oxide_cache_l0_misses_total counter\n");
        out.push_str(&format!(
            "oxide_cache_l0_misses_total {}\n",
            self.cache_misses_l0.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP oxide_pool_workers Live worker count in the JS pool\n");
        out.push_str("# TYPE oxide_pool_workers gauge\n");
        out.push_str(&format!(
            "oxide_pool_workers {}\n",
            self.pool_workers.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP oxide_pool_in_flight In-flight calls in the JS pool\n");
        out.push_str("# TYPE oxide_pool_in_flight gauge\n");
        out.push_str(&format!(
            "oxide_pool_in_flight {}\n",
            self.pool_in_flight.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP oxide_pool_queue_wait_p99_us Pool queue-wait p99 (microseconds)\n");
        out.push_str("# TYPE oxide_pool_queue_wait_p99_us gauge\n");
        out.push_str(&format!(
            "oxide_pool_queue_wait_p99_us {}\n",
            self.pool_queue_wait_p99_us.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP oxide_brownout_level The governor's current brownout level (0 Normal, 1 Stale, 2 Shed, 3 Emergency)\n");
        out.push_str("# TYPE oxide_brownout_level gauge\n");
        let _ = writeln!(
            out,
            "oxide_brownout_level {}",
            self.brownout_level.load(Ordering::Relaxed)
        );

        out.push_str("# HELP oxide_isolates_recycled_total Isolate recycle events\n");
        out.push_str("# TYPE oxide_isolates_recycled_total counter\n");
        out.push_str(&format!(
            "oxide_isolates_recycled_total {}\n",
            self.isolates_recycled.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP oxide_isolates_crashed_total Isolate crash events\n");
        out.push_str("# TYPE oxide_isolates_crashed_total counter\n");
        out.push_str(&format!(
            "oxide_isolates_crashed_total {}\n",
            self.isolates_crashed.load(Ordering::Relaxed)
        ));

        {
            let vitals = self.web_vitals.read().expect("vitals lock");
            if !vitals.is_empty() {
                out.push_str(
                    "# HELP oxide_web_vital_count Web-vital samples by metric and route\n",
                );
                out.push_str("# TYPE oxide_web_vital_count counter\n");
                out.push_str("# HELP oxide_web_vital_sum Summed web-vital values\n");
                out.push_str("# TYPE oxide_web_vital_sum counter\n");
                let mut keys: Vec<&String> = vitals.keys().collect();
                keys.sort();
                for k in keys {
                    let (name, route) = k.split_once('/').unwrap_or((k.as_str(), ""));
                    let agg = vitals[k];
                    let name_e = prom_escape(name);
                    let route_e = prom_escape(route);
                    out.push_str(&format!(
                        "oxide_web_vital_count{{metric=\"{name_e}\",route=\"{route_e}\"}} {}\n",
                        agg.count
                    ));
                    out.push_str(&format!(
                        "oxide_web_vital_sum{{metric=\"{name_e}\",route=\"{route_e}\"}} {}\n",
                        agg.sum
                    ));
                }
            }
        }

        {
            // §V2-F04: the experiment-exposure counters, per (experiment,
            // variant) pair, sorted for a stable exposition.
            let experiments = self.experiments.read().expect("experiments lock");
            if !experiments.is_empty() {
                out.push_str("# HELP oxide_experiment_assignment Experiment exposures by experiment and variant\n");
                out.push_str("# TYPE oxide_experiment_assignment counter\n");
                let mut keys: Vec<&String> = experiments.keys().collect();
                keys.sort();
                for k in keys {
                    let (experiment, variant) = k.split_once('/').unwrap_or((k.as_str(), ""));
                    let experiment_e = prom_escape(experiment);
                    let variant_e = prom_escape(variant);
                    let _ = writeln!(
                        out,
                        "oxide_experiment_assignment{{experiment=\"{experiment_e}\",variant=\"{variant_e}\"}} {}",
                        experiments[k].load(Ordering::Relaxed)
                    );
                }
            }
        }

        out.push_str("# HELP oxide_request_duration_ms Request latency by route\n");
        out.push_str("# TYPE oxide_request_duration_ms histogram\n");
        // §V2-B03 item 4: the frozen registered labels render too (a zero
        // histogram is the "route exists" signal dashboards expect), in a
        // stable sorted order; the fallback map's dynamic labels follow,
        // skipping anything the frozen map already reported.
        let frozen = self.frozen.get();
        if let Some(frozen_map) = frozen {
            let mut labels: Vec<(&str, &RouteMetrics)> =
                frozen_map.iter().map(|(k, v)| (&**k, v)).collect();
            labels.sort_by_key(|(k, _)| *k);
            for (route, m) in labels {
                render_route_histogram(&mut out, route, m);
            }
        }
        let map = self.inner.read().expect("metrics lock");
        for (route, m) in map.iter() {
            if frozen.is_some_and(|f| f.contains_key(route.as_str())) {
                continue;
            }
            render_route_histogram(&mut out, route, m);
        }
        // F37: the attached cache-plane exposition renders LAST — the
        // request-plane exposition above stays byte-stable, and one scrape
        // reads both planes. Unattached (the default): nothing appended.
        if let Some(plane) = self
            .cache_plane
            .0
            .read()
            .expect("cache plane lock")
            .as_ref()
        {
            out.push_str(&(plane.0)());
        }
        out
    }
}

/// One route's `oxide_request_duration_ms` histogram lines (buckets, sum,
/// count) — the §V2-B03 render union's shared body for the frozen and the
/// fallback maps.
fn render_route_histogram(out: &mut String, route: &str, m: &RouteMetrics) {
    let count = m.count.load(Ordering::Relaxed);
    let route_e = prom_escape(route);
    for (i, bound) in BUCKET_BOUNDS_MS.iter().enumerate() {
        out.push_str(&format!(
            "oxide_request_duration_ms_bucket{{route=\"{route_e}\",le=\"{bound}\"}} {}\n",
            m.buckets[i].load(Ordering::Relaxed)
        ));
    }
    out.push_str(&format!(
        "oxide_request_duration_ms_bucket{{route=\"{route_e}\",le=\"+Inf\"}} {count}\n"
    ));
    out.push_str(&format!(
        "oxide_request_duration_ms_sum{{route=\"{route_e}\"}} {}\n",
        (m.sum_us.load(Ordering::Relaxed) as f64) / 1000.0
    ));
    out.push_str(&format!(
        "oxide_request_duration_ms_count{{route=\"{route_e}\"}} {count}\n"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_renders_zero() {
        let s = Metrics::new().render();
        assert!(s.contains("# TYPE oxide_requests_total counter"));
        assert!(s.contains("oxide_requests_total 0"));
    }

    // §V2-B03 item 4: a REGISTERED route label observes through the frozen
    // map without touching the fallback lock — hold that lock for write in
    // another thread and assert `observe` on the registered label still
    // returns (a regression would deadlock and the recv_timeout fails).
    #[test]
    fn observe_registered_route_takes_no_lock() {
        use std::sync::Arc as StdArc;
        use std::sync::mpsc;
        let m = std::sync::Arc::new(Metrics::new());
        m.register_routes([StdArc::from("/blog")]);
        let (tx, rx) = mpsc::channel::<()>();
        let guard_m = std::sync::Arc::clone(&m);
        let guard = std::thread::spawn(move || {
            // Hold the fallback map's write lock while the observe runs.
            let _g = guard_m.inner.write().expect("metrics lock");
            tx.send(()).expect("guard thread signals");
            std::thread::sleep(std::time::Duration::from_millis(300));
        });
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .expect("the guard thread took the write lock");
        m.observe("/blog", std::time::Duration::from_millis(4), 200);
        guard.join().expect("guard thread joined");
        assert_eq!(m.requests_total.load(Ordering::Relaxed), 1);
        // An UNREGISTERED label still goes through the fallback path.
        m.observe("/dynamic", std::time::Duration::from_millis(1), 200);
        assert_eq!(m.requests_total.load(Ordering::Relaxed), 2);
    }

    // §V2-B03 item 4 (WP B03 completion): the dynamic labels the dispatch
    // stages observe — `_unmatched` (the 404 arm), `_proxy` (the prelude's
    // proxy forward) and `_data` (the `GET /__data` route) — fire on EVERY
    // unmatched / proxy / data request. If they are NOT in the frozen map
    // `register_routes` builds, each first-sight observe takes the fallback
    // `RwLock` WRITE lock (insert on miss), contending the whole registry.
    // Freezing them alongside the route patterns keeps those paths
    // atomic-only — the lock-free pattern the spec item 4 asks for beyond
    // the registered-route case.
    #[test]
    fn register_routes_freezes_dynamic_labels() {
        let m = Metrics::new();
        m.register_routes([std::sync::Arc::from("/blog")]);
        assert!(
            m.is_frozen_label("/blog"),
            "a registered route label is frozen"
        );
        assert!(
            m.is_frozen_label("_unmatched"),
            "_unmatched must be frozen so the 404 arm's observe takes no lock"
        );
        assert!(
            m.is_frozen_label("_proxy"),
            "_proxy must be frozen so the proxy forward's observe takes no lock"
        );
        assert!(
            m.is_frozen_label("_data"),
            "_data must be frozen so the data route's observe takes no lock"
        );
        assert!(
            !m.is_frozen_label("/dynamic"),
            "an unregistered label still falls back to the locked map"
        );
    }

    // V2-F04 (§15.4): the experiment-exposure counter. Each (experiment,
    // variant) pair accumulates on assignment and the exposition names the
    // pair as labels, so a dashboard can split exposure per cohort.
    #[test]
    fn experiment_assignments_render_per_pair_counters() {
        let m = Metrics::new();
        m.observe_experiment("hero", "control");
        m.observe_experiment("hero", "control");
        m.observe_experiment("hero", "treatment");
        let s = m.render();
        assert!(
            s.contains("# TYPE oxide_experiment_assignment counter"),
            "the assignment counter type line: {s}"
        );
        assert!(
            s.contains("oxide_experiment_assignment{experiment=\"hero\",variant=\"control\"} 2"),
            "the control arm counts both exposures: {s}"
        );
        assert!(
            s.contains("oxide_experiment_assignment{experiment=\"hero\",variant=\"treatment\"} 1"),
            "the treatment arm counts its exposure: {s}"
        );
        // Label values go through the SEC-07 escape (a quote in a variant
        // name cannot break the exposition format).
        m.observe_experiment("q", "v\"2");
        let s = m.render();
        assert!(
            s.contains("oxide_experiment_assignment{experiment=\"q\",variant=\"v\\\"2\"} 1"),
            "the escaped variant renders: {s}"
        );
    }

    #[test]
    fn observe_accumulates_by_route() {
        let m = Metrics::new();
        m.observe("/", Duration::from_millis(3), 200);
        m.observe("/", Duration::from_millis(7), 200);
        m.observe("/api", Duration::from_millis(30), 500);
        let s = m.render();
        assert!(s.contains("oxide_requests_total 3"));
        assert!(s.contains("oxide_errors_total 1"));
        assert!(s.contains("oxide_request_duration_ms_count{route=\"/\"} 2"));
        assert!(s.contains("oxide_request_duration_ms_bucket{route=\"/\",le=\"10\"} 2"));
    }

    #[test]
    fn plus_inf_bucket_matches_count() {
        let m = Metrics::new();
        for _ in 0..5 {
            m.observe("/x", Duration::from_millis(1), 200);
        }
        assert!(m.render().contains("route=\"/x\",le=\"+Inf\"} 5"));
    }

    /// §V2-F12: the governor's level travels the exposition as a gauge.
    #[test]
    fn brownout_level_gauge_round_trips() {
        let m = Metrics::new();
        assert!(m.render().contains("oxide_brownout_level 0"));
        m.set_brownout_level(2);
        assert!(m.render().contains("oxide_brownout_level 2"));
        m.set_brownout_level(3);
        assert!(m.render().contains("oxide_brownout_level 3"));
    }
}

#[cfg(test)]
mod sec07_tests {
    use super::*;

    #[test]
    fn prom_escape_neutralizes_injection() {
        assert_eq!(prom_escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn hostile_route_renders_inert() {
        let m = Metrics::new();
        m.observe("/x\",le=\"1\"} 99\n# evil", Duration::from_millis(1), 200);
        let s = m.render();
        // No raw newline may survive inside a label value (else the attacker
        // injects a fake `# evil` exposition line).
        for line in s.lines() {
            assert!(
                !line.starts_with("# evil"),
                "injected exposition line must be escaped: {line}"
            );
        }
        assert!(s.contains("\\n# evil"), "newline must be escaped: {s}");
        assert!(s.contains("\\\""), "quote must be escaped");
    }
}
