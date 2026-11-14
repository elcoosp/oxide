//! The offline-first server substrate (§V2-F14) — the `stage_idempotency`
//! data-plane stage the offline outbox's `x-oxide-idem` replay key
//! drives. The PWA worker (the CLI emitter's precache half) and the
//! client outbox (`packages/core/src/client/outbox.ts`) are the other
//! F14 halves; this stage is the server side the outbox's retries hit.
//!
//! The spec's stage contract, verbatim:
//!
//! > A stage `stage_idempotency` for POST actions and server functions:
//! > if `x-oxide-idem` is present, look up `idem:<key>` in L1; a hit
//! > returns the stored response (status, headers minus `set-cookie`,
//! > body); a miss runs the handler and stores the result for 24 h
//! > (only for 2xx and 4xx).
//!
//! The store is a seam ([`IdempotencyStore`]) — the production impl
//! rides the cache crate's L1 tier behind the same trait shape the
//! [`crate::record::RecordSink`]/[`crate::skew::AcceptedGenerations`]
//! seams use, and the tests drive fakes. A store failure degrades
//! loud-then-run (`DSP-116`, the house warn convention): the offline
//! convenience never takes online traffic down, the handler just runs.
//!
//! The 5xx rule is the server side of the outbox's keep-5xx rule: the
//! outbox keeps a 5xx item for the next flush because the SERVER never
//! pinned the failure — a replayed 4xx answers identically (the outbox
//! deletes it as permanently rejected), a replayed 2xx returns the
//! same bytes (the submit inserted once).

/// The executed-response shape the stage speaks (the replay module's
/// currency, re-exported so the stage's callers have one import).
pub use crate::replay::ExecutedResponse;
use oxide_fw_diagnostics::OxideError;
use serde::Deserialize;

/// The diagnostic code for an idempotency-store failure (the
/// loud-degrade warn; the stage itself never refuses on it).
pub const IDEM_STORE: &str = "DSP-116";

/// The TTL the spec pins (§V2-F14: "stores the result for 24 h").
pub const IDEM_TTL_SECS: u64 = 86_400;

/// The L1 key spelling the stage looks up (§V2-F14: "`idem:<key>` in
/// L1" — the raw header value under the stage's prefix).
#[must_use]
pub fn idem_key(raw: &str) -> String {
    format!("idem:{raw}")
}

/// The stored response shape (§V2-F14 verbatim): the status, the
/// headers minus `set-cookie`, and the body. The body rides base64 on
/// the wire — the record module's convention, so the same bytes cross
/// the L1 boundary a record does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResponse {
    /// The HTTP status the first execution produced.
    pub status: u16,
    /// The response headers, in order, `set-cookie` already stripped.
    pub headers: Vec<(String, String)>,
    /// The response body, raw (base64 on the wire).
    pub body: Vec<u8>,
}

impl StoredResponse {
    /// The store-time projection of a first execution: the status, the
    /// headers minus every `set-cookie` (case-insensitive, the HTTP
    /// convention), the body verbatim.
    #[must_use]
    pub fn from_executed(response: &ExecutedResponse) -> Self {
        Self {
            status: response.status,
            headers: response
                .headers
                .iter()
                .filter(|(k, _)| !k.eq_ignore_ascii_case("set-cookie"))
                .cloned()
                .collect(),
            body: response.body.clone(),
        }
    }

    /// The pipeline-facing shape (the CLI stage's bridge back to the
    /// response type the dispatch closure speaks).
    #[must_use]
    pub fn to_executed(&self) -> ExecutedResponse {
        ExecutedResponse {
            status: self.status,
            headers: self.headers.clone(),
            body: self.body.clone(),
        }
    }
}

impl serde::Serialize for StoredResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use base64::Engine as _;
        use serde::ser::SerializeStruct as _;
        let mut state = serializer.serialize_struct("StoredResponse", 3)?;
        state.serialize_field("status", &self.status)?;
        state.serialize_field("headers", &self.headers)?;
        state.serialize_field(
            "body",
            &base64::engine::general_purpose::STANDARD.encode(&self.body),
        )?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for StoredResponse {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use base64::Engine as _;
        #[derive(Deserialize)]
        struct Wire {
            status: u16,
            headers: Vec<(String, String)>,
            body: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        let body = base64::engine::general_purpose::STANDARD
            .decode(&wire.body)
            .map_err(serde::de::Error::custom)?;
        Ok(Self {
            status: wire.status,
            headers: wire.headers,
            body,
        })
    }
}

/// The idempotency-store seam: the L1 lookup and the 24 h put the
/// stage drives. Object-safe so a `&dyn` store drops in (the blanket
/// `&T` impl lets a boot-chosen source ride the same shape
/// [`crate::skew::AcceptedGenerations`] uses).
pub trait IdempotencyStore: Send + Sync {
    /// The stored response for `key`, or `None` on a miss. A store
    /// failure is the caller's degrade, not a request failure.
    ///
    /// # Errors
    ///
    /// The backend's own failure (unreachable, timeout) — the stage
    /// warns [`IDEM_STORE`] and runs the handler.
    fn get(&self, key: &str) -> Result<Option<StoredResponse>, OxideError>;

    /// Store the response under `key` for `ttl_secs`. A failure is the
    /// caller's degrade too — the first execution's response is
    /// unaffected.
    ///
    /// # Errors
    ///
    /// The backend's own failure.
    fn put(&self, key: &str, response: &StoredResponse, ttl_secs: u64) -> Result<(), OxideError>;
}

impl<T: IdempotencyStore + ?Sized> IdempotencyStore for &T {
    fn get(&self, key: &str) -> Result<Option<StoredResponse>, OxideError> {
        (**self).get(key)
    }

    fn put(&self, key: &str, response: &StoredResponse, ttl_secs: u64) -> Result<(), OxideError> {
        (**self).put(key, response, ttl_secs)
    }
}

/// What the stage decided: the handler ran (store the outcome for the
/// next replay) or the store answered (the handler never ran).
#[derive(Debug, Clone, PartialEq)]
pub enum IdempotencyVerdict {
    /// The store had `idem:<key>`: the stored response — the handler
    /// never ran, the bytes are the first execution's.
    Replayed(StoredResponse),
    /// A miss, a missing header or a degraded store: the handler ran
    /// and its response is the answer (stored when the status is 2xx
    /// or 4xx).
    Executed(ExecutedResponse),
}

/// The §V2-F14 stage: with `x-oxide-idem` present (`idem` = `Some`), a
/// store hit replays the stored response and the handler never runs; a
/// miss runs the handler and stores 2xx/4xx outcomes for 24 h. Without
/// the header the stage is a pass-through. A store failure warns
/// [`IDEM_STORE`] and runs the handler — never a request failure.
///
/// # Errors
///
/// Only the handler's own error surfaces (unchanged, nothing stored) —
/// the store's failures degrade.
pub async fn stage_idempotency<S, F, Fut>(
    store: &S,
    idem: Option<&str>,
    run: F,
) -> Result<IdempotencyVerdict, OxideError>
where
    S: IdempotencyStore + ?Sized,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<ExecutedResponse, OxideError>>,
{
    let Some(raw) = idem else {
        return run().await.map(IdempotencyVerdict::Executed);
    };
    let key = idem_key(raw);
    match store.get(&key) {
        Ok(Some(stored)) => return Ok(IdempotencyVerdict::Replayed(stored)),
        Ok(None) => {}
        Err(e) => {
            // The degrade: loud, then the handler runs (the house
            // convention — the offline convenience never takes online
            // traffic down).
            tracing::warn!(
                code = IDEM_STORE,
                key = %key,
                error = %e.0.message,
                "the idempotency store refused the lookup; running the handler"
            );
        }
    }
    let response = run().await?;
    // Only 2xx and 4xx are stored (the spec's rule; a 5xx stays
    // unpinned so the next flush retries).
    if (200..500).contains(&response.status) {
        let stored = StoredResponse::from_executed(&response);
        if let Err(e) = store.put(&key, &stored, IDEM_TTL_SECS) {
            tracing::warn!(
                code = IDEM_STORE,
                key = %key,
                error = %e.0.message,
                "the idempotency store refused the put; the response is unaffected"
            );
        }
    }
    Ok(IdempotencyVerdict::Executed(response))
}
