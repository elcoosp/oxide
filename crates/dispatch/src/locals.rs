//! The unified request locals (spec v0.2, V2-A05 step 1).
//!
//! §V2-A05: one `Locals` value reaches loaders, layout loaders, actions,
//! `route.ts` and render — today the pipeline threads the middleware
//! map verbatim and special-cases `session` ad hoc. [`RequestLocals`] is
//! the typed composition the phase-3 `stage_prelude` builds (session from
//! the §16.6 codec, locale from the i18n resolver, experiments from the
//! assignment middleware) and hands to every consumer via
//! [`RequestLocals::to_value`].
//!
//! Reserved keys `session` / `locale` / `experiments` are pipeline-owned:
//! they win over user-set keys with the same name in [`to_value`], and a
//! middleware that stages one via [`RequestLocals::apply_middleware_locals`]
//! gets it reported back — the caller's dev-mode warning (`DSP-101`,
//! documented drift guard). The CLI wiring is deferred to the A01
//! phase-3 move (see `docs/deviations.md`).

use std::collections::BTreeMap;

/// The reserved locals keys, sorted (the DSP-101 report order).
pub const RESERVED_KEYS: [&str; 3] = ["experiments", "locale", "session"];

/// The diagnostic code for a middleware staging a reserved key.
pub const DSP_RESERVED_KEY_COLLISION: &str = "DSP-101";

/// The one request-scoped locals value: pipeline-owned reserved slots
/// plus the free-form map middleware and loaders write.
#[derive(Debug, Clone, Default)]
pub struct RequestLocals {
    /// The free-form map every stage can read and write (later wins).
    pub user: BTreeMap<String, serde_json::Value>,
    /// §16.6: the decoded session, when the codec sealed one.
    pub session: Option<serde_json::Value>,
    /// The i18n-resolved locale; empty when the app resolves none.
    pub locale: String,
    /// Experiment assignments for this request (§15.4).
    pub experiments: BTreeMap<String, serde_json::Value>,
}

impl RequestLocals {
    /// The single JSON value handed to loaders, actions, endpoints and
    /// render. Reserved keys win over user-set keys with the same name;
    /// an unset slot (`session` absent, `locale` empty, `experiments`
    /// empty) contributes no key at all.
    #[must_use]
    pub fn to_value(&self) -> serde_json::Value {
        let mut merged = self.user.clone();
        if let Some(session) = &self.session {
            merged.insert("session".into(), session.clone());
        }
        if !self.locale.is_empty() {
            merged.insert("locale".into(), self.locale.clone().into());
        }
        if !self.experiments.is_empty() {
            merged.insert(
                "experiments".into(),
                serde_json::Value::Object(self.experiments.clone().into_iter().collect()),
            );
        }
        serde_json::Value::Object(merged.into_iter().collect())
    }

    /// Merge one middleware's `locals` payload into the user map
    /// (later wins). Returns the reserved keys the payload tried to set —
    /// the DSP-101 warning sites — sorted, with the staged value dropped:
    /// the pipeline-owned slot decides at [`to_value`](Self::to_value)
    /// time. A non-object payload contributes nothing.
    pub fn apply_middleware_locals(&mut self, from: serde_json::Value) -> Vec<String> {
        let serde_json::Value::Object(source) = from else {
            return Vec::new();
        };
        let mut collisions = Vec::new();
        for (key, value) in source {
            if RESERVED_KEYS.contains(&key.as_str()) {
                collisions.push(key);
            } else {
                self.user.insert(key, value);
            }
        }
        collisions.sort();
        collisions
    }
}
