//! Parallel and intercepting routes (spec §8.5).
//!
//! Slot identity is reserved from day one in the tree format; this module
//! implements the slot naming and the intercept matching rules.
//!
//! - `@slot/` → parallel route. A route under `@slot/foo/page.tsx` renders
//!   into `<slot name="slot">` concurrently with its siblings. The default
//!   slot is `children`.
//! - `(.)intercept/` → intercepting route. `(.)` matches a sibling path,
//!   `(..)` one level up, `(...)` from the root. The intercept renders the
//!   `@slot/` variant when navigation originates from the intercepting path,
//!   and the primary route otherwise.

use std::collections::BTreeMap;

/// One parallel slot on a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotSpec {
    /// Slot name; `"children"` is the default slot.
    pub name: String,
    /// Route paths under this slot, in match-priority order.
    pub paths: Vec<String>,
}

/// Intercept level: how far up the tree the intercepting route applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptLevel {
    /// `(.)` — same level.
    Same,
    /// `(..)` — one level up.
    UpOne,
    /// `(..)(..)` etc — N levels up.
    Up(u32),
    /// `(...)` — from the root.
    FromRoot,
}

/// An intercepting route declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterceptSpec {
    /// The slot the intercept targets (e.g. `"modal"`).
    pub slot: String,
    /// The intercepted target path (the primary route it shadows).
    pub target: String,
    /// The intercept's own source path (where the intercept file lives).
    pub source: String,
    pub level: InterceptLevel,
}

/// Result of a slot match: which slot owns a page at `route`, and whether the
/// request is intercepted.
#[derive(Debug, Clone, Default)]
pub struct SlotMatch {
    /// `slot_name` → page path to render.
    pub slots: BTreeMap<String, String>,
    /// Set when an intercept applies: the intercepting slot wins for the given
    /// navigation source.
    pub intercepted_from: Option<String>,
}

/// Parse a slot directory name: `@modal` → `"modal"`, `@` alone → error.
pub fn parse_slot_name(dir: &str) -> Option<String> {
    dir.strip_prefix('@')
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Parse an intercept marker at the start of a route segment.
pub fn parse_intercept_marker(segment: &str) -> Option<(InterceptLevel, &str)> {
    if let Some(rest) = segment.strip_prefix("(...)") {
        return Some((InterceptLevel::FromRoot, rest));
    }
    if let Some(rest) = segment.strip_prefix("(.)") {
        return Some((InterceptLevel::Same, rest));
    }
    if segment.starts_with("(..)") {
        let mut n = 0u32;
        let mut rest = segment;
        while let Some(r) = rest.strip_prefix("(..)") {
            n += 1;
            rest = r;
        }
        return Some((
            if n == 1 {
                InterceptLevel::UpOne
            } else {
                InterceptLevel::Up(n)
            },
            rest,
        ));
    }
    None
}

/// Compute which slots render for a given route and navigation source.
///
/// `route` is the target path (e.g. `/photos/123`); `navigation_source` is the
/// path the user came *from* (or `None` for a full-page load). If an intercept
/// matches, the intercept's slot renders in place of the intercepted page for
/// the primary slot.
pub fn resolve_slots(
    slots: &[SlotSpec],
    intercepts: &[InterceptSpec],
    route: &str,
    navigation_source: Option<&str>,
) -> SlotMatch {
    let mut out = SlotMatch::default();

    // 1. Fill in the default slot from the primary path.
    if let Some(children) = slots.iter().find(|s| s.name == "children") {
        if let Some(p) = children.paths.iter().find(|p| matches_route(p, route)) {
            out.slots.insert("children".into(), p.clone());
        }
    }

    // 2. Additional parallel slots — any whose paths match the route.
    for s in slots {
        if s.name == "children" {
            continue;
        }
        if let Some(p) = s.paths.iter().find(|p| matches_route(p, route)) {
            out.slots.insert(s.name.clone(), p.clone());
        }
    }

    // 3. Intercepts override the primary slot when the navigation source matches.
    if let Some(src) = navigation_source {
        for ic in intercepts {
            if !matches_route(&ic.target, route) {
                continue;
            }
            if intercept_applies(ic, src, route) {
                out.slots.insert(ic.slot.clone(), ic.source.clone());
                out.intercepted_from = Some(src.to_string());
            }
        }
    }

    out
}

/// A minimal route matcher: `/a/:id` matches `/a/123`. Supports a trailing
/// `/*` catch-all. Exact segments are case-sensitive.
fn matches_route(pattern: &str, route: &str) -> bool {
    if pattern == route {
        return true;
    }
    let pat: Vec<&str> = pattern.trim_matches('/').split('/').collect();
    let got: Vec<&str> = route.trim_matches('/').split('/').collect();
    if let Some(catch) = pat.iter().position(|s| *s == "*") {
        return got.len() >= catch;
    }
    if pat.len() != got.len() {
        return false;
    }
    for (p, g) in pat.iter().zip(got.iter()) {
        if p.starts_with(':') || *p == *g {
            continue;
        }
        return false;
    }
    true
}

fn intercept_applies(ic: &InterceptSpec, src: &str, route: &str) -> bool {
    // The navigation source must be on the intercepting path itself (or a
    // subpath), and not equal to the target route — you can't intercept from
    // within the intercepted page.
    if src == route {
        return false;
    }
    // The intercept's *anchor* is the directory that contains the segment with
    // the marker. For `source = "/feed/(.)photo/:id"` the anchor is `/feed`:
    // navigation from `/feed` (or below) triggers the intercept.
    let anchor = intercept_anchor(&ic.source);
    match ic.level {
        InterceptLevel::Same => src == anchor || src.starts_with(&format!("{anchor}/")),
        InterceptLevel::UpOne => {
            let up = parent_path(&anchor);
            src == up || src.starts_with(&format!("{up}/"))
        }
        InterceptLevel::Up(n) => {
            let mut p = anchor;
            for _ in 0..n {
                p = parent_path(&p);
            }
            src == p || src.starts_with(&format!("{p}/"))
        }
        InterceptLevel::FromRoot => true,
    }
}

/// Strip the segment that carries the intercept marker; the remaining path is
/// the anchor directory the intercept lives under.
fn intercept_anchor(source: &str) -> String {
    let parts: Vec<&str> = source.trim_matches('/').split('/').collect();
    let idx = parts
        .iter()
        .position(|p| p.starts_with('(') && p.contains(')'));
    let Some(i) = idx else {
        return "/".into();
    };
    if i == 0 {
        return "/".into();
    }
    format!("/{}", parts[..i].join("/"))
}

fn parent_path(p: &str) -> String {
    match p.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => p[..i].into(),
        None => "/".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_name_parsing() {
        assert_eq!(parse_slot_name("@modal").as_deref(), Some("modal"));
        assert_eq!(parse_slot_name("@"), None);
        assert_eq!(parse_slot_name("regular"), None);
    }

    #[test]
    fn intercept_marker_parsing() {
        let (level, rest) = parse_intercept_marker("(.)photo").unwrap();
        assert_eq!(level, InterceptLevel::Same);
        assert_eq!(rest, "photo");
        let (level, rest) = parse_intercept_marker("(..)photo").unwrap();
        assert_eq!(level, InterceptLevel::UpOne);
        assert_eq!(rest, "photo");
        let (level, _) = parse_intercept_marker("(...)photo").unwrap();
        assert_eq!(level, InterceptLevel::FromRoot);
        assert!(parse_intercept_marker("plain").is_none());
    }

    #[test]
    fn matches_route_with_params_and_catchall() {
        assert!(matches_route("/photos/:id", "/photos/123"));
        assert!(matches_route("/photos/*", "/photos/a/b/c"));
        assert!(!matches_route("/photos/:id", "/photos"));
    }

    #[test]
    fn resolve_slots_fills_default_and_named() {
        let slots = vec![
            SlotSpec {
                name: "children".into(),
                paths: vec!["/photos/:id".into()],
            },
            SlotSpec {
                name: "sidebar".into(),
                paths: vec!["/photos/:id".into()],
            },
        ];
        let m = resolve_slots(&slots, &[], "/photos/42", None);
        assert_eq!(m.slots.get("children"), Some(&"/photos/:id".to_string()));
        assert_eq!(m.slots.get("sidebar"), Some(&"/photos/:id".to_string()));
        assert!(m.intercepted_from.is_none());
    }

    #[test]
    fn resolve_slots_intercepts_from_source() {
        let slots = vec![SlotSpec {
            name: "children".into(),
            paths: vec!["/photos/:id".into()],
        }];
        let intercepts = vec![InterceptSpec {
            slot: "children".into(),
            target: "/photos/:id".into(),
            source: "/feed/(.)photo/:id".into(),
            level: InterceptLevel::Same,
        }];
        let m = resolve_slots(&slots, &intercepts, "/photos/7", Some("/feed"));
        assert_eq!(
            m.slots.get("children"),
            Some(&"/feed/(.)photo/:id".to_string())
        );
        assert_eq!(m.intercepted_from.as_deref(), Some("/feed"));
    }

    #[test]
    fn intercept_ignored_from_target_itself() {
        let slots = vec![SlotSpec {
            name: "children".into(),
            paths: vec!["/photos/:id".into()],
        }];
        let intercepts = vec![InterceptSpec {
            slot: "children".into(),
            target: "/photos/:id".into(),
            source: "/feed/(.)photo/:id".into(),
            level: InterceptLevel::Same,
        }];
        // Source equals target — intercept must not fire.
        let m = resolve_slots(&slots, &intercepts, "/photos/7", Some("/photos/7"));
        assert_eq!(m.slots.get("children"), Some(&"/photos/:id".to_string()));
        assert!(m.intercepted_from.is_none());
    }
}
