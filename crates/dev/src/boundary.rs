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
//! Every public item here is pure and allocation-bounded by the graph size;
//! the tracing spans (`hmr_boundary`) make the per-tick planner cost visible
//! under `RUST_LOG=oxide_fw_dev=debug`.

use oxide_fw_diagnostics::Diagnostic;
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
