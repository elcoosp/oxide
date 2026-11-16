//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// §9.2 / §20 / O3c: the wall-clock time the parallel load path saved.
/// When the last-layout load and the page load run under `tokio::join!`,
/// the total load phase is `max(a, b)` instead of `a + b`; the saving is
/// `min(a, b)`. Zero when they are the same duration (nothing saved) or
/// when the sequential path ran (caller passes 0.0 for one side).
///
/// Pure so the arithmetic is testable without a real server: the
/// arithmetic *is* the marking, the recorder just carries the number.
pub(crate) fn parallel_load_savings_ms(a_ms: f64, b_ms: f64) -> f64 {
    a_ms.min(b_ms).max(0.0)
}

pub(crate) fn session_ttl_secs() -> u64 {
    SESSION_TTL_SECS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set the session cookie TTL. Called by the CLI at server start from
/// `cfg.security.session_ttl_secs`.
pub(crate) fn set_session_ttl(ttl: u64) {
    SESSION_TTL_SECS.store(ttl, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn encode_session_cookie(
    after: Option<&serde_json::Value>,
    before: Option<&serde_json::Value>,
    codec: Option<&oxide_fw_server::SessionCodec>,
) -> Option<String> {
    let codec = codec?;
    let after = after?;
    let ttl_secs = session_ttl_secs();
    // Unchanged object → no cookie. `Some({})` (the worker's default for a
    // project with no inbound cookie) equals `None` before → not a change.
    let is_empty_object = after.as_object().map(|o| o.is_empty()).unwrap_or(false);
    let changed = match (after, before) {
        (a, Some(b)) => a != b,
        (a, None) => !is_empty_object && !a.is_null(),
    };
    if !changed {
        return None;
    }
    let mut sess = oxide_fw_server::Session::new();
    if let Some(obj) = after.as_object() {
        for (k, v) in obj {
            sess.insert(k.clone(), v.clone());
        }
    }
    match sess.encode(codec) {
        Ok(token) => Some(format!(
            "oxide_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={ttl_secs}"
        )),
        Err(e) => {
            tracing::warn!(error = %e.0.message, "session encode failed; cookie not set");
            None
        }
    }
}
