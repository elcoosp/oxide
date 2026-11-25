//! The F11 `releases/` layout boot (§V2-F11's store layout consumed by
//! §V2-F10's serve boot): resolve a deployed `dir:` store's `current`
//! pointer to its release prefix and materialize a dist-shaped view the
//! serve boot reads, so `oxide start --releases <STORE>` serves the
//! deployed release's bytes without a rebuild and without re-deploying.
//!
//! The store layout is the deploy engine's own contract (`crates/adapters`
//! `delta.rs`): immutable `assets/` + `_oxide/` trees at the store root
//! (content-hashed, shared across releases), every non-hashed file
//! mirrored under `releases/<build-id>/<rel>` (a self-contained prefix per
//! release), the `manifest.json` ledger, and `current` — the pointer the
//! deploy writes LAST, so a reader that resolves it never sees a
//! half-uploaded release.
//!
//! The boot leg is the F10 row's recorded remaining half ("booting the
//! pools from `dist/` + F11's `releases/<build-id>/` layout"): the serve
//! reads every dist path through `<root>/dist`, so the leg resolves the
//! pointer, materializes the merged view (release prefix + the immutable
//! store-root trees) into the store's `.oxide/releases-boot/<id>/dist`,
//! and repoints `dist/` at it through the SAME swap the §V2-F10 step-4
//! rollback leg performs — every dist read after it resolves to the
//! deployed release's bytes.
//!
//! # Errors
//!
//! Every refusal is a coded [`OxideError`]: the pointer's absence is a
//! [`DEP-POINTER`](oxide_fw_diagnostics) boot refusal (the deploy writes
//! it last; a store without one was never deployed), a corrupt pointer
//! propagates the deploy parser's own `DEP-POINTER` refusal verbatim, a
//! missing release prefix is [`DEP-RELEASE`](oxide_fw_diagnostics) (the
//! store names an id whose release tree is gone — pruned or corrupt).

use std::path::PathBuf;

use oxide_fw_adapters::{DeployStore, DirStore, IMMUTABLE_PREFIXES, POINTER_NAME};
use oxide_fw_diagnostics::OxideError;

use super::generations::SNAPSHOT_MAX_DEPTH;

/// The boot view's directory name, under the store root's `.oxide/` —
/// hidden from the deploy scan (the store is the deploy's output, not its
/// input) and off the `releases/`/immutable prefixes the deploy owns.
const BOOT_DIR: &str = ".oxide/releases-boot";

/// One resolved release boot: the build id the store's pointer names, the
/// release prefix it resolves to, and the materialized dist view the
/// serve boot reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseBoot {
    /// The build id the store's `current` pointer names (the §V2-A04
    /// 16-hex shape the deploy engine validates).
    pub build_id: String,
    /// The release prefix `releases/<build-id>/` under the store root.
    pub release_root: PathBuf,
    /// The materialized dist view (a dist-shaped merge of the release
    /// prefix and the store's immutable trees) the boot repoints `dist/`
    /// at.
    pub dist: PathBuf,
}

/// The two §V2-F10 boot sources a serve can name — a retained generation
/// (`--generation`, the step-4 rollback leg) and a deployed store release
/// (`--releases`, this module) — both repoint `dist/` before the boot, so
/// naming both is a usage conflict the command refuses instead of
/// guessing which swap wins.
#[must_use]
pub fn boot_sources_conflict(generation: Option<&str>, releases: Option<&str>) -> bool {
    generation.is_some() && releases.is_some()
}

/// Resolve the store's current release and materialize its boot view.
///
/// The pointer is read through the deploy engine's own [`DirStore`] (the
/// §V2-F11 parser's coded refusals propagate verbatim), the release
/// prefix is verified present, and the merged dist view is materialized
/// under the store's `.oxide/releases-boot/<id>/dist` — reused as-is when
/// a previous boot already materialized THIS id (build ids are content
/// digests, so same id means same bytes).
///
/// # Errors
///
/// [`DEP-POINTER`](oxide_fw_diagnostics) when the store has no `current`
/// pointer (never deployed) or a corrupt one (the parser's own refusal),
/// [`DEP-RELEASE`](oxide_fw_diagnostics) when the pointer names a build
/// id whose `releases/<id>/` prefix is missing, and the materialization's
/// [`DEP-IO`](oxide_fw_diagnostics) on a failed walk or link.
pub fn resolve_release_boot(store: &std::path::Path) -> Result<ReleaseBoot, OxideError> {
    let Some(build_id) = DirStore::new(store).read_pointer()? else {
        return Err(boot_err(
            "DEP-POINTER",
            format!(
                "the deploy store {} has no {POINTER_NAME} pointer — deploy first \
                     (the pointer is the deploy's last write)",
                store.display()
            ),
        ));
    };
    let release_root = store.join("releases").join(&build_id);
    if !release_root.is_dir() {
        return Err(boot_err(
            "DEP-RELEASE",
            format!(
                "the release prefix releases/{build_id} is missing from the deploy store {}",
                store.display()
            ),
        ));
    }
    let dist = materialize_release_dist(store, &build_id)?;
    Ok(ReleaseBoot {
        build_id,
        release_root,
        dist,
    })
}

/// Materialize the merged dist view for one release id: the release
/// prefix's files at their rel paths plus the store root's immutable
/// trees (`assets/`, `_oxide/` — the deploy stores them once at the root,
/// shared across releases).
///
/// Every entry is a symlink to its source when the filesystem allows (the
/// view is zero-copy; a page's chunk tree can be tens of thousands of
/// files) falling back to a copy per entry otherwise (the §V2-F10 step-4
/// fallback rule). A stale view for a DIFFERENT id is rebuilt; a view for
/// THIS id is reused as-is.
///
/// # Errors
///
/// [`DEP-IO`](oxide_fw_diagnostics) on a failed walk, link or copy.
pub fn materialize_release_dist(
    store: &std::path::Path,
    build_id: &str,
) -> Result<PathBuf, OxideError> {
    let view = store.join(BOOT_DIR).join(build_id).join("dist");
    // The reuse arm: a view for THIS id is a content digest's twin — same
    // id means same bytes, so a previous boot's view serves as-is. The
    // build-id file inside the view is the identity check (the release
    // prefix's own non-hashed copy, mirrored into the view).
    if oxide_fw_cache::read_build_id(&view).as_deref() == Some(build_id) {
        return Ok(view);
    }
    // A stale view (a previous boot of a DIFFERENT id, or a half-written
    // one) is rebuilt from the store's current bytes.
    if view.exists() {
        std::fs::remove_dir_all(&view)
            .map_err(|e| io_err("DEP-IO", format!("cannot clear {}: {e}", view.display())))?;
    }
    // The release prefix's files at their rel paths (the non-hashed dist
    // shape the deploy mirrors per release).
    let release_root = store.join("releases").join(build_id);
    link_tree(&release_root, &view, 0)?;
    // The store root's immutable trees (`assets/`, `_oxide/` — the deploy
    // stores them once at the root, shared across releases).
    for prefix in IMMUTABLE_PREFIXES {
        let src = store.join(prefix);
        if src.is_dir() {
            link_tree(&src, &view.join(prefix), 0)?;
        }
    }
    Ok(view)
}

/// Link every file under `src` to the same rel path under `dest` — a
/// symlink per file (the zero-copy view), falling back to a copy per
/// file when the filesystem refuses (the §V2-F10 step-4 fallback rule).
/// Depth-guarded like the generation snapshot's walk.
fn link_tree(
    src: &std::path::Path,
    dest: &std::path::Path,
    depth: usize,
) -> Result<(), OxideError> {
    if depth > SNAPSHOT_MAX_DEPTH {
        return Err(io_err(
            "DEP-IO",
            format!(
                "release view deeper than {SNAPSHOT_MAX_DEPTH} at {}",
                src.display()
            ),
        ));
    }
    std::fs::create_dir_all(dest)
        .map_err(|e| io_err("DEP-IO", format!("mkdir {}: {e}", dest.display())))?;
    let entries = std::fs::read_dir(src)
        .map_err(|e| io_err("DEP-IO", format!("read_dir {}: {e}", src.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err("DEP-IO", format!("dir entry: {e}")))?;
        let p = entry.path();
        let target = dest.join(entry.file_name());
        let ft = entry
            .file_type()
            .map_err(|e| io_err("DEP-IO", format!("file_type {}: {e}", p.display())))?;
        if ft.is_dir() {
            link_tree(&p, &target, depth + 1)?;
        } else if ft.is_file() {
            link_file(&p, &target)?;
        }
        // Symlinks and other entries are skipped: a release's dist files
        // are regular files the deploy uploaded; a linked tree inside a
        // store is not a release byte.
    }
    Ok(())
}

/// One view entry: symlink first, copy on refusal (a filesystem without
/// symlinks, or a cross-device refusal) — the view is a read model, so
/// either shape serves.
fn link_file(src: &std::path::Path, dest: &std::path::Path) -> Result<(), OxideError> {
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(src, dest).is_ok() {
            return Ok(());
        }
    }
    std::fs::copy(src, dest)
        .map(|_| ())
        .map_err(|e| io_err("DEP-IO", format!("link {}: {e}", dest.display())))
}

/// The view refusals' coded constructor — the `DEP-*` deploy registry
/// class (the boot leg refuses over the deploy engine's store layout);
/// the same constructor [`boot_err`] uses.
fn io_err(code: &'static str, msg: impl Into<String>) -> OxideError {
    boot_err(code, msg)
}

/// The boot view's coded constructor — the `DEP-*` deploy registry class
/// (this leg refuses over the deploy engine's store layout).
fn boot_err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(oxide_fw_diagnostics::Diagnostic::error(code, msg)))
}

#[cfg(test)]
mod tests {
    use super::*;

    use oxide_fw_adapters::DeployStore;

    /// A store laid out exactly as the deploy engine's `dir:` target
    /// writes it: the pointer, one release prefix (the non-hashed files),
    /// and the immutable trees at the root.
    fn seed_store(root: &std::path::Path, id: &str) {
        std::fs::create_dir_all(root.join("releases").join(id).join("prerendered"))
            .expect("mkdir release");
        std::fs::write(
            root.join("releases").join(id).join("build-id"),
            format!("{id}\n"),
        )
        .expect("write build-id");
        std::fs::write(
            root.join("releases")
                .join(id)
                .join("prerendered/index.html"),
            format!("<html>release {id}</html>"),
        )
        .expect("write html");
        std::fs::create_dir_all(root.join("assets")).expect("mkdir assets");
        std::fs::write(root.join("assets/app-1a2b.js"), b"export const v = 1;\n")
            .expect("write asset");
        std::fs::create_dir_all(root.join("_oxide/manifest")).expect("mkdir _oxide");
        std::fs::write(root.join("_oxide/manifest/x.json"), b"{}").expect("write manifest");
        DirStore::new(root)
            .put_pointer(id)
            .expect("deploy writes the pointer last");
    }

    #[test]
    fn missing_pointer_refuses_the_boot() {
        let tmp = tempfile::tempdir().expect("tmp");
        let e = resolve_release_boot(tmp.path()).expect_err("an undeployed store refuses");
        assert_eq!(e.0.code, "DEP-POINTER");
        assert!(
            e.0.message.contains("no current pointer"),
            "the refusal names the pointer's absence: {}",
            e.0.message
        );
    }

    #[test]
    fn corrupt_pointer_carries_the_parser_refusal() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join(POINTER_NAME), "not-a-build-id\n").expect("write pointer");
        let direct = DirStore::new(tmp.path())
            .read_pointer()
            .expect_err("the parser refuses a corrupt pointer");
        let e = resolve_release_boot(tmp.path()).expect_err("the boot propagates it");
        assert_eq!(e.0.code, direct.0.code);
        assert_eq!(e.0.message, direct.0.message, "verbatim, not re-worded");
    }

    #[test]
    fn missing_release_prefix_refuses() {
        let tmp = tempfile::tempdir().expect("tmp");
        // A pointer naming an id whose release tree was pruned: the boot
        // refuses instead of materializing an empty dist.
        DirStore::new(tmp.path())
            .put_pointer("0123456789abcdef")
            .expect("pointer");
        let e = resolve_release_boot(tmp.path()).expect_err("a pruned release refuses");
        assert_eq!(e.0.code, "DEP-RELEASE");
        assert!(
            e.0.message.contains("releases/0123456789abcdef"),
            "the refusal names the missing prefix: {}",
            e.0.message
        );
    }

    #[test]
    fn materializes_the_release_view() {
        let tmp = tempfile::tempdir().expect("tmp");
        let id = "0123456789abcdef";
        seed_store(tmp.path(), id);
        let boot = resolve_release_boot(tmp.path()).expect("boot");
        assert_eq!(boot.build_id, id);
        assert_eq!(
            oxide_fw_cache::read_build_id(&boot.dist).as_deref(),
            Some(id),
            "the view carries the release's build id"
        );
        let html = std::fs::read_to_string(boot.dist.join("prerendered/index.html"))
            .expect("the release prefix rides the view");
        assert_eq!(html, "<html>release 0123456789abcdef</html>");
        let asset = std::fs::read(boot.dist.join("assets/app-1a2b.js"))
            .expect("the store-root immutable tree rides the view");
        assert_eq!(asset, b"export const v = 1;\n");
        std::fs::read(boot.dist.join("_oxide/manifest/x.json"))
            .expect("the second immutable tree rides the view");
    }

    #[test]
    fn materialization_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tmp");
        let id = "0123456789abcdef";
        seed_store(tmp.path(), id);
        let first = materialize_release_dist(tmp.path(), id).expect("first");
        let second = materialize_release_dist(tmp.path(), id).expect("second");
        assert_eq!(first, second, "same id, same view");
        let html = std::fs::read_to_string(first.join("prerendered/index.html"))
            .expect("the reused view still reads");
        assert!(html.contains(id));
    }

    #[test]
    fn a_moved_pointer_materializes_the_new_release() {
        let tmp = tempfile::tempdir().expect("tmp");
        let a = "0123456789abcdef";
        let b = "fedcba9876543210";
        seed_store(tmp.path(), a);
        resolve_release_boot(tmp.path()).expect("boot A");
        // A second deploy: release B's prefix + the pointer swap (the
        // deploy's last write).
        seed_store(tmp.path(), b);
        let boot = resolve_release_boot(tmp.path()).expect("boot B");
        assert_eq!(boot.build_id, b);
        assert_eq!(
            oxide_fw_cache::read_build_id(&boot.dist).as_deref(),
            Some(b),
            "the boot view follows the pointer"
        );
    }

    #[test]
    fn boot_sources_conflict_refuses_both_swaps() {
        assert!(boot_sources_conflict(Some("previous"), Some("/srv/store")));
        assert!(!boot_sources_conflict(Some("previous"), None));
        assert!(!boot_sources_conflict(None, Some("/srv/store")));
        assert!(!boot_sources_conflict(None, None));
    }
}
