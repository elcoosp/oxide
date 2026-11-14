//! The record-and-replay playback substrate (spec v0.2, V2-F15 replay
//! half).
//!
//! §V2-F15: the replay re-executes a captured request deterministically.
//! The worker side installs `globalThis.__oxide_replay` when started with
//! `OXIDE_REPLAY=<file>`; this module is the V8-free playback half the
//! worker and the `oxide replay` command drive:
//!
//! - [`ReplayMatcher`]: the recorded outbound fetch ops, played IN ORDER
//!   ("matched by method, URL and body hash in order; an unexpected call
//!   is an error `RPL-002` naming the call"). A failed take consumes
//!   nothing — the retry with the correct call plays on.
//! - [`ReplaySequences`]: the recorded `Date.now` / `Math.random` /
//!   `crypto.randomUUID` streams, in observation order; a drained stream
//!   is `RPL-004` (the replay made more non-deterministic observations
//!   than the record captured).
//! - [`warn_stale_build`]: "a record from an older build id warns
//!   (`RPL-003`) but runs" — the verdict is a bool plus the structured
//!   warn, never a refusal.
//! - [`normalize_nonce`] + [`replay_diff`]: the `oxide replay --diff`
//!   comparison. The per-request CSP nonce (`oxide_fw_server::nonce_for`
//!   = 32 lowercase hex) differs between the recorded and the replayed
//!   response by construction, so it is blanked before the body compare;
//!   the header compare skips the same re-derived state the L0 cache
//!   already treats as per-request (`content-security-policy`, `date`,
//!   `set-cookie`). The [`ReplayDiff`] shape serializes camelCase for the
//!   `--json` surface.
//!
//! The wiring — the worker's `globalThis.__oxide_replay` install, the
//! `oxide replay` CLI command and the MCP `replay.run` tool — is the
//! CLI-adoption half, deferred with the rest of the A01 phase-3 wiring
//! (see `docs/deviations.md`); the one-definition orchestration they all
//! drive is the [`ReplayRunner`] + [`ReplayExecutor`] seam here, so the
//! command and the tool stay thin wrappers over the same tested path.

use std::collections::VecDeque;
use std::sync::Arc;

use base64::Engine as _;
use oxide_fw_diagnostics::{Diagnostic, OxideError};

/// The diagnostic code for an outbound call that is not the record's next
/// op (§V2-F15: "an unexpected call is an error `RPL-002` naming the
/// call").
pub const RPL_UNEXPECTED_CALL: &str = "RPL-002";

/// The diagnostic code for a record from an older build id: the replay
/// warns but runs.
pub const RPL_STALE_BUILD: &str = "RPL-003";

/// The diagnostic code for a non-deterministic stream the replay drained:
/// the re-execution observed more values than the record captured.
pub const RPL_SEQUENCE_DRAINED: &str = "RPL-004";

/// The nonce length the diff normalizes: `oxide_fw_server::nonce_for`
/// hashes the request id with blake3 and keeps 32 lowercase hex chars.
pub const NONCE_LENGTH: usize = 32;

fn rpl(code: &'static str, msg: impl Into<String>, fix: &'static str) -> OxideError {
    OxideError(Box::new(Diagnostic::error(code, msg).with_fix(fix)))
}

const RPL_002_FIX: &str = "Re-record, or the code path changed";
const RPL_004_FIX: &str = "Re-record; the replay observed more than the record captured";

/// The recorded outbound fetch ops, played in order.
///
/// The matcher is the seam the worker's fetch override consults: the
/// replayed call must be the record's NEXT op — method, URL and request
/// body (the "body hash" identity: the decoded recorded bytes compare
/// equal) — and the recorded response is handed back. Any divergence is
/// [`RPL_UNEXPECTED_CALL`], naming both sides of the mismatch; the failed
/// take consumes nothing.
#[derive(Debug, Clone, Default)]
pub struct ReplayMatcher {
    ops: VecDeque<crate::record::FetchOp>,
}

impl ReplayMatcher {
    /// The matcher over one record's fetch ops.
    #[must_use]
    pub fn from_record(record: &crate::record::RequestRecord) -> Self {
        Self {
            ops: record.fetches.iter().cloned().collect(),
        }
    }

    /// The ops not yet played.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.ops.len()
    }

    /// Play the next op: `method` + `url` + `request_body` must be the
    /// record's next call, and the recorded response comes back.
    ///
    /// # Errors
    ///
    /// [`OxideError`] with [`RPL_UNEXPECTED_CALL`] when the record is
    /// exhausted or the call is not the next recorded op.
    pub fn take(
        &mut self,
        method: &str,
        url: &str,
        request_body: &[u8],
    ) -> Result<crate::record::RecordedResponse, OxideError> {
        let Some(next) = self.ops.front() else {
            return Err(rpl(
                RPL_UNEXPECTED_CALL,
                format!(
                    "replay: outbound {method} {url} is not in the record (all recorded ops are played)"
                ),
                RPL_002_FIX,
            ));
        };
        let recorded_body = base64::engine::general_purpose::STANDARD
            .decode(&next.request_body_base64)
            .unwrap_or_default();
        if next.method != method || next.url != url || recorded_body.as_slice() != request_body {
            return Err(rpl(
                RPL_UNEXPECTED_CALL,
                format!(
                    "replay: outbound {method} {url} is not the recorded call (expected {} {})",
                    next.method, next.url
                ),
                RPL_002_FIX,
            ));
        }
        // `front` was `Some(next)` above; the response clones out of the
        // front op (the substrate never panics) and the pop is
        // unconditional.
        let response = next.response.clone();
        self.ops.pop_front();
        Ok(response)
    }
}

/// The recorded non-deterministic streams, in observation order (§V2-F15:
/// the worker overrides `Date.now`, `Math.random` and
/// `crypto.randomUUID` from the recorded sequences).
#[derive(Debug, Clone, Default)]
pub struct ReplaySequences {
    now_ms: VecDeque<u64>,
    randoms: VecDeque<f64>,
    uuids: VecDeque<String>,
}

impl ReplaySequences {
    /// The sequences over one record.
    #[must_use]
    pub fn from_record(record: &crate::record::RequestRecord) -> Self {
        Self {
            now_ms: record.sequences.now_ms.iter().copied().collect(),
            randoms: record.sequences.randoms.iter().copied().collect(),
            uuids: record.sequences.uuids.iter().cloned().collect(),
        }
    }

    /// The next `Date.now()` value.
    ///
    /// # Errors
    ///
    /// [`OxideError`] with [`RPL_SEQUENCE_DRAINED`] when the stream is
    /// exhausted.
    pub fn next_now(&mut self) -> Result<u64, OxideError> {
        self.now_ms.pop_front().ok_or_else(|| {
            rpl(
                RPL_SEQUENCE_DRAINED,
                "replay: the recorded Date.now() sequence is drained",
                RPL_004_FIX,
            )
        })
    }

    /// The next `Math.random()` value.
    ///
    /// # Errors
    ///
    /// [`OxideError`] with [`RPL_SEQUENCE_DRAINED`] when the stream is
    /// exhausted.
    pub fn next_random(&mut self) -> Result<f64, OxideError> {
        self.randoms.pop_front().ok_or_else(|| {
            rpl(
                RPL_SEQUENCE_DRAINED,
                "replay: the recorded Math.random() sequence is drained",
                RPL_004_FIX,
            )
        })
    }

    /// The next `crypto.randomUUID()` value.
    ///
    /// # Errors
    ///
    /// [`OxideError`] with [`RPL_SEQUENCE_DRAINED`] when the stream is
    /// exhausted.
    pub fn next_uuid(&mut self) -> Result<String, OxideError> {
        self.uuids.pop_front().ok_or_else(|| {
            rpl(
                RPL_SEQUENCE_DRAINED,
                "replay: the recorded crypto.randomUUID() sequence is drained",
                RPL_004_FIX,
            )
        })
    }
}

/// The `RPL-003` verdict (§V2-F15: "a record from an older build id warns
/// (`RPL-003`) but runs"): `true` when the record was made on a different
/// build than the process runs now — the structured warn carries the
/// record's build id, and the caller replays anyway. The recorded build
/// id never refuses.
pub fn warn_stale_build(record: &crate::record::RequestRecord, current: &str) -> bool {
    if record.build_id == current {
        return false;
    }
    tracing::warn!(
        code = RPL_STALE_BUILD,
        record_build = %record.build_id,
        current = %current,
        "the record was made on an older build id; the replay continues and the results may differ"
    );
    true
}

/// The `--diff` nonce normalization (§V2-F15: "compares to the recorded
/// response (normalizing the nonce)"): every `nonce="<{NONCE_LENGTH} hex>"`
/// attribute value becomes `nonce="<nonce>"` — the per-request CSP nonce
/// differs between the two responses by construction. Non-hex or
/// wrong-length attribute values are untouched.
#[must_use]
pub fn normalize_nonce(body: &str) -> String {
    let needle = "nonce=\"";
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(p) = rest.find(needle) {
        let value_start = p + needle.len();
        out.push_str(&rest[..value_start]);
        let after = &rest[value_start..];
        let hex_len = after
            .chars()
            .take(NONCE_LENGTH)
            .take_while(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            .count();
        let is_nonce_value = hex_len == NONCE_LENGTH && after[NONCE_LENGTH..].starts_with('"');
        if is_nonce_value {
            out.push_str("<nonce>\"");
            rest = &after[NONCE_LENGTH + 1..];
        } else {
            // Not a nonce value: copy the rest of the attribute value
            // verbatim up to (and including) its closing quote, or the
            // segment end when there is none.
            if let Some(q) = after.find('"') {
                out.push_str(&after[..=q]);
                rest = &after[q + 1..];
            } else {
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// One `--diff` divergence: the kind (`status` / `header` / `body`) and
/// the detail naming what differs.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Divergence {
    /// The divergence kind (`status`, `header`, `body`).
    pub kind: &'static str,
    /// The human detail: the two statuses, the header name and values, or
    /// the first differing byte offset.
    pub detail: String,
}

/// The `oxide replay --diff` verdict (§V2-F15: "`--json` emits
/// `{ ok, divergences: [..] }`"): the status, the header set and the
/// nonce-normalized body of the replayed response against the record's.
///
/// The header compare skips the per-request re-derived state the L0 cache
/// already treats as per-request (`content-security-policy` — the
/// pipeline re-derives it, `date` — the wall clock, `set-cookie` — the
/// replay may re-mint); everything else must match by name and value.
#[must_use]
pub fn replay_diff(
    record: &crate::record::RequestRecord,
    status: u16,
    headers: &[(String, String)],
    body: &str,
) -> ReplayDiff {
    let mut divergences: Vec<Divergence> = Vec::new();

    if record.response.status != status {
        divergences.push(Divergence {
            kind: "status",
            detail: format!(
                "the record answered {}, the replay answered {status}",
                record.response.status
            ),
        });
    }

    // The recorded headers (base64-decoded body side carries them) versus
    // the replay's, name-based, minus the re-derived trio.
    let recorded = recorded_response_headers(record);
    for (name, recorded_value) in &recorded {
        if is_rederived_header(name) {
            continue;
        }
        match headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some((_, value)) if value == recorded_value => {}
            Some((_, value)) => divergences.push(Divergence {
                kind: "header",
                detail: format!(
                    "{name}: the record sent {recorded_value:?}, the replay sent {value:?}"
                ),
            }),
            None => divergences.push(Divergence {
                kind: "header",
                detail: format!("{name}: the record sent it, the replay did not"),
            }),
        }
    }
    for (name, value) in headers {
        if is_rederived_header(name) || recorded.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
            continue;
        }
        divergences.push(Divergence {
            kind: "header",
            detail: format!("{name}: the replay sent {value:?}, the record did not"),
        });
    }

    let recorded_bytes = base64::engine::general_purpose::STANDARD
        .decode(&record.response.body_base64)
        .unwrap_or_default();
    let recorded_body = String::from_utf8_lossy(&recorded_bytes);
    let a = normalize_nonce(&recorded_body);
    let b = normalize_nonce(body);
    if a != b {
        let offset = a
            .bytes()
            .zip(b.bytes())
            .position(|(x, y)| x != y)
            .unwrap_or_else(|| a.len().min(b.len()));
        divergences.push(Divergence {
            kind: "body",
            detail: format!(
                "the bodies differ after nonce normalization (first differing byte at {offset})"
            ),
        });
    }

    ReplayDiff {
        ok: divergences.is_empty(),
        divergences,
    }
}

/// The recorded response's header list, decoded from the record.
fn recorded_response_headers(record: &crate::record::RequestRecord) -> Vec<(String, String)> {
    record.response.headers.clone()
}

/// The headers the diff treats as re-derived per-request state (the L0
/// cache's own convention: the CSP header is re-derived at serve time,
/// the wall clock and the cookie mint are not reproducible state).
fn is_rederived_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "content-security-policy" || n == "date" || n == "set-cookie"
}

/// The `--diff` verdict shape (§V2-F15: "`--json` emits
/// `{ ok, divergences: [..] }`").
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayDiff {
    /// `true` when the replay reproduced the record byte-for-byte (after
    /// the nonce normalization and the re-derived-header skips).
    pub ok: bool,
    /// Every divergence, in status → header → body order.
    pub divergences: Vec<Divergence>,
}

/// One re-executed response: what a [`ReplayExecutor`] hands back. The
/// body stays raw bytes in memory; on the wire (the `--json` surface, the
/// MCP tool) it rides base64 — the record's own convention.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutedResponse {
    /// The HTTP status the re-execution produced.
    pub status: u16,
    /// The response headers, in order.
    pub headers: Vec<(String, String)>,
    /// The response body, raw.
    pub body: Vec<u8>,
}

impl serde::Serialize for ExecutedResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use base64::Engine as _;
        use serde::ser::SerializeStruct as _;
        let mut state = serializer.serialize_struct("ExecutedResponse", 3)?;
        state.serialize_field("status", &self.status)?;
        state.serialize_field("headers", &self.headers)?;
        state.serialize_field(
            "bodyBase64",
            &base64::engine::general_purpose::STANDARD.encode(&self.body),
        )?;
        state.end()
    }
}

/// The re-execution seam (§V2-F15: "`oxide replay <id>` re-executes a
/// captured request deterministically"). The runner loads the record and
/// compares; THIS trait is the actual re-execution — the CLI's
/// implementation drives the captured request line-set, route id, params
/// and locals back through the real pipeline (V8 loaders, the §9.2 load
/// graph), with the worker's `globalThis.__oxide_replay` install playing
/// the recorded fetch ops and sequences into the same run. Test and
/// example hosts implement it over fakes; the runner stays
/// re-execution-agnostic.
#[async_trait::async_trait]
pub trait ReplayExecutor: Send + Sync {
    /// Re-execute one captured request.
    ///
    /// # Errors
    ///
    /// The implementation's own diagnostics surface unchanged — a
    /// playback that hits a call outside the record refuses `RPL-002`,
    /// a drained sequence `RPL-004`; the runner never wraps them.
    async fn execute(
        &self,
        record: &crate::record::RequestRecord,
    ) -> Result<ExecutedResponse, OxideError>;
}

/// The replay orchestration — the one definition the `oxide replay
/// <id> [--diff]` command and the MCP `replay.run` tool drive (the
/// CLI-side wiring wraps this; the substrate stays V8-free). The flow is
/// the spec's: load the record (`RPL-001` on a missing store entry,
/// `RPL-005` on a foreign schema version), warn-but-run on an older
/// build id (`RPL-003`), re-execute through the injected executor, and
/// produce the `--diff` verdict (nonce-normalized).
pub struct ReplayRunner {
    source: Arc<dyn crate::record::RecordSource>,
    current_build_id: String,
}

impl ReplayRunner {
    /// A runner over `source`, replaying against the build id the
    /// process runs now (the `RPL-003` comparison side).
    #[must_use]
    pub fn new(source: Arc<dyn crate::record::RecordSource>, current_build_id: &str) -> Self {
        Self {
            source,
            current_build_id: current_build_id.to_string(),
        }
    }

    /// Load, re-execute and diff one record.
    ///
    /// # Errors
    ///
    /// [`OxideError`] with `RPL-001` / `RPL-005` when the record cannot
    /// load, or the executor's own refusal (an `RPL-002` unexpected call,
    /// an `RPL-004` drained sequence, the pipeline's own codes) unchanged.
    pub async fn run(
        &self,
        request_id: &str,
        executor: &dyn ReplayExecutor,
    ) -> Result<ReplayOutcome, OxideError> {
        let record = self.source.load(request_id)?;
        // §V2-F15: "a record from an older build id warns (RPL-003) but
        // runs" — the structured warn fires here, the verdict rides the
        // outcome.
        let stale_build = warn_stale_build(&record, &self.current_build_id);
        let response = executor.execute(&record).await?;
        let body = String::from_utf8_lossy(&response.body);
        let diff = replay_diff(&record, response.status, &response.headers, &body);
        Ok(ReplayOutcome {
            record,
            response,
            stale_build,
            diff,
        })
    }
}

/// The runner's verdict: the loaded record, the re-executed response, the
/// `RPL-003` staleness and the `--diff` comparison. Serializes camelCase
/// for the `--json` and MCP surfaces (the executed body rides base64, the
/// record's wire convention).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayOutcome {
    /// The record the replay loaded (redacted, versioned).
    pub record: crate::record::RequestRecord,
    /// The re-executed response.
    pub response: ExecutedResponse,
    /// The record was made on an older build id (`RPL-003`; the replay
    /// ran anyway).
    pub stale_build: bool,
    /// The `--diff` verdict against the recorded response.
    pub diff: ReplayDiff,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_nonce_handles_a_split_or_unterminated_value() {
        // An unterminated nonce-shaped value is not an attribute value —
        // left untouched.
        assert_eq!(
            normalize_nonce("nonce=\"11111111111111111111111111111111"),
            "nonce=\"11111111111111111111111111111111"
        );
        // Uppercase hex is not the nonce shape (the nonce is lowercase).
        assert_eq!(
            normalize_nonce("nonce=\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\""),
            "nonce=\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\""
        );
    }
}
