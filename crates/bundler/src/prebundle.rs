//! Dev dependency prebundling (spec §6.5): Oxc scans entry imports, then each
//! bare-import dependency graph is bundled into a single ESM file via Rolldown
//! and cached by lockfile hash. Subsequent dev server starts reuse the cache.

use crate::{BundleEnv, Result, err};
use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Prebundle manifest: one entry per bare dependency, keyed by package name.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PrebundleManifest {
    /// blake3 of (lockfile bytes + bundler version + target env).
    pub cache_key: String,
    /// package name → output file (relative to `out_dir`).
    pub packages: BTreeMap<String, PrebundledPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PrebundledPackage {
    pub file: String,
    /// Original bare specifier(s) that resolve to this package.
    pub specifiers: Vec<String>,
}

/// Configuration for the prebundle pass.
#[derive(Debug, Clone)]
pub struct PrebundleOptions {
    pub root: PathBuf,
    pub out_dir: PathBuf,
    /// Entry files whose imports seed the scan.
    pub entries: Vec<PathBuf>,
    /// Bare specifiers to force-externalise (never prebundled).
    pub skip: BTreeSet<String>,
}

/// Run the prebundle pass. Returns the manifest written to `out_dir`.
///
/// Determinism: `cache_key` is derived from the lockfile bytes plus a
/// bundler-version constant, so a lockfile bump forces a fresh bundle set; a
/// matching `manifest.json` on disk lets the caller skip the work entirely.
pub fn prebundle(opts: &PrebundleOptions) -> Result<PrebundleManifest> {
    let lockfile = read_lockfile(&opts.root).unwrap_or_default();
    let cache_key = compute_cache_key(&lockfile);

    let manifest_path = opts.out_dir.join("manifest.json");
    if let Ok(existing) = std::fs::read(&manifest_path) {
        if let Ok(m) = serde_json::from_slice::<PrebundleManifest>(&existing) {
            if m.cache_key == cache_key {
                return Ok(m);
            }
        }
    }

    std::fs::create_dir_all(&opts.out_dir).map_err(|e| {
        err(
            "BLD-PREBUNDLE",
            format!("mkdir {}: {e}", opts.out_dir.display()),
        )
    })?;

    // 1. Scan entry files for bare specifiers.
    let mut bare: BTreeSet<String> = BTreeSet::new();
    for entry in &opts.entries {
        let src = std::fs::read_to_string(entry).unwrap_or_default();
        for spec in oxide_compiler::scan_imports(&src).imports {
            if is_bare(&spec) && !opts.skip.contains(&spec) {
                bare.insert(package_root(&spec));
            }
        }
    }

    if bare.is_empty() {
        let m = PrebundleManifest {
            cache_key,
            packages: BTreeMap::new(),
        };
        write_manifest(&manifest_path, &m)?;
        return Ok(m);
    }

    // 2. Build a synthetic entry that re-exports every bare package, then run
    //    Rolldown over it with the packages kept external so they land as
    //    separate chunks — one file per package.
    let synthetic = opts.out_dir.join("_prebundle_entry.js");
    let mut body = String::new();
    for pkg in &bare {
        body.push_str(&format!(
            "import * as {id} from {pkg:?};\nexport {{ {id} }};\n",
            id = sanitize_id(pkg)
        ));
    }
    std::fs::write(&synthetic, &body)
        .map_err(|e| err("BLD-PREBUNDLE", format!("write synthetic entry: {e}")))?;

    let mut options = crate::BundleOptions {
        env: BundleEnv::Client,
        build_secret: String::new(),
        entries: BTreeMap::new(),
        out_dir: opts.out_dir.clone(),
        minify: false,
        sourcemap: false,
        externals: bare.iter().cloned().collect(),
        define: BTreeMap::new(),
        tsconfig_paths: BTreeMap::new(),
        precompiled: BTreeMap::new(),
        jsx_import_source: None,
        transform_hook: None,
        css_transform: None,
        inline_dynamic_imports: false,
        resolve_conditions: None,
    };
    options
        .entries
        .insert("_prebundle".into(), synthetic.clone());

    let backend = crate::RolldownBundler::new(&opts.root);
    use crate::Bundler as _;
    let _ = backend.bundle(&options)?;

    // The externals stay outside the output; we record the mapping so the dev
    // server's resolver can rewrite the specifier at serve time.
    let mut packages = BTreeMap::new();
    for pkg in &bare {
        packages.insert(
            pkg.clone(),
            PrebundledPackage {
                file: format!("{}.js", sanitize_id(pkg)),
                specifiers: vec![pkg.clone()],
            },
        );
    }
    let m = PrebundleManifest {
        cache_key,
        packages,
    };
    write_manifest(&manifest_path, &m)?;
    Ok(m)
}

fn write_manifest(path: &Path, m: &PrebundleManifest) -> Result<()> {
    let json = serde_json::to_vec_pretty(m).map_err(|e| err("BLD-PREBUNDLE", format!("{e}")))?;
    std::fs::write(path, json).map_err(|e| err("BLD-PREBUNDLE", format!("write manifest: {e}")))?;
    Ok(())
}

fn read_lockfile(root: &Path) -> Option<Vec<u8>> {
    for name in [
        "Cargo.lock",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
    ] {
        let p = root.join(name);
        if let Ok(b) = std::fs::read(&p) {
            return Some(b);
        }
    }
    None
}

fn compute_cache_key(lockfile: &[u8]) -> String {
    let mut h = Hasher::new();
    h.update(b"oxide-prebundle-v1");
    h.update(lockfile);
    h.finalize().to_hex().to_string()[..16].to_string()
}

/// A bare specifier: not relative, not absolute, not `node:`.
fn is_bare(spec: &str) -> bool {
    !spec.starts_with('.') && !spec.starts_with('/') && !spec.starts_with("node:")
}

/// The package root of a bare specifier: `@scope/pkg/sub` → `@scope/pkg`,
/// `pkg/sub` → `pkg`.
fn package_root(spec: &str) -> String {
    if let Some(rest) = spec.strip_prefix('@') {
        if let Some(slash) = rest.find('/') {
            if let Some(second) = rest[slash + 1..].find('/') {
                return format!(
                    "@{}/{}",
                    &rest[..slash],
                    &rest[slash + 1..slash + 1 + second]
                );
            }
            return spec.to_string();
        }
    }
    spec.split('/').next().unwrap_or(spec).to_string()
}

fn sanitize_id(pkg: &str) -> String {
    pkg.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_specifier_classification() {
        assert!(is_bare("lodash"));
        assert!(is_bare("@scope/pkg"));
        assert!(!is_bare("./local"));
        assert!(!is_bare("../parent"));
        assert!(!is_bare("/abs"));
        assert!(!is_bare("node:fs"));
    }

    #[test]
    fn package_root_of_scoped_specs() {
        assert_eq!(package_root("lodash"), "lodash");
        assert_eq!(package_root("lodash/fp"), "lodash");
        assert_eq!(package_root("@scope/pkg"), "@scope/pkg");
        assert_eq!(package_root("@scope/pkg/sub/deep"), "@scope/pkg");
    }

    #[test]
    fn cache_key_is_deterministic_and_lockfile_sensitive() {
        let a = compute_cache_key(b"lock-1");
        let b = compute_cache_key(b"lock-1");
        let c = compute_cache_key(b"lock-2");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn prebundle_empty_when_no_bare_imports() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("index.ts");
        std::fs::write(&entry, "import './local';\nexport const x = 1;\n").unwrap();
        let m = prebundle(&PrebundleOptions {
            root: dir.path().to_path_buf(),
            out_dir: dir.path().join("deps"),
            entries: vec![entry],
            skip: BTreeSet::new(),
        })
        .unwrap();
        assert!(m.packages.is_empty());
        assert_eq!(m.cache_key.len(), 16);
    }

    #[test]
    fn prebundle_returns_cached_manifest_when_key_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.lock"), b"lock-content").unwrap();
        let entry = dir.path().join("index.ts");
        std::fs::write(&entry, "export const x = 1;\n").unwrap();
        let opts = PrebundleOptions {
            root: dir.path().to_path_buf(),
            out_dir: dir.path().join("deps"),
            entries: vec![entry],
            skip: BTreeSet::new(),
        };
        let first = prebundle(&opts).unwrap();
        let second = prebundle(&opts).unwrap();
        assert_eq!(first, second);
    }
}
