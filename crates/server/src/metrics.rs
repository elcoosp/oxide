//! Prometheus metrics endpoint (spec §20). Exposes request rate and latency
//! histograms per route, cache hit ratios, isolate pool saturation, and a
//! process-level counter. Text exposition format; no client library required.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::Duration;

const BUCKET_BOUNDS_MS: [u64; 10] = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000];

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
        self.sum_us.fetch_add(d.as_micros() as u64, Ordering::Relaxed);
        let ms = d.as_millis() as u64;
        for (i, bound) in BUCKET_BOUNDS_MS.iter().enumerate() {
            if ms <= *bound {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// The metric registry. One per server instance.
#[derive(Debug, Default)]
pub struct Metrics {
    inner: RwLock<HashMap<String, RouteMetrics>>,
    pub requests_total: AtomicU64,
    pub errors_total: AtomicU64,
    pub cache_hits_l0: AtomicU64,
    pub cache_misses_l0: AtomicU64,
    pub isolates_recycled: AtomicU64,
    pub isolates_crashed: AtomicU64,
    /// §16.6 / §20: Core Web Vitals reported by browsers, keyed by
    /// `"<metric>/<route>"` (e.g. `"LCP//home"`). A count plus a sum of the
    /// raw value (milliseconds for the time metrics, unitless for CLS) — the
    /// `render` output exposes `_count` and `_sum` for a Prometheus summary.
    pub web_vitals: RwLock<HashMap<String, VitalAgg>>,
}

/// A reported web-vital aggregate: how many samples, and their summed value.
#[derive(Debug, Default, Clone, Copy)]
pub struct VitalAgg {
    pub count: u64,
    pub sum: f64,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&self, route: &str, latency: Duration, status: u16) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        if status >= 500 {
            self.errors_total.fetch_add(1, Ordering::Relaxed);
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

    pub fn render(&self) -> String {
        let mut out = String::with_capacity(1024);
        out.push_str("# HELP oxide_requests_total Total requests served\n");
        out.push_str("# TYPE oxide_requests_total counter\n");
        out.push_str(&format!("oxide_requests_total {}\n", self.requests_total.load(Ordering::Relaxed)));

        out.push_str("# HELP oxide_errors_total Total 5xx responses\n");
        out.push_str("# TYPE oxide_errors_total counter\n");
        out.push_str(&format!("oxide_errors_total {}\n", self.errors_total.load(Ordering::Relaxed)));

        out.push_str("# HELP oxide_cache_l0_hits_total L0 cache hits\n");
        out.push_str("# TYPE oxide_cache_l0_hits_total counter\n");
        out.push_str(&format!("oxide_cache_l0_hits_total {}\n", self.cache_hits_l0.load(Ordering::Relaxed)));
        out.push_str("# HELP oxide_cache_l0_misses_total L0 cache misses\n");
        out.push_str("# TYPE oxide_cache_l0_misses_total counter\n");
        out.push_str(&format!("oxide_cache_l0_misses_total {}\n", self.cache_misses_l0.load(Ordering::Relaxed)));

        out.push_str("# HELP oxide_isolates_recycled_total Isolate recycle events\n");
        out.push_str("# TYPE oxide_isolates_recycled_total counter\n");
        out.push_str(&format!("oxide_isolates_recycled_total {}\n", self.isolates_recycled.load(Ordering::Relaxed)));
        out.push_str("# HELP oxide_isolates_crashed_total Isolate crash events\n");
        out.push_str("# TYPE oxide_isolates_crashed_total counter\n");
        out.push_str(&format!("oxide_isolates_crashed_total {}\n", self.isolates_crashed.load(Ordering::Relaxed)));

        {
            let vitals = self.web_vitals.read().expect("vitals lock");
            if !vitals.is_empty() {
                out.push_str("# HELP oxide_web_vital_count Web-vital samples by metric and route\n");
                out.push_str("# TYPE oxide_web_vital_count counter\n");
                out.push_str("# HELP oxide_web_vital_sum Summed web-vital values\n");
                out.push_str("# TYPE oxide_web_vital_sum counter\n");
                let mut keys: Vec<&String> = vitals.keys().collect();
                keys.sort();
                for k in keys {
                    let (name, route) = k.split_once('/').unwrap_or((k.as_str(), ""));
                    let agg = vitals[k];
                    out.push_str(&format!(
                        "oxide_web_vital_count{{metric=\"{name}\",route=\"{route}\"}} {}\n",
                        agg.count
                    ));
                    out.push_str(&format!(
                        "oxide_web_vital_sum{{metric=\"{name}\",route=\"{route}\"}} {}\n",
                        agg.sum
                    ));
                }
            }
        }

        let map = self.inner.read().expect("metrics lock");
        out.push_str("# HELP oxide_request_duration_ms Request latency by route\n");
        out.push_str("# TYPE oxide_request_duration_ms histogram\n");
        for (route, m) in map.iter() {
            let count = m.count.load(Ordering::Relaxed);
            for (i, bound) in BUCKET_BOUNDS_MS.iter().enumerate() {
                out.push_str(&format!(
                    "oxide_request_duration_ms_bucket{{route=\"{route}\",le=\"{bound}\"}} {}\n",
                    m.buckets[i].load(Ordering::Relaxed)
                ));
            }
            out.push_str(&format!(
                "oxide_request_duration_ms_bucket{{route=\"{route}\",le=\"+Inf\"}} {count}\n"
            ));
            out.push_str(&format!(
                "oxide_request_duration_ms_sum{{route=\"{route}\"}} {}\n",
                (m.sum_us.load(Ordering::Relaxed) as f64) / 1000.0
            ));
            out.push_str(&format!(
                "oxide_request_duration_ms_count{{route=\"{route}\"}} {count}\n"
            ));
        }
        out
    }
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
}
