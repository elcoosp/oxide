//! V2-B02: candidate index for the route matcher (decision D23).
//!
//! Route matching cost must depend on URL depth, not on route count. The index
//! returns, in original order, a **superset** of the entries that can match a
//! URL: an entry is omitted only when its static prefix provably differs from
//! the URL. The existing exact scorer (see `matcher`) then runs on those
//! candidates only, preserving score and tie-break semantics by construction —
//! the old linear matcher (`match_in_linear`) stays as the property-test
//! oracle.
//!
//! Structure: a trie over the *static* leading segments of each matchable
//! entry. At the node where an entry's first dynamic segment (param, optional
//! catch-all, catch-all) would appear, the entry is recorded in that node's
//! `wild` list; entries made only of static segments are recorded in the
//! terminal node's `term` list. A URL walk collects `wild` at every node along
//! its static path plus `term` at its end, so every entry whose full static
//! prefix matches the URL's corresponding prefix is included.
//!
//! Archived trees (memory-mapped, spec §8.2): the index is built once at load
//! from owned copies of the static prefixes. Under the crate's P-01 invariant
//! ("the mapping is read-only and cannot change") the index is exact; the walk
//! additionally reports whether it completed, and the matcher falls back to the
//! exact linear scan when it does not — which also keeps the in-place-patch
//! liveness test (`archived_tree_reads_through_the_live_mmap`) green.

use crate::matcher::EntryLike;
use crate::segment::{SegKind, classify};
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
struct Node {
    /// Static-segment children (owned copies; see module docs for archived trees).
    stat: HashMap<Box<str>, u32>,
    /// Entries whose next segment is dynamic (param, optional, catch-all) and
    /// whose preceding segments are all static and lead to this node.
    wild: Vec<u32>,
    /// Entries made only of static segments that end exactly here.
    term: Vec<u32>,
}

/// Trie over static route prefixes; see [module docs](self).
#[derive(Debug, Clone)]
pub(crate) struct CandidateIndex {
    nodes: Vec<Node>,
}

/// Whether the walk consumed every URL segment through static children.
pub(crate) type Walked = bool;

impl CandidateIndex {
    pub(crate) fn build<E: EntryLike>(entries: &[E]) -> Self {
        let mut nodes = vec![Node::default()];
        for (i, e) in entries.iter().enumerate() {
            // Only pages without a slot marker and endpoints are URL-addressable
            // (§8.5); everything else can never win a match, so it is not indexed.
            if !e.matchable() {
                continue;
            }
            let mut cur = 0usize;
            let mut placed = false;
            for k in 0..e.seg_len() {
                if let SegKind::Static(s) = classify(e.seg(k)) {
                    let next = if let Some(&n) = nodes[cur].stat.get(s) {
                        n as usize
                    } else {
                        nodes.push(Node::default());
                        let n = nodes.len() - 1;
                        nodes[cur]
                            .stat
                            .insert(s.into(), u32::try_from(n).expect("node id"));
                        n
                    };
                    cur = next;
                } else {
                    nodes[cur].wild.push(u32::try_from(i).expect("entry id"));
                    placed = true;
                    break;
                }
            }
            if !placed {
                nodes[cur].term.push(u32::try_from(i).expect("entry id"));
            }
        }
        Self { nodes }
    }

    /// Candidate entry indices for these URL segments, ascending (original
    /// order). Returns whether the walk reached the end of `url` through
    /// static children (`false` means the walk broke early — callers over
    /// possibly-mutated maps must fall back to the exact linear scan).
    ///
    /// `escaped` marks URLs carrying percent-escapes: raw segments then decode
    /// per level before the child lookup (rare path; the common path is
    /// borrowed and allocation-free).
    pub(crate) fn candidates(
        &self,
        url: &[&str],
        escaped: bool,
        out: &mut smallvec::SmallVec<[u32; 16]>,
    ) -> Walked {
        let (mut cur, mut d) = (0usize, 0usize);
        loop {
            // Dynamic entries reachable here (also consulted at `d == url.len()`,
            // which is what lets `[[...optional]]` match zero segments).
            out.extend_from_slice(&self.nodes[cur].wild);
            if d == url.len() {
                out.extend_from_slice(&self.nodes[cur].term);
                return true;
            }
            let next = if escaped {
                let decoded = crate::segment::percent_decode(url[d]);
                self.nodes[cur].stat.iter().find_map(|(k, n)| {
                    if &**k == decoded.as_str() {
                        Some(*n as usize)
                    } else {
                        None
                    }
                })
            } else {
                self.nodes[cur].stat.get(url[d]).map(|&n| n as usize)
            };
            match next {
                Some(n) => {
                    cur = n;
                    d += 1;
                }
                None => return false,
            }
        }
    }

    /// All matchable entry indices (linear fallback, ascending).
    pub(crate) fn all(entries_len: usize) -> smallvec::SmallVec<[u32; 16]> {
        (0..u32::try_from(entries_len).expect("entry count fits u32")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::match_in;
    use crate::{RouteEntry, RouteId, RouteKind, RouteTree};

    fn page(id: &str, segments: &[&str]) -> RouteEntry {
        RouteEntry {
            id: RouteId(id.to_string()),
            kind: RouteKind::Page,
            segments: segments
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            file: format!("{id}.tsx").into(),
            render_mode: None,
            tags: Vec::new(),
            has_server_module: false,
            needs_parent: false,
            slot: None,
        }
    }

    fn tree(entries: Vec<RouteEntry>) -> RouteTree {
        let mut t = RouteTree::new();
        for e in entries {
            t.insert(e);
        }
        t
    }

    /// V2-B02 test 1: for every fixture and every URL in a generated list, the
    /// linear winner must be contained in `candidates()`.
    #[test]
    fn index_candidates_superset() {
        let fixtures: Vec<Vec<RouteEntry>> = vec![
            vec![
                page("blog/:slug", &["blog", ":slug"]),
                page("blog/new", &["blog", "new"]),
            ],
            vec![
                page("en/blog", &["en", "blog"]),
                page(":locale/about", &[":locale", "about"]),
                page("*?rest", &["*?rest"]),
            ],
            vec![
                page("docs/*rest", &["docs", "*rest"]),
                page("a/:n{[0-9]+}/edit", &["a", ":n{[0-9]+}", "edit"]),
            ],
            vec![page("root", &[])],
        ];
        let urls = [
            "",
            "/",
            "/blog",
            "/blog/new",
            "/blog/x",
            "/en/blog",
            "/en/about",
            "/en/x",
            "/about",
            "/x/y/z",
            "/docs",
            "/docs/a",
            "/docs/a/b/c",
            "/a/1/edit",
            "/a/x/edit",
            "/a/1",
            "//a//1/edit//",
            "/a%20b",
            "/zzz",
            "/zzz/deep/path/here",
        ];
        for entries in &fixtures {
            let index = CandidateIndex::build(entries);
            for url in urls {
                let segs: Vec<&str> = url
                    .split(['?', '#'])
                    .next()
                    .unwrap_or(url)
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .collect();
                let mut cands = smallvec::SmallVec::<[u32; 16]>::new();
                let escaped = segs.iter().any(|s| s.contains('%'));
                index.candidates(&segs, escaped, &mut cands);
                if let Some((idx, _, _)) = match_in(entries, url) {
                    assert!(
                        cands.contains(&(u32::try_from(idx).expect("entry id"))),
                        "linear winner {idx} ({:?}) missing from candidates {cands:?} for {url:?}",
                        entries[idx].segments
                    );
                }
                // And the winner through the indexed path must be the same route.
                let t = tree(entries.clone());
                let linear = match_in(entries, url).map(|(i, _, _)| entries[i].id.clone());
                let indexed = t.match_path(url).map(|m| m.route.id.clone());
                assert_eq!(linear, indexed, "indexed winner differs for {url:?}");
            }
        }
    }

    /// Non-matchable entries (layouts, slots) are never indexed: they cannot
    /// appear in candidates and can never win.
    #[test]
    fn unmatchable_entries_are_not_indexed() {
        let mut layout = page("routes/layout", &[]);
        layout.kind = RouteKind::Layout;
        let mut slot_page = page("routes/@modal/page", &[]);
        slot_page.slot = Some("@modal".to_string());
        let entries = vec![layout, slot_page, page("routes/blog/page", &["blog"])];
        let index = CandidateIndex::build(&entries);
        let mut cands = smallvec::SmallVec::<[u32; 16]>::new();
        assert!(index.candidates(&["blog"], false, &mut cands));
        assert_eq!(
            cands.as_slice(),
            &[2u32],
            "only the matchable page is indexed"
        );
    }
}
