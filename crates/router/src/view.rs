//! V2-B02: one match view over both trees — the seam the dispatch
//! call-sites consume.
//!
//! The two zero-copy match results are structurally different: the owned
//! [`RouteTree::match_ref`] borrows the winning [`RouteEntry`], while the
//! memory-mapped [`ArchivedTree::match_ref`] identifies the winner by
//! index and keeps the mapped bytes the single source of truth (the P-01
//! liveness contract — see `docs/deviations.md` (the
//! `ArchivedTree::match_ref` shape). Call-sites that consume either tree fell back to
//! `match_path`, which materializes a full owned [`MatchResult`] per match
//! just to read one field. [`MatchedView`] resolves the split:
//!
//! - **`route_id` / `score` / `params` / `is_live_route`** read both arms
//!   in place — no entry materialization (the archived strings stay in
//!   the mmap, the live entry stays borrowed);
//! - **`layout_chain`** reuses the per-tree chain oracle the
//!   `MatchResult` construction uses today (the P-02 cached chain for the
//!   owned tree, the entry-scan chain for the archive);
//! - **`into_match_result`** produces the byte-identical owned
//!   [`MatchResult`] for callers that need owned data — the same shape
//!   `match_path` returns, reached through the index-based hot path.
//!
//! Pinned by the tests at the bottom (live and archived arms against the
//! `match_path` oracle) — V8-free, they run everywhere.

use crate::matcher::EntryLike;
use crate::{ArchivedMatchRef, ArchivedTree, MatchRef, MatchResult, RouteId, RouteTree};
use std::borrow::Cow;

/// One match against either tree, resolved to a uniform read-only view.
#[derive(Debug, Clone)]
pub enum MatchedView<'a> {
    /// The owned tree: the winning entry by reference, captures borrowed.
    Live {
        /// The tree the match ran against (the cached chain oracle).
        tree: &'a RouteTree,
        /// The zero-copy match (borrowed entry + borrowed captures).
        m: MatchRef<'a>,
    },
    /// The memory-mapped archive: the winner by index over the mapped
    /// entries.
    Archived {
        /// The archive the index refers into (for in-place entry access).
        tree: &'a ArchivedTree,
        /// The zero-copy match (index + borrowed captures).
        m: ArchivedMatchRef<'a>,
    },
}

impl<'a> MatchedView<'a> {
    /// Match `path` against the owned tree through the hot path.
    #[must_use]
    pub fn live(tree: &'a RouteTree, path: &'a str) -> Option<Self> {
        tree.match_ref(path).map(|m| Self::Live { tree, m })
    }

    /// Match `path` against the memory-mapped archive through the hot
    /// path.
    #[must_use]
    pub fn archived(tree: &'a ArchivedTree, path: &'a str) -> Option<Self> {
        tree.match_ref(path).map(|m| Self::Archived { tree, m })
    }

    /// The matched route's id (`routes/blog/page`), read in place on both
    /// arms.
    #[must_use]
    pub fn route_id(&self) -> &'a str {
        match self {
            Self::Live { m, .. } => &m.route.id.0,
            Self::Archived { tree, m } => tree.root().entries.as_slice()[m.route_index].id_str(),
        }
    }

    /// The match score (higher wins), identical to
    /// [`MatchResult::score`].
    #[must_use]
    pub fn score(&self) -> i64 {
        match self {
            Self::Live { m, .. } => m.score,
            Self::Archived { m, .. } => m.score,
        }
    }

    /// The captured params as `(name, value)` pairs — borrowed where the
    /// URL carried no escapes, owned only for catch-all joins and decoded
    /// escapes (the shared `Captures` shape both arms carry).
    pub fn params(&self) -> impl Iterator<Item = (&str, Cow<'_, str>)> {
        let captures = match self {
            Self::Live { m, .. } => &m.params,
            Self::Archived { m, .. } => &m.params,
        };
        captures.iter().map(|(k, v)| (k.as_ref(), v.clone()))
    }

    /// Whether the matched route opted into the live machinery
    /// (`RouteEntry.live.is_some()`) — read in place on both arms, so the
    /// prelude's live gate materializes nothing.
    #[must_use]
    pub fn is_live_route(&self) -> bool {
        match self {
            Self::Live { m, .. } => m.route.live.is_some(),
            Self::Archived { tree, m } => tree.root().entries.as_slice()[m.route_index]
                .live
                .as_ref()
                .is_some(),
        }
    }

    /// The layout chain, outer to inner — the same chain the owned
    /// `MatchResult` carries (the P-02 cached chain for the owned tree,
    /// the entry-scan chain for the archive).
    #[must_use]
    pub fn layout_chain(&self) -> Vec<RouteId> {
        match self {
            Self::Live { tree, m } => tree.layout_chain_cached(&m.route.id.0),
            Self::Archived { tree, m } => crate::matcher::layout_chain(
                tree.root().entries.as_slice(),
                tree.root().entries.as_slice()[m.route_index].id_str(),
            ),
        }
    }

    /// The owned [`MatchResult`] — byte-identical to what `match_path`
    /// returns for the same path (the oracle the tests pin), reached
    /// through the index-based hot path.
    #[must_use]
    pub fn into_match_result(&self) -> MatchResult {
        let params = self
            .params()
            .map(|(k, v)| (k.to_owned(), v.into_owned()))
            .collect();
        match self {
            Self::Live { m, .. } => MatchResult {
                route: m.route.clone(),
                layout_chain: self.layout_chain(),
                params,
                score: m.score,
            },
            Self::Archived { tree, m } => MatchResult {
                route: crate::archive::entry_from_arch(
                    &tree.root().entries.as_slice()[m.route_index],
                ),
                layout_chain: self.layout_chain(),
                params,
                score: m.score,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RouteEntry, RouteId, RouteKind};

    fn entry(id: &str, kind: RouteKind, segments: &[&str]) -> RouteEntry {
        RouteEntry {
            id: RouteId(id.to_string()),
            kind,
            segments: segments
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            file: format!("{id}.tsx").into(),
            render_mode: None,
            tags: Vec::new(),
            has_server_module: false,
            needs_parent: false,
            live: None,
            priority: None,
            slot: None,
        }
    }

    fn fixture_tree() -> RouteTree {
        let mut t = RouteTree::new();
        t.insert(entry("routes/page", RouteKind::Page, &[]));
        t.insert(entry("routes/blog/page", RouteKind::Page, &["blog"]));
        t.insert(entry(
            "routes/blog/[slug]/page",
            RouteKind::Page,
            &["blog", ":slug"],
        ));
        t.insert(entry(
            "routes/files/[...rest]/page",
            RouteKind::Page,
            &["files", "*rest"],
        ));
        t.insert(entry("routes/layout", RouteKind::Layout, &[]));
        t
    }

    fn live_opted(mut e: RouteEntry) -> RouteEntry {
        e.live = Some(crate::RouteLive { debounce_ms: None });
        e
    }

    const PATHS: [&str; 8] = [
        "/",
        "/blog",
        "/blog/hello",
        "/files/a/b/c",
        "/nope",
        "/blog/",
        "/blog/a/b",
        "/files/x",
    ];

    /// The live arm's owned result equals the `match_path` oracle, and the
    /// in-place accessors agree with the owned shape.
    #[test]
    fn view_live_matches_the_owned_oracle() {
        let tree = fixture_tree();
        for path in PATHS {
            let oracle = tree.match_path(path);
            let view = MatchedView::live(&tree, path);
            assert_eq!(
                view.as_ref().map(MatchedView::into_match_result),
                oracle.clone(),
                "live view diverges from match_path for {path}"
            );
            if let (Some(v), Some(o)) = (&view, &oracle) {
                assert_eq!(v.route_id(), o.route.id.0);
                assert_eq!(v.score(), o.score);
                assert_eq!(v.layout_chain(), o.layout_chain);
                assert_eq!(v.is_live_route(), o.route.live.is_some());
            }
        }
    }

    /// The archived arm's owned result equals the owned tree's oracle —
    /// the seam serves both trees with one shape.
    #[test]
    fn view_archived_matches_the_owned_oracle() {
        let tree = fixture_tree();
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("routes.manifest");
        tree.archive_to(&archive).expect("archive writes");
        let archived = crate::ArchivedTree::load_archived(&archive).expect("archive loads");
        for path in PATHS {
            let oracle = tree.match_path(path);
            let view = MatchedView::archived(&archived, path);
            assert_eq!(
                view.as_ref().map(MatchedView::into_match_result),
                oracle.clone(),
                "archived view diverges from match_path for {path}"
            );
            if let (Some(v), Some(o)) = (&view, &oracle) {
                assert_eq!(v.route_id(), o.route.id.0);
                assert_eq!(v.layout_chain(), o.layout_chain);
            }
        }
    }

    /// `is_live_route` reads both arms in place — an opted route answers
    /// true without materializing an entry.
    #[test]
    fn view_is_live_route_reads_both_trees() {
        let mut tree = fixture_tree();
        let page = tree
            .entries
            .iter()
            .position(|e| e.id.0 == "routes/blog/page")
            .unwrap();
        tree.entries[page] = live_opted(tree.entries[page].clone());
        for path in ["/blog", "/"] {
            let want = tree.match_path(path).unwrap().route.live.is_some();
            assert_eq!(
                MatchedView::live(&tree, path).unwrap().is_live_route(),
                want,
                "live arm disagrees for {path}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("routes.manifest");
        tree.archive_to(&archive).unwrap();
        let archived = crate::ArchivedTree::load_archived(&archive).unwrap();
        assert!(
            MatchedView::archived(&archived, "/blog")
                .unwrap()
                .is_live_route(),
            "the archived arm reads the opted-in route's live field in place"
        );
        assert!(
            !MatchedView::archived(&archived, "/")
                .unwrap()
                .is_live_route()
        );
    }

    /// The params accessor yields the same pairs the owned shape carries,
    /// including a catch-all join (owned capture) and a plain param
    /// (borrowed capture).
    #[test]
    fn view_params_match_the_owned_shape() {
        let tree = fixture_tree();
        for path in ["/blog/hello", "/files/a/b/c"] {
            let oracle = tree.match_path(path).unwrap();
            let view = MatchedView::live(&tree, path).unwrap();
            let owned: Vec<(String, String)> = oracle.params.clone().into_iter().collect();
            let viewed: Vec<(String, String)> = view
                .params()
                .map(|(k, v)| (k.to_owned(), v.into_owned()))
                .collect();
            assert_eq!(viewed, owned, "params diverge for {path}");
        }
    }
}
