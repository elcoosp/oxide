//! §5.1 early cutoff: a project fingerprint so an unchanged `oxide build`
//! skips the whole pipeline.
//!
//! Every `oxide build` re-runs precompile plus both bundler environments,
//! whether or not any source changed — the §21 benchmark records warm == cold
//! (a warm React build measured ~3.3 s against a cold ~3.1 s, all of it
//! precompile + bundle client + bundle server). The long-term answer is the
//! graph engine plus CAS in `oxide-fw-graph` driving the bundler node by node
//! (§5.1 Tier A, §5.4 Tier B). This module is the bounded first step: **skip
//! the build entirely when no tracked input changed and the previous output is
//! still on disk.**
//!
//! The fingerprint is a blake3 over the running binary (so a rebuilt CLI
//! invalidates), the CLI mode, the `--static` flag, and every project file
//! outside the output and vendor directories. It is stored at
//! `<root>/.oxide/cache/build-fingerprint`.
//!
//! The skip is conservative: it fires only when both build manifests exist, so
//! a deleted `dist/` forces a full rebuild even on a fingerprint match. Per-node
//! cutoff (re-bundle only the changed chunk) is the follow-up and needs the
//! bundler driven by the graph.

use std::path::{Path, PathBuf};

/// Directories never walked: build output, caches, vendor trees. A change
/// under one of these is never a source change.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    ".oxide",
    ".git",
    "target",
    ".cache",
    ".turbo",
];

/// Files at the project root that are build artifacts, not inputs. The bundler
/// synthesises `.oxide-tsconfig.json` when a config declares `tsconfig` paths.
const SKIP_FILES: &[&str] = &[".oxide-tsconfig.json"];

/// Fingerprint every tracked input for a build of `root` in `mode`.
pub fn fingerprint(root: &Path, mode: &str, static_export: bool) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"oxide-build-fingerprint-v1");
    h.update(mode.as_bytes());
    h.update(&[static_export as u8]);

    // A rebuilt CLI must invalidate the cache. Hashing the ~140 MB binary is
    // too expensive, so size + mtime of the running executable stands in.
    if let Ok(exe) = std::env::current_exe()
        && let Ok(md) = std::fs::metadata(&exe)
    {
        h.update(&md.len().to_le_bytes());
        if let Ok(t) = md.modified()
            && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
        {
            h.update(&d.as_secs().to_le_bytes());
            h.update(&d.subsec_nanos().to_le_bytes());
        }
    }

    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, &mut files);
    files.sort();
    // P-22: read + hash in parallel; `par_iter().map().collect()` over a
    // sorted `Vec` preserves index order, so the digest is byte-identical
    // to the sequential walk while the hashing scales across cores.
    use rayon::prelude::*;
    let mut hashed: Vec<(&Path, Option<blake3::Hash>)> = files
        .par_iter()
        .map(|f| {
            (
                f.strip_prefix(root).unwrap_or(f),
                std::fs::read(f).ok().map(|b| blake3::hash(&b)),
            )
        })
        .collect();
    hashed.sort_by(|a, b| a.0.cmp(b.0));
    for (rel, digest) in hashed {
        h.update(rel.to_string_lossy().as_bytes());
        h.update(&[0]);
        if let Some(d) = digest {
            h.update(d.as_bytes());
        }
        h.update(&[0]);
    }
    h.finalize().to_hex().to_string()
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        // `file_type` does not follow symlinks, so a symlink loop cannot hang
        // the walk; symlinked files and dirs are simply not tracked.
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            collect_files(&p, out);
        } else if ft.is_file() {
            if SKIP_FILES.contains(&name.as_ref()) {
                continue;
            }
            // §6.2: the worker writes compiled-server-module scratch files
            // under `<root>/.oxide/transform/` (hash-named `.ts`, no `.oxide`
            // suffix on the file itself). `.oxide` is already in SKIP_DIRS
            // below, so this branch is dead for current writers; it stays as
            // defense against residue from the historical writer (which wrote
            // `.<base>.<hash>.oxide.ts` next to the source). Both shapes are
            // build artifacts, not inputs.
            if name.ends_with(".oxide.ts") {
                continue;
            }
            out.push(p);
        }
    }
}

fn fingerprint_path(root: &Path) -> PathBuf {
    root.join(".oxide/cache/build-fingerprint")
}

/// True when the stored fingerprint for `root` equals `fp`.
pub fn is_up_to_date(root: &Path, fp: &str) -> bool {
    std::fs::read_to_string(fingerprint_path(root))
        .map(|s| s.trim() == fp)
        .unwrap_or(false)
}

/// Persist `fp` for the next build.
pub fn store(root: &Path, fp: &str) -> std::io::Result<()> {
    let p = fingerprint_path(root);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(p, fp)
}

/// A per-environment fingerprint: the transitive content of everything that
/// env's bundle depends on, plus the options that shape it. Distinct from
/// [`fingerprint`], which covers the whole project and decides whether to
/// skip the *entire* build.
///
/// The granularity is one environment (client or ssr) — not one chunk. That
/// is the incrementality Rolldown's API allows: it exposes no per-module
/// incremental build, so the smallest honest unit is "this env's bundle is
/// still valid". Editing a server-only module then skips the client bundle,
/// which is the common case a full-rebuild-on-any-edit gets wrong.
pub fn env_fingerprint(
    root: &Path,
    env: &str,
    entries: &[(String, PathBuf)],
    extra: &[&str],
) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"oxide-env-fingerprint-v1");
    h.update(env.as_bytes());
    h.update(&[0]);
    for e in extra {
        h.update(e.as_bytes());
        h.update(&[0]);
    }
    // The running binary invalidates (a compiler change changes output).
    if let Ok(exe) = std::env::current_exe()
        && let Ok(md) = std::fs::metadata(&exe)
    {
        h.update(&md.len().to_le_bytes());
        if let Ok(t) = md.modified()
            && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
        {
            h.update(&d.as_secs().to_le_bytes());
        }
    }
    h.update(&[0]);
    // Entry set, sorted for determinism.
    let mut sorted: Vec<&(String, PathBuf)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path) in sorted {
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(path.to_string_lossy().as_bytes());
        h.update(&[0]);
    }
    // Every file under the project that this env's bundle can reach. The
    // one distinction the bundler itself makes: a `*.server.*` module is
    // never in the client bundle (§6.4 guard). Editing `page.server.ts`
    // therefore invalidates the SSR env and leaves the client env valid —
    // which is the incrementality a whole-project fingerprint misses.
    //
    // A finer cut (per-module import graph) would need the bundler to expose
    // its graph; Rolldown's API does not, so this is the tightest honest
    // boundary.
    let client_env = env == "client";
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, &mut files);
    files.sort();
    for f in &files {
        if client_env {
            let s = f.to_string_lossy();
            if oxide_fw_compiler::is_server_module_path(&s) {
                continue;
            }
        }
        let rel = f.strip_prefix(root).unwrap_or(f);
        h.update(rel.to_string_lossy().as_bytes());
        h.update(&[0]);
        if let Ok(bytes) = std::fs::read(f) {
            h.update(blake3::hash(&bytes).as_bytes());
        }
        h.update(&[0]);
    }
    h.finalize().to_hex().to_string()
}

fn env_fingerprint_path(root: &Path, env: &str) -> PathBuf {
    root.join(format!(".oxide/cache/env-{env}-fingerprint"))
}

/// True when the stored per-env fingerprint equals `fp`.
pub fn env_is_up_to_date(root: &Path, env: &str, fp: &str) -> bool {
    std::fs::read_to_string(env_fingerprint_path(root, env))
        .map(|s| s.trim() == fp)
        .unwrap_or(false)
}

/// Persist a per-env fingerprint.
pub fn env_store(root: &Path, env: &str, fp: &str) -> std::io::Result<()> {
    let p = env_fingerprint_path(root, env);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(p, fp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn fingerprint_is_stable_and_content_sensitive() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.ts", "export const a = 1;\n");
        write(dir.path(), "oxide.config.ts", "export default {};\n");

        let a = fingerprint(dir.path(), "production", false);
        let b = fingerprint(dir.path(), "production", false);
        assert_eq!(a, b, "same inputs must fingerprint identically");

        write(dir.path(), "src/a.ts", "export const a = 2;\n");
        let c = fingerprint(dir.path(), "production", false);
        assert_ne!(a, c, "a changed source file must change the fingerprint");
    }

    #[test]
    fn fingerprint_ignores_output_and_vendor_dirs() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.ts", "export const a = 1;\n");
        let base = fingerprint(dir.path(), "production", false);

        // Writes under skipped dirs must NOT change the fingerprint.
        write(dir.path(), "dist/client/x.js", "junk");
        write(dir.path(), "node_modules/pkg/index.js", "junk");
        write(dir.path(), ".oxide/cache/blob", "junk");
        let after = fingerprint(dir.path(), "production", false);
        assert_eq!(base, after, "output/vendor writes must not invalidate");

        // A real source write DOES change it.
        write(dir.path(), "src/b.ts", "export const b = 1;\n");
        let real = fingerprint(dir.path(), "production", false);
        assert_ne!(base, real);
    }

    #[test]
    fn fingerprint_tracks_mode_and_static_flag() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.ts", "export const a = 1;\n");
        let prod = fingerprint(dir.path(), "production", false);
        let dev = fingerprint(dir.path(), "development", false);
        let stat = fingerprint(dir.path(), "production", true);
        assert_ne!(prod, dev);
        assert_ne!(prod, stat);
    }

    #[test]
    fn store_and_is_up_to_date_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.ts", "export const a = 1;\n");
        let fp = fingerprint(dir.path(), "production", false);

        assert!(!is_up_to_date(dir.path(), &fp), "nothing stored yet");
        store(dir.path(), &fp).unwrap();
        assert!(is_up_to_date(dir.path(), &fp), "stored fp matches");

        write(dir.path(), "src/a.ts", "export const a = 2;\n");
        let fp2 = fingerprint(dir.path(), "production", false);
        assert!(!is_up_to_date(dir.path(), &fp2), "changed input mismatches");
    }

    #[test]
    fn oxide_ts_temp_files_do_not_change_the_fingerprint() {
        // The worker writes `<name>.<hash>.oxide.ts` (originally next to the
        // source; now a hash-named `.ts` under `<root>/.oxide/transform/`,
        // excluded by SKIP_DIRS). Both shapes are build artifacts; if the
        // fingerprint counted them, every compile would invalidate the next
        // build.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/page.server.ts", "export const load = 1;\n");
        let before = fingerprint(dir.path(), "production", false);

        write(
            dir.path(),
            "src/.page.server.ts.deadbeef.oxide.ts",
            "// compiled temp",
        );
        let after = fingerprint(dir.path(), "production", false);
        assert_eq!(
            before, after,
            "a *.oxide.ts temp must not change the fingerprint"
        );

        // A real source edit still does.
        write(dir.path(), "src/page.server.ts", "export const load = 2;\n");
        let edited = fingerprint(dir.path(), "production", false);
        assert_ne!(before, edited);
    }
}
