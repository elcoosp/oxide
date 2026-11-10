//! The V2-A07 prerender error-status sidecar (spec §V2-A07).
//!
//! A prerendered page whose render status is not 200 must not be served as
//! 200. The build writes `<route>/index.html.status` containing the decimal
//! status next to the page (`write_status_sidecar`, the write half of the
//! contract); at serve start the dist is scanned ONCE into
//! [`StatusSidecars`] — no per-request I/O — and the static-asset responder
//! ([`crate::StaticAssets::serve`]) applies the status to the page it
//! serves. The L0 seed already carries the status for the seeded route;
//! the sidecar covers every other disk-served path (a fresh `oxide start`
//! with a cold L0, and the non-default-locale pages the seed skips).
//!
//! The reader is deliberately forgiving: malformed content (non-numeric,
//! empty, out of `u16` range) is ignored and the page keeps the default
//! 200 — a corrupted sidecar must not turn a good build into a wall of
//! errors, it must fail toward the pre-A07 behavior.
//!
//! Sidecar files are build-internal: they never enter the asset index (the
//! walk skips `*.status`) and [`crate::StaticAssets::serve`] additionally
//! hard-denies them — the SEC-STATIC defense-in-depth shape.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The statuses a serve-time scan found (§V2-A07): file → HTTP status.
///
/// Keys are the PAGE files (the sidecar's path with the `.status` suffix
/// stripped, `index.html.status` → `index.html`), so a responder can ask
/// for the file it is about to serve.
#[derive(Debug, Default, Clone)]
pub struct StatusSidecars(std::collections::HashMap<PathBuf, u16>);

impl StatusSidecars {
    /// Walk `root` recursively once and index every `*.status` sidecar (the
    /// spec's scan: a stack walk, hidden entries skipped, malformed content
    /// ignored). A missing root yields an empty map — `oxide start` on a
    /// half-built dist must not refuse to boot over a missing prerendered
    /// tree.
    #[must_use]
    pub fn scan(root: &Path) -> Self {
        let _span = tracing::debug_span!("status_sidecars_scan", root = %root.display()).entered();
        let mut m: HashMap<PathBuf, u16> = HashMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if !is_status_sidecar(&p) {
                    continue;
                }
                if let Ok(s) = std::fs::read_to_string(&p)
                    && let Some(code) = parse_status(&s)
                {
                    m.insert(p.with_extension(""), code);
                }
            }
        }
        tracing::debug!(sidecars = m.len(), "status_sidecars_scan: scanned");
        Self(m)
    }

    /// The status a sidecar prescribes for `file` (`None` = the default
    /// 200 — no sidecar, or malformed content, or a different file).
    #[must_use]
    pub fn status_for(&self, file: &Path) -> Option<u16> {
        self.0.get(file).copied()
    }

    /// Number of sidecars the scan indexed (observability / tests).
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// True when no sidecar was indexed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// True when `path` is a sidecar file: its extension is exactly `status`
/// (`index.html.status`, `boom.html.status`). A file whose name merely ends
/// in a longer dotted suffix (`.status.bak`, `.status2`) is not one — and
/// stays visible to the asset walk.
#[must_use]
pub fn is_status_sidecar(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("status")
}

/// Parse sidecar content into a status: trimmed decimal, `u16` range
/// (`"500"`, `"503\n"` ok; `""`, `"abc"`, `"70000"` are None).
#[must_use]
pub fn parse_status(content: &str) -> Option<u16> {
    content.trim().parse::<u16>().ok()
}

/// The sidecar path for a page file: the `.status` suffix appended
/// (`prerendered/index.html` → `prerendered/index.html.status`).
#[must_use]
pub fn sidecar_path_for(file: &Path) -> PathBuf {
    let mut os = file.to_path_buf().into_os_string();
    os.push(".status");
    PathBuf::from(os)
}

/// The write half of the contract, called by the build's prerender sites
/// (`write_prerender` and the incremental-reuse branch):
///
/// - `status != 200` → write `<file>.status` containing the decimal status.
/// - `status == 200` → REMOVE a stale `<file>.status` so a page that
///   returned to 200 does not keep serving the previous build's error
///   status (the incremental-reuse path writes fresh HTML over the old
///   page; the sidecar must not outlive it).
///
/// I/O errors propagate — the caller decides whether a build can continue.
pub fn write_status_sidecar(file: &Path, status: u16) -> std::io::Result<()> {
    let sidecar = sidecar_path_for(file);
    if status == 200 {
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {
                tracing::debug!(sidecar = %sidecar.display(), "status_sidecar: cleared");
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    } else {
        if let Some(parent) = sidecar.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&sidecar, status.to_string())?;
        tracing::debug!(
            sidecar = %sidecar.display(),
            status,
            "status_sidecar: written"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_appends_the_suffix() {
        assert_eq!(
            sidecar_path_for(Path::new("dist/prerendered/index.html")),
            PathBuf::from("dist/prerendered/index.html.status")
        );
        assert_eq!(
            sidecar_path_for(Path::new("a/b.html")),
            PathBuf::from("a/b.html.status")
        );
    }

    #[test]
    fn is_status_sidecar_needs_the_exact_extension() {
        assert!(is_status_sidecar(Path::new("a/index.html.status")));
        assert!(!is_status_sidecar(Path::new("a/index.html")));
        assert!(!is_status_sidecar(Path::new("a/index.html.status.bak")));
        assert!(!is_status_sidecar(Path::new("a/index.html.status2")));
        assert!(!is_status_sidecar(Path::new("a/status")));
    }

    #[test]
    fn parse_status_is_trimmed_and_ranged() {
        assert_eq!(parse_status("500"), Some(500));
        assert_eq!(parse_status("503\n"), Some(503));
        assert_eq!(parse_status(" 404 "), Some(404));
        assert_eq!(parse_status(""), None);
        assert_eq!(parse_status("abc"), None);
        assert_eq!(parse_status("70000"), None);
        assert_eq!(parse_status("-1"), None);
    }

    #[test]
    fn scan_maps_the_sidecar_and_ignores_malformed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let html = tmp.path().join("p/boom.html");
        std::fs::create_dir_all(tmp.path().join("p")).expect("mkdir");
        std::fs::write(&html, "x").expect("html");
        std::fs::write(sidecar_path_for(&html), "500").expect("sidecar");
        let bad = tmp.path().join("p/bad.html.status");
        std::fs::write(&bad, "not-a-number").expect("bad sidecar");
        let sidecars = StatusSidecars::scan(tmp.path());
        assert_eq!(sidecars.status_for(&html), Some(500));
        assert_eq!(sidecars.status_for(&tmp.path().join("p/bad.html")), None);
        assert_eq!(sidecars.len(), 1, "the malformed sidecar is not indexed");
    }

    #[test]
    fn write_then_scan_round_trips_and_clears() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let html = tmp.path().join("p/x.html");
        std::fs::create_dir_all(tmp.path().join("p")).expect("mkdir");
        std::fs::write(&html, "x").expect("html");
        write_status_sidecar(&html, 404).expect("write");
        assert_eq!(
            std::fs::read_to_string(sidecar_path_for(&html)).expect("sidecar"),
            "404"
        );
        assert_eq!(
            StatusSidecars::scan(tmp.path()).status_for(&html),
            Some(404)
        );
        write_status_sidecar(&html, 200).expect("clear");
        assert!(!sidecar_path_for(&html).exists());
        assert_eq!(StatusSidecars::scan(tmp.path()).status_for(&html), None);
    }

    #[test]
    fn clearing_a_missing_sidecar_is_a_no_op() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let html = tmp.path().join("never-written.html");
        write_status_sidecar(&html, 200).expect("removing a missing sidecar is not an error");
    }
}
