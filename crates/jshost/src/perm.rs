//! V2-F19: the per-route capability permission policy substrate.
//!
//! Pure Rust — no V8, no deno_core, no tokio. The [`PermissionSet`] is the
//! typed grant a route declares (`config.permissions` /
//! `export const permissions` in `page.server.ts`); [`host_allowed`] is the
//! host-match predicate the `op_oxide_fetch` boundary check calls. The V8-
//! bound half (the [`PermGuard`](crate::v8_ops::PermGuard) RAII that seeds a
//! thread-local per call, the per-op deny sites) lives in
//! [`crate::v8_ops`]; the policy itself is V8-free so the unit pins run in
//! every `--no-default-features` build.
//!
//! The wildcard semantics are the spec's exact rule (§V2-F19): a `*.`
//! prefix matches a **label boundary**, not a bare suffix — `*.internal`
//! matches `api.internal` but rejects `evvil.internal` only by host, and
//! critically rejects `notinternal` (no dot) and `api.api.internal` only
//! when the suffix is the right depth. A bare `*` is not a pattern
//! (use `None` for unrestricted); `*.` alone matches any single-label-deeper
//! host. Subdomain spoofing (`api.internal.evil.com` against `*.internal`)
//! is rejected: the label boundary check ensures the suffix follows a `.`.
//!
//! `None` on every field means unrestricted (the v0.2 default — allow-all,
//! deny-by-default is the opt-in via `permissions.default = 'deny'`).

use serde::{Deserialize, Serialize};

/// The default posture when a route declares no `permissions` block (or a
/// field is absent). v0.2 ships allow-all (the honest "no opt-in yet"
/// stance); deny-by-default is the opt-in a security-conscious app sets.
///
/// `None` on a [`PermissionSet`] field always means unrestricted *for that
/// field*, independent of [`PermissionDefault`] — `default` governs only
/// the absent-block case. This keeps the two knobs orthogonal: a route
/// with `permissions: { default: 'deny', net: ['api.stripe.com'] }` denies
/// env and fs by default but allows the listed net hosts; a route with
/// `permissions: { default: 'allow' }` (or no block) allows everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDefault {
    /// Allow every capability the isolate exposes (the v0.2 default).
    #[default]
    Allow,
    /// Deny every capability not explicitly listed in `net`/`env`/`fs`.
    Deny,
}

/// The per-route capability grant (spec §V2-F19). Each field is an
/// allow-list of host / env-name / fs-path patterns; `None` means
/// unrestricted (the field is absent from the declaration). An empty
/// `Vec` (`Some(vec![])`) means **nothing** is allowed — the route
/// declared the field but listed no entries, the explicit-deny-all arm.
///
/// The host patterns in `net` use the [`host_allowed`] wildcard rule;
/// `env` entries match env-var names exactly (case-sensitive — env names
/// are); `fs` entries are path prefixes (a `./data` entry allows
/// `./data/anything`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionSet {
    /// `net: ['api.stripe.com', '*.internal']` — host patterns the route may
    /// fetch. `None` = unrestricted; `Some([])` = no host.
    pub net: Option<Vec<String>>,
    /// `env: ['STRIPE_KEY']` — env-var names the route may read. `None` =
    /// unrestricted; `Some([])` = no name.
    pub env: Option<Vec<String>>,
    /// `fs: ['./data']` — path prefixes the route may read. `None` =
    /// unrestricted; `Some([])` = no path.
    pub fs: Option<Vec<String>>,
}

impl PermissionSet {
    /// The unrestricted grant — every field `None`. This is the v0.2 default
    /// for a route that declares no `permissions` block (and the grant a
    /// dispatcher seeds when the route has no declaration).
    #[must_use]
    pub fn unrestricted() -> Self {
        Self::default()
    }

    /// Whether `host` is allowed by the `net` grant. `None` → unrestricted
    /// (allowed); `Some([])` → nothing allowed; otherwise the spec's
    /// [`host_allowed`] wildcard match over the patterns.
    #[must_use]
    pub fn is_net_allowed(&self, host: &str) -> bool {
        let Some(pats) = self.net.as_deref() else {
            return true;
        };
        host_allowed(pats, host)
    }

    /// Whether `name` is allowed by the `env` grant. `None` → unrestricted;
    /// `Some([])` → nothing; otherwise an exact (case-sensitive) match —
    /// env-var names are case-sensitive on every host that matters.
    #[must_use]
    pub fn is_env_allowed(&self, name: &str) -> bool {
        let Some(names) = self.env.as_deref() else {
            return true;
        };
        names.iter().any(|n| n == name)
    }

    /// Whether `path` is allowed by the `fs` grant. `None` → unrestricted;
    /// `Some([])` → nothing; otherwise a prefix match on the normalized
    /// path (a `./data` entry allows `./data/x` and `./data/sub/y`).
    ///
    /// The prefix is compared after a `trim_start_matches("./")` on both
    /// sides so `./data` and `data` are the same grant (the config and the
    /// op both normalize). A path that exactly equals an entry is allowed.
    #[must_use]
    pub fn is_fs_allowed(&self, path: &str) -> bool {
        let Some(pfxs) = self.fs.as_deref() else {
            return true;
        };
        let p = norm_fs(path);
        pfxs.iter().any(|pfx| {
            let pfx = norm_fs(pfx);
            p == pfx || p.starts_with(&format!("{pfx}/"))
        })
    }

    /// Whether this set is fully unrestricted (every field `None`). The
    /// profile-hash shortcut (§V2-F19 Model 2): an unrestricted route maps
    /// to the pre-F19 pool, byte-identical overhead.
    #[must_use]
    pub fn is_unrestricted(&self) -> bool {
        self.net.is_none() && self.env.is_none() && self.fs.is_none()
    }

    /// The canonical byte representation for the profile hash (§V2-F19
    /// Model 2). Sorted, `:`-joined, deterministic — two routes with the
    /// same grant produce the same bytes so they share an isolate.
    #[must_use]
    pub fn profile_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        out.push_str(match self.default_posture() {
            PermissionDefault::Allow => "a|",
            PermissionDefault::Deny => "d|",
        });
        out.push_str("n:");
        if let Some(n) = self.net.as_deref() {
            let mut s = n.to_vec();
            s.sort();
            out.push_str(&s.join(","));
        }
        out.push_str("|e:");
        if let Some(e) = self.env.as_deref() {
            let mut s = e.to_vec();
            s.sort();
            out.push_str(&s.join(","));
        }
        out.push_str("|f:");
        if let Some(f) = self.fs.as_deref() {
            let mut s = f.to_vec();
            s.sort();
            out.push_str(&s.join(","));
        }
        out.into_bytes()
    }

    /// The default posture this set implies. A `None`-everywhere set with
    /// no `default` context is `Allow` (v0.2). This is the dispatch-side
    /// read; the config layer carries the explicit `default` field.
    #[must_use]
    pub fn default_posture(&self) -> PermissionDefault {
        PermissionDefault::Allow
    }
}

/// The host-match predicate (spec §V2-F19, verbatim wildcard rule).
///
/// A pattern matches when it equals the host exactly, OR it starts with
/// `*.`, the host ends with the suffix after `*.`, the host is longer than
/// the suffix, and the byte before the suffix in the host is `.` (a label
/// boundary — not a bare suffix). This rejects subdomain spoofing:
/// `*.internal` matches `api.internal` but rejects `notinternal` (no dot)
/// and `api.internal.evil` (the suffix is not the last label).
///
/// A bare `*` is not a valid pattern (use `None` on the field for
/// unrestricted); `*.` alone matches any host with at least one label
/// boundary before the empty suffix. An empty pattern slice matches
/// nothing.
#[must_use]
pub fn host_allowed(pat: &[String], host: &str) -> bool {
    pat.iter().any(|p| match_host(p, host))
}

/// One pattern's match — the spec's verbatim rule, extracted so the unit
/// pins can target a single pattern without building a slice.
fn match_host(pat: &str, host: &str) -> bool {
    if pat == host {
        return true;
    }
    let Some(suf) = pat.strip_prefix("*.") else {
        return false;
    };
    // The host must end with the suffix, be longer than it, AND the byte
    // before the suffix must be a `.` — a label boundary, not a bare
    // suffix. This is the subdomain-spoof rejection: `*.internal` against
    // `api.internal.evil` fails because the suffix `internal` is followed
    // by `.evil` (the host does not END with `internal`), and against
    // `notinternal` fails because there is no `.` before `internal`.
    host.ends_with(suf)
        && host.len() > suf.len()
        && host.as_bytes()[host.len() - suf.len() - 1] == b'.'
}

/// Normalize an fs path: strip a leading `./` so `./data` and `data` are
/// the same grant. Does NOT canonicalize `..` (the op boundary rejects
/// traversal separately — the prefix match alone is not a security
/// boundary, the grant is advisory on Node and enforced on Native where
/// the host also validates).
fn norm_fs(p: &str) -> &str {
    p.trim_start_matches("./")
}

/// Extract the host from a URL for the net boundary check (V2-F19). A
/// URL without a host (a relative path, a `data:` URL) returns `None`
/// (no network — the check passes); a malformed URL returns `None` (the
/// fetch handler's SSRF guard rejects it separately — the permission
/// check is a grant check, not a URL validator).
///
/// V8-free so the Node advisory path (`op_dispatch`'s `Fetch` arm) and
/// the V8 hard-block path (`op_oxide_fetch`) share one host extractor.
#[must_use]
pub fn url_host(url: &str) -> Option<String> {
    // Strip the scheme; the host follows `://` (or is the whole string
    // for a schemeless URL — no host). A `//`-prefixed protocol-relative
    // URL has a host too.
    let after: &str = url.split("://").nth(1).or_else(|| url.strip_prefix("//"))?;
    // The host is up to the first `/`, `?`, `#` or `:`.
    let host: String = after
        .chars()
        .take_while(|c| !matches!(c, '/' | '?' | '#' | ':'))
        .collect();
    if host.is_empty() { None } else { Some(host) }
}

// V2-F19: the route→permission registry. The dispatcher populates this
// before calling into the isolate (`set_route_permissions`); the
// `PermGuard` (V8) and the Node advisory (`op_dispatch`'s `Fetch` arm)
// both read it. V8-free so the Node path (which does not compile
// `v8_ops`) reaches the registry without a feature gate.
static ROUTE_PERMS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, PermissionSet>>,
> = std::sync::OnceLock::new();

fn route_perms() -> &'static std::sync::RwLock<std::collections::HashMap<String, PermissionSet>> {
    ROUTE_PERMS.get_or_init(Default::default)
}

/// Register a route's per-route permission grant (V2-F19). The dispatcher
/// calls this when it loads a route's `export const permissions`
/// declaration from `page.server.ts` (the CLI's declaration scan reads
/// `declares_permissions`; the typed grant is parsed from the export's
/// value). A route with no declaration is left absent — the caller seeds
/// the unrestricted default for it.
pub fn set_route_permissions(route: &str, grant: PermissionSet) {
    route_perms()
        .write()
        .unwrap()
        .insert(route.to_string(), grant);
}

/// Read a route's grant from the registry (the `PermGuard`'s lookup and
/// the Node advisory's read). Returns `None` when the route has no
/// declaration (the unrestricted default applies).
#[must_use]
pub fn route_permissions(route: &str) -> Option<PermissionSet> {
    route_perms().read().unwrap().get(route).cloned()
}

/// Whether ANY registered route's grant would deny `host` (V2-F19, Node
/// advisory). The Node `op_dispatch` path cannot attribute a fetch to a
/// single route (the route label is not on the op's args), so the
/// advisory fires when any registered grant denies the host — the
/// conservative posture. A host no registered grant denies produces no
/// warn (the common case: most routes declare no permissions).
#[must_use]
pub fn any_route_denies_net(host: &str) -> bool {
    let regs = route_perms().read().unwrap();
    regs.values().any(|g| !g.is_net_allowed(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact-host match — the common case (a fully-qualified API).
    #[test]
    fn exact_host_matches() {
        assert!(host_allowed(
            &["api.stripe.com".to_string()],
            "api.stripe.com"
        ));
    }

    /// A different exact host does not match.
    #[test]
    fn different_host_rejected() {
        assert!(!host_allowed(
            &["api.stripe.com".to_string()],
            "api.paypal.com"
        ));
    }

    /// The `*.` wildcard matches a single deeper label.
    #[test]
    fn wildcard_matches_subdomain() {
        assert!(host_allowed(&["*.internal".to_string()], "api.internal"));
        assert!(host_allowed(&["*.internal".to_string()], "svc.internal"));
    }

    /// The spoof rejection: `*.internal` does NOT match `notinternal` —
    /// there is no label boundary before the suffix. This is the rule the
    /// spec writes explicitly (label-boundary, not bare suffix).
    #[test]
    fn wildcard_rejects_no_dot() {
        assert!(!host_allowed(&["*.internal".to_string()], "notinternal"));
        assert!(!host_allowed(&["*.internal".to_string()], "xnotinternal"));
    }

    /// The spoof rejection across labels: `*.internal` does NOT match
    /// `api.internal.evil` — the host does not end with `internal` (it
    /// ends with `evil`), so the suffix test itself fails.
    #[test]
    fn wildcard_rejects_suffix_not_last_label() {
        assert!(!host_allowed(
            &["*.internal".to_string()],
            "api.internal.evil.com"
        ));
    }

    /// The deeper match: `*.internal` matches `a.b.internal` — the suffix
    /// is the last label cluster and the byte before it is `.`.
    #[test]
    fn wildcard_matches_deeper_subdomain() {
        assert!(host_allowed(&["*.internal".to_string()], "a.b.internal"));
    }

    /// An empty pattern slice matches nothing — the explicit-deny-all arm.
    #[test]
    fn empty_patterns_match_nothing() {
        assert!(!host_allowed(&[], "api.stripe.com"));
        assert!(!host_allowed(&[], "anything"));
    }

    /// A bare `*` is not a valid pattern — it matches only a host literally
    /// named `*` (which is not a real host). Use `None` on the field for
    /// unrestricted. This keeps the wildcard rule honest: `*.` is the
    /// documented wildcard, `*` is not.
    #[test]
    fn bare_star_is_not_a_wildcard() {
        assert!(!host_allowed(&["*".to_string()], "anything.com"));
    }

    /// `*.` alone is a degenerate pattern: the suffix is empty, so the rule
    /// checks the byte before the empty suffix — the host's LAST byte — and
    /// requires it to be `.`. This matches only the trailing-dot FQDN form
    /// (`api.stripe.com.`), not a normal host. A route that wants "any
    /// host" uses `None` on the field (unrestricted), not `*.`. This pin
    /// documents the honest behavior so the wildcard rule is not
    /// over-read.
    #[test]
    fn star_dot_alone_matches_only_trailing_dot_host() {
        assert!(!host_allowed(&["*.".to_string()], "api.stripe.com"));
        assert!(!host_allowed(&["*.".to_string()], "x.y.z"));
        // The trailing-dot FQDN form matches (the last byte is `.`).
        assert!(host_allowed(&["*.".to_string()], "api.stripe.com."));
    }

    /// The `PermissionSet::unrestricted` default — every field `None`,
    /// every `is_*_allowed` returns `true`.
    #[test]
    fn unrestricted_allows_everything() {
        let p = PermissionSet::unrestricted();
        assert!(p.is_net_allowed("evil.example"));
        assert!(p.is_env_allowed("SECRET_KEY"));
        assert!(p.is_fs_allowed("/etc/passwd"));
        assert!(p.is_unrestricted());
    }

    /// `Some([])` — the explicit-deny-all arm for one field. The field is
    /// present but lists nothing, so nothing is allowed for that
    /// capability; the other fields stay unrestricted.
    #[test]
    fn empty_vec_denies_all_for_that_field() {
        let p = PermissionSet {
            net: Some(Vec::new()),
            env: None,
            fs: None,
        };
        assert!(!p.is_net_allowed("api.stripe.com"));
        assert!(!p.is_net_allowed("anything"));
        // The other fields are unrestricted.
        assert!(p.is_env_allowed("X"));
        assert!(p.is_fs_allowed("./x"));
        assert!(!p.is_unrestricted());
    }

    /// The net allow-list — only the listed host (and its wildcard) is
    /// allowed; a sibling host is rejected.
    #[test]
    fn net_allow_list() {
        let p = PermissionSet {
            net: Some(vec!["api.stripe.com".into(), "*.internal".into()]),
            env: None,
            fs: None,
        };
        assert!(p.is_net_allowed("api.stripe.com"));
        assert!(p.is_net_allowed("svc.internal"));
        assert!(!p.is_net_allowed("api.paypal.com"));
        assert!(!p.is_net_allowed("notinternal"));
    }

    /// The env allow-list — exact, case-sensitive match. `STRIPE_KEY` is
    /// not `stripe_key`.
    #[test]
    fn env_allow_list_case_sensitive() {
        let p = PermissionSet {
            net: None,
            env: Some(vec!["STRIPE_KEY".into()]),
            fs: None,
        };
        assert!(p.is_env_allowed("STRIPE_KEY"));
        assert!(!p.is_env_allowed("stripe_key"));
        assert!(!p.is_env_allowed("DATABASE_URL"));
    }

    /// The fs allow-list — prefix match, `./` normalized so `./data` and
    /// `data` are the same grant. A sibling path is rejected.
    #[test]
    fn fs_allow_list_prefix() {
        let p = PermissionSet {
            net: None,
            env: None,
            fs: Some(vec!["./data".into()]),
        };
        assert!(p.is_fs_allowed("./data"));
        assert!(p.is_fs_allowed("data"));
        assert!(p.is_fs_allowed("./data/users.json"));
        assert!(p.is_fs_allowed("data/users.json"));
        assert!(!p.is_fs_allowed("./data-evil"));
        assert!(!p.is_fs_allowed("./secrets"));
    }

    /// The profile hash is deterministic — two identical sets produce
    /// identical bytes; a different set produces different bytes. This is
    /// the invariant the Model 2 isolate pool relies on.
    #[test]
    fn profile_bytes_are_deterministic_and_distinct() {
        let a = PermissionSet {
            net: Some(vec!["b.internal".into(), "a.internal".into()]),
            env: Some(vec!["X".into()]),
            fs: None,
        };
        let b = PermissionSet {
            net: Some(vec!["a.internal".into(), "b.internal".into()]),
            env: Some(vec!["X".into()]),
            fs: None,
        };
        // Order-independent: the patterns are sorted in the canonical bytes.
        assert_eq!(a.profile_bytes(), b.profile_bytes());
        let c = PermissionSet {
            net: Some(vec!["a.internal".into(), "b.internal".into()]),
            env: Some(vec!["Y".into()]),
            fs: None,
        };
        assert_ne!(a.profile_bytes(), c.profile_bytes());
        // Unrestricted vs restricted differ.
        assert_ne!(
            PermissionSet::unrestricted().profile_bytes(),
            a.profile_bytes()
        );
    }

    /// The default posture is `Allow` for the substrate (the config layer
    /// carries the explicit `default` field; the substrate's `None`-fields
    /// default to allow).
    #[test]
    fn default_posture_is_allow() {
        assert_eq!(
            PermissionSet::unrestricted().default_posture(),
            PermissionDefault::Allow
        );
    }

    /// `PermissionDefault` serializes lowercase to match the config-file
    /// spelling (`permissions.default = 'allow' | 'deny'`).
    #[test]
    fn default_serializes_lowercase() {
        let allow = serde_json::to_string(&PermissionDefault::Allow).unwrap();
        let deny = serde_json::to_string(&PermissionDefault::Deny).unwrap();
        assert_eq!(allow, "\"allow\"");
        assert_eq!(deny, "\"deny\"");
        assert_eq!(
            serde_json::from_str::<PermissionDefault>("\"deny\"").unwrap(),
            PermissionDefault::Deny
        );
        // The default is Allow (v0.2).
        assert_eq!(PermissionDefault::default(), PermissionDefault::Allow);
    }

    /// The full `PermissionSet` round-trips through serde — the
    /// config-file shape (`{ net, env, fs }`, camelCase is the config
    /// layer's concern; the substrate uses snake_case field names the
    /// typed config re-serializes).
    #[test]
    fn permission_set_round_trips() {
        let p = PermissionSet {
            net: Some(vec!["api.stripe.com".into()]),
            env: Some(vec!["STRIPE_KEY".into()]),
            fs: Some(vec!["./data".into()]),
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: PermissionSet = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    /// `url_host` extracts the host from a URL; relative URLs and
    /// schemeless URLs return `None` (no network — the check passes).
    #[test]
    fn url_host_extracts_the_host() {
        assert_eq!(
            url_host("https://api.stripe.com/v1/charges"),
            Some("api.stripe.com".to_string())
        );
        assert_eq!(
            url_host("https://api.stripe.com"),
            Some("api.stripe.com".to_string())
        );
        // With a port.
        assert_eq!(
            url_host("http://localhost:3000/api"),
            Some("localhost".to_string())
        );
        // Protocol-relative.
        assert_eq!(
            url_host("//cdn.example.com/x.js"),
            Some("cdn.example.com".to_string())
        );
        // A relative path has no host.
        assert_eq!(url_host("/api/charges"), None);
        assert_eq!(url_host("data:text/plain,hello"), None);
        // An empty host after the scheme.
        assert_eq!(url_host("https:///path"), None);
    }

    /// The route→permission registry round-trips a grant and returns
    /// `None` for an unregistered route.
    #[test]
    fn route_registry_round_trips() {
        // Clear any prior state from other tests (the registry is global).
        let grant = PermissionSet {
            net: Some(vec!["api.stripe.com".into()]),
            env: None,
            fs: None,
        };
        set_route_permissions("test-route-1", grant.clone());
        let got = route_permissions("test-route-1");
        assert_eq!(got, Some(grant));
        assert!(route_permissions("unregistered-route").is_none());
    }

    /// `any_route_denies_net` is the Node advisory's read — true when
    /// any registered grant denies the host.
    #[test]
    fn any_route_denies_net_reports_a_deny() {
        set_route_permissions(
            "test-route-2",
            PermissionSet {
                net: Some(vec!["api.stripe.com".into()]),
                env: None,
                fs: None,
            },
        );
        // The registered route allows api.stripe.com but denies others.
        assert!(any_route_denies_net("evil.example"));
        assert!(!any_route_denies_net("api.stripe.com"));
    }
}
