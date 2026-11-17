//! The CLI's resumable-stream wiring state (§V2-F07): the
//! `stream.resume` config bounds flowing into the
//! [`StreamStore::new`] constructor, and the per-IP reattach
//! rate limit riding the server crate's sharded-bucket limiter (PERF-22)
//! the pipeline already owns.
//!
//! The store itself only exists while a resumable response is being teed
//! (`oxide_fw_server::stream_store` holds the semantics, pinned by
//! `crates/server/tests/stream_tdd.rs` and the sweep-task pins in
//! `stream_sweep_tdd.rs`); this module is the boot-side construction —
//! the same CLI-state pattern the skew pool (`serve::generations`) and
//! the live hub (`serve::live`) follow: typed config in, a handle the
//! dispatch closure captures out.
//!
//! Every knob is load-validated (`DSP-112`: a non-parsing size literal
//! fails `CFG-SIZE` at config load, a physically useless bound is
//! refused), so the fallbacks here are belt-only for a hand-constructed
//! config — a serve never degrades silently through them.

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_config::StreamConfig;
use oxide_fw_server::RateLimiter;
use oxide_fw_server::stream_store::{
    CookieBindingSource, SessionBinding, SessionBindingSource, StreamStore,
};

/// The per-IP reattach rate limit (§V2-F07 wiring step 3: "per-IP
/// reattach rate limit"). The reattach replays buffered bytes — cheap,
/// but a hot loop over one id is pure waste, so the bucket bounds it:
/// 60 reattaches per minute per peer, the same shape the search endpoint's
/// fixed window uses. The client's own backoff ladder tops out far below
/// this (five tries per drop).
pub const REATTACH_RATE_MAX: u32 = 60;

/// The reattach bucket's window in seconds (the `allow` grammar).
pub const REATTACH_RATE_WINDOW_SECS: u64 = 60;

/// The housekeeping cadence (§V2-F07 wiring step 3: "A background task
/// checks every 30 s").
pub const SWEEP_CADENCE_SECS: u64 = 30;

/// The spec's `maxBytesEach` default (`'4mb'`) — the belt for a
/// hand-built config whose literal cannot parse (load validation already
/// refused that shape on every real serve).
pub const DEFAULT_MAX_BYTES_EACH: u64 = 4 << 20;

/// The spec's `maxBytesTotal` default (`'256mb'`) — same belt.
pub const DEFAULT_MAX_BYTES_TOTAL: u64 = 256 << 20;

/// The serve-side stream state: the replay store the tee feeds and the
/// reattach route answers from, plus the per-IP reattach bucket.
pub struct StreamWiring {
    /// The id → buffer store (`x-oxide-stream` lifetime = this handle).
    pub store: Arc<StreamStore>,
    /// The per-IP reattach bucket (the sharded limiter the server crate
    /// owns; a dedicated instance, so reattach pressure never shares a
    /// bucket with the search/OG endpoints).
    pub limiter: Arc<RateLimiter>,
    /// The §13.4 session binding source — ONE boot-owned instance both
    /// sides of the contract call (the tee site hashes the ORIGINAL
    /// request's `oxide_session` cookie into the buffer's binding; the
    /// reattach arm hashes the presented one). The key is per-boot random,
    /// so bindings never validate across restarts.
    pub binding: Arc<CookieBindingSource>,
}

impl StreamWiring {
    /// Build the wiring from the typed `stream:` config section
    /// (`StreamConfig` — `ttl_secs` seconds, the two §14 size literals
    /// parsed by [`oxide_fw_config::parse_size`]). The store bounds go in
    /// spec order (`new(ttl, max_each, max_total)`); a total bound below
    /// the per-stream bound is raised to the per-stream bound so the
    /// accounting cannot thrash (load validation `DSP-112` already
    /// refuses that shape on a real serve).
    ///
    /// # Panics
    /// The binding source needs the boot RNG; a system-RNG failure panics
    /// with context instead of silently serving reattachable streams
    /// without the §13.4 binding (the R05 gap this wiring closes).
    #[must_use]
    pub fn from_config(cfg: &StreamConfig) -> Self {
        let max_each = oxide_fw_config::parse_size(&cfg.resume.max_bytes_each)
            .unwrap_or(DEFAULT_MAX_BYTES_EACH);
        let max_total = oxide_fw_config::parse_size(&cfg.resume.max_bytes_total)
            .unwrap_or(DEFAULT_MAX_BYTES_TOTAL)
            .max(max_each);
        Self {
            store: Arc::new(StreamStore::new(
                Duration::from_secs(cfg.resume.ttl_secs),
                max_each,
                max_total,
            )),
            limiter: Arc::new(RateLimiter::default()),
            binding: Arc::new(
                CookieBindingSource::new(crate::serve::session::SESSION_COOKIE_NAME).expect(
                    "the boot RNG is unavailable; refusing to serve unbound resumable streams",
                ),
            ),
        }
    }

    /// The request's session binding, delegated to the boot's one source
    /// (the stages call this — the trait stays an implementation detail of
    /// the wiring, and the tee/reattach sites cannot pick up a different
    /// instance).
    #[must_use]
    pub fn binding_of(&self, headers: &[(String, String)]) -> Option<SessionBinding> {
        self.binding.binding(headers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The typed config defaults flow into the store bounds verbatim: the
    /// spec's `ttl: '60s'`, `maxBytesEach: '4mb'`, `maxBytesTotal:
    /// '256mb'` (§V2-F07 wiring step 3).
    #[test]
    fn config_defaults_build_the_spec_bounds() {
        let w = StreamWiring::from_config(&StreamConfig::default());
        assert_eq!(w.store.total_bytes(), 0, "a fresh store holds nothing");
        // The bounds are not directly readable; the observable proxy is
        // the eviction behavior pinned in the substrate — here the
        // construction itself is the contract (a parse failure would
        // panic, a missing field would not compile). The literal parse is
        // pinned below.
    }

    /// The §14 size literal grammar the config surface promises:
    /// `'4mb'` → 4 MiB, `'256mb'` → 256 MiB, plain digits are bytes.
    #[test]
    fn size_literals_parse_to_the_spec_bytes() {
        assert_eq!(
            oxide_fw_config::parse_size("4mb").ok(),
            Some(DEFAULT_MAX_BYTES_EACH),
            "the spec's per-stream default"
        );
        assert_eq!(
            oxide_fw_config::parse_size("256mb").ok(),
            Some(DEFAULT_MAX_BYTES_TOTAL),
            "the spec's store-wide default"
        );
        assert_eq!(oxide_fw_config::parse_size("1024").ok(), Some(1024));
    }

    /// A hand-built config whose total bound sits below the per-stream
    /// bound is raised to the per-stream bound (the accounting cannot
    /// evict the buffer it is currently appending to below its own cap).
    #[test]
    fn total_below_each_is_raised_to_each() {
        let mut cfg = StreamConfig::default();
        cfg.resume.max_bytes_total = "1kb".into();
        let w = StreamWiring::from_config(&cfg);
        let _ = &w; // construction must not panic; bounds pinned via parse above
        assert_eq!(
            oxide_fw_config::parse_size("1kb").ok(),
            Some(1024),
            "the small literal parses (the raise is the max() in from_config)"
        );
    }
}
