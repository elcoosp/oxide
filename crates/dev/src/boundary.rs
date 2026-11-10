//! The V2-E01 HMR boundary planner (spec §V2-E01, §13.3).
//!
//! Given the changed module and the dev-time module graph — reverse deps
//! (`importers`), the self-accepting set (`accepting`) and the entry set
//! (`entries`) — [`plan`] computes the smallest update surface: walk the
//! importers breadth-first until a self-accepting module is reached on every
//! path; if any path reaches an entry module (or an orphan) without a
//! boundary, fall back to [`Plan::FullReload`] with an actionable reason.
//! The walk is cycle-safe (`visited` set).
//!
//! This replaces the §13.3 one-hop simplification (`update_boundary`, which
//! only followed a single importer and stopped on the first fork): a
//! non-boundary chain `util.ts ← util2.ts ← page.tsx` now updates `page.tsx`
//! instead of leaving its import binding stale.
//!
//! **Phase 2 (the CSS boundary substrate).** The spec's message schema
//! carries a `kind: 'js' | 'css' | 'data'` discriminant, and its CSS rule is
//! a terminal: a stylesheet swaps in place and NEVER reloads (§13.3 — the
//! `<link>` / `<style data-ox-css=path>` swap). [`plan_classified`] composes
//! the walk with that rule: [`update_kind`] maps the watcher's
//! [`ChangeKind`](crate::ChangeKind) onto the wire kind, and a `FullReload`
//! walk for a stylesheet collapses to an in-place swap of the changed file
//! itself — the graph beneath it (or the absence of one) is reported
//! through [`css_orphan_diagnostic`] (`DEV-HMR-002`) instead of user-visible
//! reload noise. [`css_self_accepts`] is the extension-level probe the
//! harvest uses to mark stylesheets as boundaries.
//!
//! Every public item here is pure and allocation-bounded by the graph size;
//! the tracing spans (`hmr_boundary`, `hmr_boundary_classified`) make the
//! per-tick planner cost visible under `RUST_LOG=oxide_fw_dev=debug`.

use crate::ChangeKind;
use oxide_fw_diagnostics::Diagnostic;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// The plan a changed module produces (§V2-E01).
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum Plan {
    /// Update these boundaries in place. Sorted and deduplicated — the
    /// deterministic batch order the §3.2 coalescing publishes.
    Update(Vec<PathBuf>),
    /// No boundary found on some path: reload the client. The reason is the
    /// spec's spelling (`<module> has no HMR boundary` /
    /// `<module> is not imported`) and feeds
    /// [`full_reload_diagnostic`] for the §13.4 overlay.
    FullReload(String),
}

/// Compute the smallest update boundary set for `changed` (§V2-E01).
///
/// The inputs are the dev-time module graph:
///
/// - `importers` — reverse deps: `module -> its importers` (the same
///   orientation `oxide-fw-cli`'s graph harvest builds).
/// - `accepting` — modules that self-accept: the adapter's compile step
///   registered `import.meta.hot.accept` ([`source_self_accepts`]) or the
///   framework plugin marks the module as a boundary (component files,
///   server/CSS swap files).
/// - `entries` — entry modules; reaching one without a boundary is a
///   full reload.
///
/// The boundary check runs BEFORE the entry check, so a module that is both
/// (a route `page.tsx` is a component boundary) updates in place.
#[must_use]
pub fn plan(
    changed: &Path,
    importers: &HashMap<PathBuf, Vec<PathBuf>>,
    accepting: &HashSet<PathBuf>,
    entries: &HashSet<PathBuf>,
) -> Plan {
    let _span = tracing::debug_span!("hmr_boundary", changed = %changed.display()).entered();
    let mut boundaries: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut q: VecDeque<PathBuf> = VecDeque::from([changed.to_path_buf()]);
    while let Some(m) = q.pop_front() {
        if !seen.insert(m.clone()) {
            continue;
        }
        if accepting.contains(&m) {
            boundaries.push(m);
            continue;
        }
        if entries.contains(&m) {
            let reason = format!("{} has no HMR boundary", m.display());
            tracing::debug!(reason = %reason, "hmr_boundary: full reload");
            return Plan::FullReload(reason);
        }
        match importers.get(&m) {
            Some(v) if !v.is_empty() => q.extend(v.iter().cloned()),
            _ => {
                let reason = format!("{} is not imported", m.display());
                tracing::debug!(reason = %reason, "hmr_boundary: full reload");
                return Plan::FullReload(reason);
            }
        }
    }
    boundaries.sort();
    boundaries.dedup();
    tracing::debug!(boundaries = boundaries.len(), "hmr_boundary: plan computed");
    Plan::Update(boundaries)
}

/// §V2-E01: a module is self-accepting when its source registers
/// `import.meta.hot.accept` (the adapter's compile step emits the
/// registration). The scan is a substring probe on purpose — the planner
/// runs per watcher tick and must not parse.
#[must_use]
pub fn source_self_accepts(source: &str) -> bool {
    source.contains("import.meta.hot.accept")
}

/// The miette help-text seam for a [`Plan::FullReload`] (§13.4): a
/// `DEV-HMR-001` warning carrying the reason plus the fix line the overlay
/// (and `oxide why`-style consumers) render.
#[must_use]
pub fn full_reload_diagnostic(reason: &str) -> Diagnostic {
    Diagnostic::warning(
        "DEV-HMR-001",
        format!("HMR fell back to a full reload: {reason}"),
    )
    .with_fix(
        "mark the nearest module along the chain self-accepting \
         (`import.meta.hot.accept()`) or let the framework plugin mark component \
         modules as boundaries; if the file is not imported anywhere, delete it \
         or fix the specifier typo",
    )
}

/// The §13.3 message schema's update-kind discriminant
/// (`kind: 'js' | 'css' | 'data'`). The TS client switches on the exact
/// spelling, so [`UpdateKind::as_str`] is wire contract, not decoration —
/// and since the message-schema v1 flip, the kind rides the wire directly
/// inside [`crate::UpdateEntry`], so the serde derives spell it lowercase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateKind {
    /// A client module changed: the boundary re-imports (adapter HMR) and
    /// the framework runtime applies the patch in place.
    Js,
    /// A stylesheet changed: the `/@css/` module (ESM mode) or the `<link>` /
    /// `<style data-ox-css>` element swaps in place — never a reload (the
    /// §13.3 CSS terminal [`plan_classified`] applies).
    Css,
    /// A `page.server.ts` loader changed: the route re-runs the loader and
    /// pushes fresh ODF data (`OxData`, or a `data` entry the client
    /// re-fetches) — no module re-import.
    Data,
}

impl UpdateKind {
    /// The spec's wire spelling for the schema `kind` field.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateKind::Js => "js",
            UpdateKind::Css => "css",
            UpdateKind::Data => "data",
        }
    }
}

/// SPEC-14 bridge: the watcher's [`ChangeKind`] (what changed) maps onto the
/// schema's [`UpdateKind`] (what the client must do). A loader change is a
/// data update; a stylesheet is a css swap; everything else re-imports as a
/// module update.
#[must_use]
pub fn update_kind(kind: ChangeKind) -> UpdateKind {
    match kind {
        ChangeKind::Css => UpdateKind::Css,
        ChangeKind::ServerLoader => UpdateKind::Data,
        ChangeKind::Component | ChangeKind::ServerModule | ChangeKind::ConfigOrRoutes => {
            UpdateKind::Js
        }
    }
}

/// The §13.3 CSS terminal probe: a stylesheet self-accepts through style
/// injection. The probe is extension-level on purpose — the planner runs per
/// watcher tick and must not parse (the same class as
/// [`source_self_accepts`]). Case-insensitive on the extension.
#[must_use]
pub fn css_self_accepts(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "css" | "scss" | "sass")
}

/// The classified result of [`plan_classified`]: the §13.3 wire
/// [`UpdateKind`] plus the planner's [`Plan`] with the CSS terminal applied
/// (a stylesheet never reloads — a `FullReload` walk for a stylesheet
/// collapses to an in-place swap of the changed file itself).
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct ClassifiedPlan {
    /// The wire kind — the schema v1 `kind` discriminant.
    pub kind: UpdateKind,
    /// The update surface: `Update(boundaries)` in the planner's batch
    /// order, or the reasoned `FullReload` for the `Js`/`Data` kinds (the
    /// `Css` kind collapses before it can reach a caller).
    pub plan: Plan,
}

impl ClassifiedPlan {
    /// True when the plan fell back to a full reload. For a stylesheet this
    /// is always `false` — the §13.3 terminal owns the decision.
    #[must_use]
    pub fn is_full_reload(&self) -> bool {
        matches!(self.plan, Plan::FullReload(_))
    }
}

/// Classify `changed` into the §13.3 wire kind and compute its update
/// surface in one call — the fan-out's decision function (§V2-E01 phase 2).
///
/// The walk is [`plan`]; the classification is [`update_kind`] over the
/// watcher's [`ChangeKind`]; the composition is the §13.3 CSS terminal:
///
/// - `kind = Css`: a stylesheet self-accepts through style injection, so a
///   `FullReload` walk **collapses** to `Update([changed])` — the swap the
///   `/@css/` module or the `<style data-ox-css>` element performs. The
///   collapse is logged on the `hmr_boundary_classified` span; when the
///   collapsed reason is "is not imported", the stylesheet is orphaned and
///   [`css_orphan_diagnostic`] carries the `DEV-HMR-002` warning.
/// - `kind = Js` / `kind = Data`: the walk's verdict passes through
///   unchanged — the terminal is CSS-only, by spec.
///
/// Deterministic: the same inputs produce the same [`ClassifiedPlan`]
/// (the walk sorts and deduplicates before it returns).
pub fn plan_classified(
    kind: ChangeKind,
    changed: &Path,
    importers: &HashMap<PathBuf, Vec<PathBuf>>,
    accepting: &HashSet<PathBuf>,
    entries: &HashSet<PathBuf>,
) -> ClassifiedPlan {
    let _span =
        tracing::debug_span!("hmr_boundary_classified", changed = %changed.display()).entered();
    let wire_kind = update_kind(kind);
    let walked = plan(changed, importers, accepting, entries);
    let plan = match (wire_kind, walked) {
        (UpdateKind::Css, Plan::FullReload(reason)) => {
            tracing::debug!(
                reason = %reason,
                "hmr_boundary_classified: the §13.3 CSS terminal collapses \
                 the reload to a style swap"
            );
            Plan::Update(vec![changed.to_path_buf()])
        }
        (_, walked_plan) => walked_plan,
    };
    ClassifiedPlan {
        kind: wire_kind,
        plan,
    }
}

/// The `DEV-HMR-002` orphan-stylesheet warning (§13.3, phase 2): a
/// stylesheet under `src/` that nothing imports cannot reach a client
/// through the module graph. The style swap still runs — a static `<link>`
/// in the shell may carry the sheet — but the boundary fan-out will never
/// include it, and the dev should know why.
#[must_use]
pub fn css_orphan_diagnostic(path: &Path) -> Diagnostic {
    Diagnostic::warning(
        "DEV-HMR-002",
        format!("{} is not imported by any module", path.display()),
    )
    .with_fix(
        "import the stylesheet from a component or module \
         (`import './styles.css'`) or reference it from a static `<link>`; \
         a stylesheet the module graph cannot see only updates through the \
         swap, never through a boundary fan-out",
    )
}

/// The `DEV-HMR-003` schema-mismatch warning (§13.3, the message-schema v1
/// flip): a peer spoke another wire generation — a frame without a version
/// (legacy v0) or a foreign `v`. The dev loop drops the frame loudly; the
/// fix text names both escape hatches (the stale tab and the stale CLI).
#[must_use]
pub fn schema_mismatch_diagnostic(found: u32) -> Diagnostic {
    Diagnostic::warning(
        "DEV-HMR-003",
        format!(
            "HMR frame speaks schema v{found}; this build speaks v{}",
            crate::HmrMessage::SCHEMA_VERSION
        ),
    )
    .with_fix(
        "hard-reload the stale tab (it keeps the previous session's client \
         runtime) or rebuild the CLI (the dev server and the client must \
         speak the same message-schema version); frames from the mismatched \
         side are dropped until the versions agree",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_accept_scans_the_call_not_the_module_system() {
        assert!(source_self_accepts("import.meta.hot.accept(() => {});"));
        assert!(source_self_accepts("import.meta.hot.acceptDependencies(['./x'])"));
        assert!(!source_self_accepts("import.meta.env.MODE;"));
        assert!(!source_self_accepts(""));
    }

    #[test]
    fn full_reload_diagnostic_is_a_warning_with_the_dev_hmr_code() {
        let d = full_reload_diagnostic("src/lib/util.ts is not imported");
        assert_eq!(d.code, "DEV-HMR-001");
        assert_eq!(d.severity, oxide_fw_diagnostics::Severity::Warning);
        assert!(d.fix.is_some());
    }

    #[test]
    fn plan_update_is_sorted_and_deduped_for_diamond_graphs() {
        let changed = Path::new("src/lib/util.ts");
        let mut importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        importers.insert(
            PathBuf::from("src/lib/util.ts"),
            vec![
                PathBuf::from("src/comp/b.tsx"),
                PathBuf::from("src/comp/a.tsx"),
                PathBuf::from("src/comp/a.tsx"),
            ],
        );
        let accepting: HashSet<PathBuf> = [
            PathBuf::from("src/comp/a.tsx"),
            PathBuf::from("src/comp/b.tsx"),
        ]
        .into_iter()
        .collect();
        let entries: HashSet<PathBuf> = HashSet::new();
        assert_eq!(
            plan(changed, &importers, &accepting, &entries),
            Plan::Update(vec![
                PathBuf::from("src/comp/a.tsx"),
                PathBuf::from("src/comp/b.tsx"),
            ])
        );
    }
}
