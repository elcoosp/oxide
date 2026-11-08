//! The §15.4 experiments assignment engine (spec v0.2, V2-F04 step 1).
//!
//! §V2-F04: A/B experiments evaluated natively become L0 variant-key
//! dimensions, so an experiment stops forcing a route dynamic. The
//! `stage_experiments` wiring (CLI adoption deferred, see
//! `docs/deviations.md`) resolves [`Experiment`]s for the request path and
//! feeds [`Resolved::assigned`] into the pipeline-owned
//! `experiments` locals slot ([`RequestLocals::set_experiments`],
//! V2-A05); [`Resolved::vary_suffix`] is the `|exp:` dimension the L0 key
//! appends **only when non-empty**, so routes outside every experiment
//! keep byte-identical keys to the pre-F04 baseline.
//!
//! The assignment is deterministic: blake3 over `id || 0x00 ||
//! sticky_value`, the first eight bytes little-endian as the bucket
//! number — `n % 100` is the traffic gate, `(n / 100) % variants` picks
//! the variant. Re-evaluating the same identity never flips its variant.
//!
//! Deliberate delta from the spec pseudocode (recorded in
//! `docs/deviations.md`): [`Resolved::set_cookie`] is a `Vec<String>` —
//! the spec's single `Option<String>` slot silently drops every minted
//! cookie except the last when two cookie-sticky experiments both need a
//! fresh identity, and two `Set-Cookie` values can never legally share
//! one header.

use std::collections::BTreeMap;

/// The `Set-Cookie` attributes every minted experiment identity carries:
/// one month, first-party path, script-invisible (§V2-F04 item 3).
const MINT_ATTRS: &str = "; Path=/; Max-Age=2592000; SameSite=Lax; HttpOnly";

/// How one experiment pins an identity across requests (§V2-F04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sticky {
    /// Read (then mint) the named cookie. The cookie name is the storage
    /// key, not the experiment id, so two experiments can share a jar.
    Cookie(String),
    /// Read the named request header (edge/CDN cohorts); absent → no
    /// assignment, never minted.
    Header(String),
    /// Hash the client IP directly — no storage, coarse-grained.
    IpHash,
}

/// One configured experiment (§V2-F04 item 5, the `experiments` config
/// entries converted by the stage wiring).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Experiment {
    /// The experiment id — also the locals key and the vary-suffix name.
    pub id: String,
    /// Percentage of eligible identities in the experiment, 0–100.
    pub traffic: u8,
    /// The identity strategy.
    pub sticky: Sticky,
    /// Variant names, in config order (the assignment indexes them).
    pub variants: Vec<String>,
    /// URL-pattern prefixes the experiment applies to; empty = all routes.
    pub routes: Vec<String>,
}

impl Experiment {
    /// Whether the experiment is eligible on `path`: the route list is
    /// URL-pattern prefixes (§V2-F04), and an empty list means all routes.
    fn eligible_on(&self, path: &str) -> bool {
        self.routes.is_empty() || self.routes.iter().any(|r| path.starts_with(r.as_str()))
    }
}

/// The per-request outcome of the §V2-F04 evaluation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    /// `experiment id → variant name` for every eligible experiment the
    /// identity landed in (id-sorted, so it is byte-stable). Feeds
    /// [`RequestLocals::set_experiments`](crate::locals::RequestLocals::set_experiments).
    pub assigned: BTreeMap<String, String>,
    /// One `Set-Cookie` header value per **minted** identity, in
    /// evaluation order — the wiring appends them to the response *after*
    /// the L0 lookup and never stores them in the cached entry.
    pub set_cookie: Vec<String>,
    /// `id=variant` pairs joined by `;` (id-sorted). Empty when nothing
    /// was assigned — the L0 key only grows an `|exp:` dimension when this
    /// is non-empty (the "keys equal the pre-F04 baseline" acceptance).
    pub vary_suffix: String,
}

/// Deterministic assignment for one experiment (§V2-F04). `None` means the
/// identity is not in the experiment: empty variants, an empty sticky
/// value, or the traffic gate.
#[must_use]
pub fn assign(exp: &Experiment, sticky_value: &str) -> Option<usize> {
    if exp.variants.is_empty() || sticky_value.is_empty() {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(exp.id.as_bytes());
    hasher.update(&[0]);
    hasher.update(sticky_value.as_bytes());
    let digest = hasher.finalize();
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&digest.as_bytes()[..8]);
    let bucket = u64::from_le_bytes(prefix);
    let traffic = u64::from(exp.traffic);
    if bucket % 100 >= traffic {
        return None;
    }
    let Ok(len) = u64::try_from(exp.variants.len()) else {
        return None;
    };
    let Ok(idx) = usize::try_from((bucket / 100) % len) else {
        return None;
    };
    Some(idx)
}

/// Evaluate every experiment for one request (§V2-F04 `resolve`). Experiments
/// run in config order; the assigned map is id-sorted so the suffix is
/// byte-stable regardless of config order.
pub fn resolve(
    experiments: &[Experiment],
    path: &str,
    cookie_header: Option<&str>,
    header: &dyn Fn(&str) -> Option<String>,
    ip: &str,
    mint: &dyn Fn() -> String,
) -> Resolved {
    let mut resolved = Resolved::default();
    for exp in experiments.iter().filter(|e| e.eligible_on(path)) {
        let sticky_value = match &exp.sticky {
            Sticky::Cookie(name) => {
                if let Some(v) = cookie_value(cookie_header, name) {
                    v
                } else {
                    let v = mint();
                    resolved.set_cookie.push(format!("{name}={v}{MINT_ATTRS}"));
                    v
                }
            }
            Sticky::Header(name) => header(name).unwrap_or_default(),
            Sticky::IpHash => ip.to_string(),
        };
        if let Some(idx) = assign(exp, &sticky_value) {
            resolved
                .assigned
                .insert(exp.id.clone(), exp.variants[idx].clone());
        }
    }
    resolved.vary_suffix = resolved
        .assigned
        .iter()
        .map(|(id, variant)| format!("{id}={variant}"))
        .collect::<Vec<_>>()
        .join(";");
    resolved
}

/// The named cookie out of a `Cookie` header value — first `name=value`
/// pair whose name matches exactly, tolerating the usual `; ` separators
/// (§V2-F04 `cookie_value`).
fn cookie_value(header: Option<&str>, name: &str) -> Option<String> {
    header?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| value.to_string())
}
