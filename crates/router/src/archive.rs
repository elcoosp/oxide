//! rkyv archive of the route tree (§8.2, D8): build-time serialization, memory-mapped
//! at boot, matches read the mapped bytes in place.
//!
//! Archive strategy: the public [`RouteEntry`] keeps its `file: PathBuf`; on disk we
//! store a shadow DTO (`ArchEntry`) whose `file` is a UTF-8 `String` (lossy). On load,
//! matched entries convert back to owned `RouteEntry`s. This keeps the public struct
//! shape untouched while staying inside rkyv's supported types.

use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};

use rkyv::option::ArchivedOption;
use rkyv::string::ArchivedString;
use rkyv::{rancor, Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};

use crate::matcher::{layout_chain, match_in, EntryLike};
use crate::{
    ferr, ArchivedRouteKind, ArchivedTree, MatchResult, Result, RouteEntry, RouteId, RouteKind,
    RouteTree,
};

/// Shadow DTO written to the archive (see module docs).
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub(crate) struct ArchiveRoot {
    pub entries: Vec<ArchEntry>,
}

/// One route as stored on disk.
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub(crate) struct ArchEntry {
    pub id: RouteId,
    pub kind: RouteKind,
    pub segments: Vec<String>,
    pub file: String,
    pub render_mode: Option<String>,
    pub tags: Vec<String>,
    pub has_server_module: bool,
    pub slot: Option<String>,
}

impl From<&RouteEntry> for ArchEntry {
    fn from(e: &RouteEntry) -> Self {
        Self {
            id: e.id.clone(),
            kind: e.kind.clone(),
            segments: e.segments.clone(),
            file: e.file.to_string_lossy().into_owned(),
            render_mode: e.render_mode.clone(),
            tags: e.tags.clone(),
            has_server_module: e.has_server_module,
            slot: e.slot.clone(),
        }
    }
}

/// Access (and validate) the archived root inside a byte slice.
fn access(bytes: &[u8]) -> Result<&ArchivedArchiveRoot> {
    rkyv::access::<ArchivedArchiveRoot, rancor::Error>(bytes).map_err(|e| {
        ferr(
            "RTR-102",
            format!("route archive is corrupt: {e}"),
            "rebuild the route manifest (`oxide build` rewrites .oxide/routes.manifest)",
        )
    })
}

/// Convert one archived entry back into the public owned shape.
pub(crate) fn entry_from_arch(a: &ArchivedArchEntry) -> RouteEntry {
    RouteEntry {
        id: RouteId(a.id.0.to_string()),
        kind: EntryLike::kind(a),
        segments: a.segments.iter().map(|s| s.to_string()).collect(),
        file: PathBuf::from(&*a.file),
        render_mode: arch_opt_to_owned(&a.render_mode),
        tags: a.tags.iter().map(|s| s.to_string()).collect(),
        has_server_module: a.has_server_module,
        slot: arch_opt_to_owned(&a.slot),
    }
}

fn arch_opt_to_owned(o: &ArchivedOption<ArchivedString>) -> Option<String> {
    match o {
        ArchivedOption::Some(s) => Some(s.to_string()),
        ArchivedOption::None => None,
    }
}

pub(crate) fn archive_impl(tree: &RouteTree, out: &Path) -> Result<()> {
    let root = ArchiveRoot {
        entries: tree.entries.iter().map(ArchEntry::from).collect(),
    };
    let bytes = rkyv::to_bytes::<rancor::Error>(&root).map_err(|e| {
        ferr(
            "RTR-100",
            format!("failed to serialize route archive: {e}"),
            "check route entries for non-UTF-8 paths or cycles",
        )
    })?;
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| {
                ferr(
                    "RTR-100",
                    format!("cannot create archive directory {}: {e}", parent.display()),
                    "check write permissions for the output directory",
                )
            })?;
        }
    }
    fs::write(out, &bytes).map_err(|e| {
        ferr(
            "RTR-100",
            format!("cannot write route archive {}: {e}", out.display()),
            "check write permissions for the output path",
        )
    })?;
    Ok(())
}

pub(crate) fn load_impl(path: &Path) -> Result<ArchivedTree> {
    let file = File::open(path).map_err(|e| {
        ferr(
            "RTR-101",
            format!("cannot open route archive {}: {e}", path.display()),
            "build first: `oxide build` writes .oxide/routes.manifest",
        )
    })?;
    // SAFETY: the mapping is read-only over a regular file we just opened; no
    // in-process writer can shrink it under us. (The contract field is `memmap2::Mmap`,
    // whose only constructors are `unsafe`.)
    let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| {
        ferr(
            "RTR-101",
            format!("cannot memory-map route archive {}: {e}", path.display()),
            "ensure the archive is a regular file on a local filesystem",
        )
    })?;
    // Validate once at boot so corruption surfaces with a clear code, not a silent
    // None on the first request.
    access(&mmap[..])?;
    Ok(ArchivedTree { _mmap: mmap })
}

pub(crate) fn arch_match_impl(tree: &ArchivedTree, path: &str) -> Option<MatchResult> {
    // Safe access validates on every call (no unsafe aliasing is allowed in this
    // crate); strings and segments are read in place from the mapping and only the
    // matched entry is materialized into an owned `RouteEntry`.
    let root = access(&tree._mmap[..]).ok()?;
    let entries = root.entries.as_slice();
    let (idx, params, score) = match_in(entries, path)?;
    let a = &entries[idx];
    Some(MatchResult {
        route: entry_from_arch(a),
        layout_chain: layout_chain(entries, a.id_str()),
        params,
        score,
    })
}

pub(crate) fn arch_len_impl(tree: &ArchivedTree) -> usize {
    access(&tree._mmap[..])
        .map(|root| root.entries.len())
        .unwrap_or(0)
}

impl EntryLike for ArchivedArchEntry {
    fn id_str(&self) -> &str {
        &self.id.0
    }
    fn kind(&self) -> RouteKind {
        match self.kind {
            ArchivedRouteKind::Page => RouteKind::Page,
            ArchivedRouteKind::Layout => RouteKind::Layout,
            ArchivedRouteKind::Loading => RouteKind::Loading,
            ArchivedRouteKind::Error => RouteKind::Error,
            ArchivedRouteKind::NotFound => RouteKind::NotFound,
            ArchivedRouteKind::Template => RouteKind::Template,
            ArchivedRouteKind::Socket => RouteKind::Socket,
            ArchivedRouteKind::Endpoint => RouteKind::Endpoint,
            ArchivedRouteKind::Middleware => RouteKind::Middleware,
        }
    }
    fn seg_len(&self) -> usize {
        self.segments.len()
    }
    fn seg(&self, i: usize) -> &str {
        &self.segments[i]
    }
    fn slot(&self) -> Option<&str> {
        match &self.slot {
            ArchivedOption::Some(s) => Some(&**s),
            ArchivedOption::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RouteTree;

    fn put(root: &Path, rel: &str, contents: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }

    fn fixture(root: &Path) -> RouteTree {
        put(root, "layout.tsx", "l");
        put(root, "blog/layout.tsx", "l");
        put(root, "blog/page.tsx", "p");
        put(root, "blog/[slug]/page.tsx", "p");
        RouteTree::scan(root).unwrap()
    }

    #[test]
    fn archive_roundtrip_matches_identically() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = fixture(tmp.path());
        let manifest = tmp.path().join(".oxide/routes.manifest");
        tree.archive_to(&manifest).unwrap();
        let archived = RouteTree::load_archived(&manifest).unwrap();
        assert_eq!(archived.len(), tree.entries.len());
        assert!(!archived.is_empty());
        for path in ["/", "/blog", "/blog/x", "/blog/a/b", "/nope"] {
            assert_eq!(
                tree.match_path(path),
                archived.match_path(path),
                "archive match diverges for {path}"
            );
        }
        // Layout chains survive the archive.
        let m = archived.match_path("/blog/x").unwrap();
        let ids: Vec<&str> = m.layout_chain.iter().map(|i| i.0.as_str()).collect();
        assert_eq!(ids, vec!["routes/layout", "routes/blog/layout"]);
    }

    #[test]
    fn archived_tree_reads_through_the_live_mmap() {
        let tmp = tempfile::tempdir().unwrap();
        let mut tree = RouteTree::new();
        tree.insert(RouteEntry {
            id: RouteId("routes/blog/page".to_string()),
            kind: RouteKind::Page,
            segments: vec!["blog".to_string()],
            file: "blog/page.tsx".into(),
            render_mode: None,
            tags: Vec::new(),
            has_server_module: false,
            slot: None,
        });
        let manifest = tmp.path().join("routes.manifest");
        tree.archive_to(&manifest).unwrap();
        let archived = RouteTree::load_archived(&manifest).unwrap();
        assert!(archived.match_path("/blog").is_some());

        // Patch the pattern bytes in place ("blog" -> "bl9g"): if the tree matches
        // through the mapping, the change is observable without reloading.
        let bytes = std::fs::read(&manifest).unwrap();
        let mut offsets = Vec::new();
        for i in 0..bytes.len().saturating_sub(3) {
            if &bytes[i..i + 4] == b"blog" {
                offsets.push(i as u64);
            }
        }
        assert!(
            !offsets.is_empty(),
            "pattern bytes must appear in the archive"
        );
        use std::io::{Seek, Write};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(&manifest)
            .unwrap();
        for off in offsets {
            f.seek(std::io::SeekFrom::Start(off)).unwrap();
            f.write_all(b"bl9g").unwrap();
        }
        assert!(
            archived.match_path("/blog").is_none(),
            "old pattern must be gone from the mmap view"
        );
        assert!(
            archived.match_path("/bl9g").is_some(),
            "new pattern must be visible through the mmap"
        );
    }

    #[test]
    fn load_archived_reports_stable_error_codes() {
        let tmp = tempfile::tempdir().unwrap();
        // Missing file -> RTR-101.
        let err = RouteTree::load_archived(&tmp.path().join("nope.manifest")).unwrap_err();
        assert_eq!(err.0.code, "RTR-101");
        // Corrupt bytes -> RTR-102.
        let manifest = tmp.path().join("routes.manifest");
        std::fs::write(&manifest, b"definitely not an rkyv archive").unwrap();
        let err = RouteTree::load_archived(&manifest).unwrap_err();
        assert_eq!(err.0.code, "RTR-102");
        assert!(err.0.fix.is_some());
    }

    #[test]
    fn archive_to_writes_deterministic_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = fixture(tmp.path());
        let a = tmp.path().join("a.manifest");
        let b = tmp.path().join("b.manifest");
        tree.archive_to(&a).unwrap();
        tree.archive_to(&b).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    }
}
