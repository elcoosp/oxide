//! Module resolution (spec §6.2 step 1). `oxc_resolver` drives the resolution —
//! extensions, tsconfig `paths`, package `exports`, `browser` field, and the
//! per-environment conditions all flow through its `ResolveOptions`. This
//! module is the thin adapter between Oxide's `BundleEnv`/`BundleOptions`
//! surface and the resolver's option struct.

use super::BundleEnv;
use oxc_resolver::{Alias, AliasValue, ResolveOptions, Resolver};
use oxide_diagnostics::{Diagnostic, OxideError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

type Res<T> = std::result::Result<T, OxideError>;

/// Resolve `specifier` as seen from `from` (an absolute module path) under
/// `root`. `paths` are tsconfig-style alias patterns (`@app/*` → `src/*`).
pub fn resolve(
    root: &Path,
    specifier: &str,
    from: &Path,
    env: BundleEnv,
    paths: &BTreeMap<String, Vec<String>>,
) -> Res<PathBuf> {
    // Asset queries (`./hero.png?w=640`) resolve the path before the `?` (§6.7).
    let spec = specifier.split('?').next().unwrap_or(specifier).trim();
    if spec.is_empty() {
        return Err(unresolved(root, specifier, from));
    }

    // Non-JS asset extensions (`./card.css`, `./hero.png`) are filesystem
    // resolution, not module resolution — oxc_resolver only knows the module
    // extensions. Route them through a direct probe.
    if let Some(p) = try_asset_path(root, spec, from) {
        return Ok(p);
    }

    let options = build_options(root, env, paths);
    let resolver = Resolver::new(options);
    let from_dir = from.parent().unwrap_or(root);

    // oxc_resolver follows filesystem symlinks, which on macOS turns the
    // `tempfile` crate's `/var/folders/...` into `/private/var/folders/...`.
    // Callers compare against the un-canonicalized `root`, so strip the
    // resolver's `/private` prefix back to the caller's when the prefix
    // otherwise matches.
    resolver
        .resolve(from_dir, spec)
        .map(|r| reconcile_prefix(&r.full_path(), root))
        .map_err(|_| unresolved(root, specifier, from))
}

/// Lexically normalize `.` and `..` components without touching the filesystem.
/// Probing raw joined paths containing `..` fails on sandboxed filesystems and
/// breaks stable hashes + `strip_prefix` on real ones, so every candidate is
/// normalized before it is stat-ed or returned.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Direct filesystem probe for non-JS asset extensions. Recognised suffixes:
/// `.css`, `.png`, `.jpg`, `.jpeg`, `.svg`, `.gif`, `.webp`, `.avif`, `.woff`,
/// `.woff2`, `.ttf`, `.otf`, `.txt`, `.json`, `.wasm`, `.html`.
/// Reconcile a path returned by oxc_resolver with the caller-supplied `root`.
/// The resolver may have canonicalized macOS's `/var` → `/private/var` symlink;
/// callers compare against the un-canonicalized root, so unwrap the same
/// transformation when it's the only difference.
fn reconcile_prefix(p: &Path, root: &Path) -> PathBuf {
    let p = normalize(p);
    if p.starts_with(root) {
        return p;
    }
    // Try stripping a leading `/private` segment and re-checking.
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("/private") {
        let alt = PathBuf::from(rest);
        if alt.starts_with(root) {
            return alt;
        }
    }
    p
}

fn try_asset_path(root: &Path, spec: &str, from: &Path) -> Option<PathBuf> {
    const ASSET_EXTS: &[&str] = &[
        ".css", ".png", ".jpg", ".jpeg", ".svg", ".gif", ".webp", ".avif", ".woff", ".woff2",
        ".ttf", ".otf", ".txt", ".json", ".wasm", ".html",
    ];
    let has_asset_ext = ASSET_EXTS.iter().any(|e| spec.ends_with(e));
    if !has_asset_ext {
        return None;
    }
    let candidate = if spec.starts_with("./") || spec.starts_with("../") {
        normalize(&from.parent().unwrap_or(root).join(spec))
    } else if spec.starts_with('/') {
        normalize(&root.join(spec.trim_start_matches('/')))
    } else {
        // Bare specifier with an asset extension — look under node_modules.
        normalize(&root.join("node_modules").join(spec))
    };
    candidate.is_file().then_some(candidate)
}

fn build_options(
    root: &Path,
    env: BundleEnv,
    paths: &BTreeMap<String, Vec<String>>,
) -> ResolveOptions {
    let conditions = match env {
        BundleEnv::Client | BundleEnv::Edge => vec![
            "browser".to_string(),
            "import".to_string(),
            "module".to_string(),
            "default".to_string(),
        ],
        BundleEnv::Ssr => vec![
            "node".to_string(),
            "import".to_string(),
            "module".to_string(),
            "default".to_string(),
        ],
    };

    let extensions = vec![
        ".ts".to_string(),
        ".tsx".to_string(),
        ".js".to_string(),
        ".jsx".to_string(),
        ".mjs".to_string(),
    ];

    // tsconfig `paths` become oxc_resolver aliases. `Alias` is itself a Vec of
    // (pattern, values) pairs; a single call to `Resolver::new` therefore takes
    // the flattened form.
    let alias: Alias = paths
        .iter()
        .map(|(pattern, targets)| {
            let values: Vec<AliasValue> = targets
                .iter()
                .map(|t| target_to_alias_value(root, t))
                .collect();
            (pattern.clone(), values)
        })
        .collect();

    ResolveOptions {
        condition_names: conditions,
        extensions,
        alias,
        // `browser`-field resolution stays available for client builds; SSR
        // conditions already disambiguate server paths.
        alias_fields: Vec::new(),
        ..ResolveOptions::default()
    }
}

fn target_to_alias_value(root: &Path, target: &str) -> AliasValue {
    // A target with `*` is a substitution template; without, a direct path.
    // oxc_resolver expects `$1` for the wildcard capture, so translate.
    if target.contains('*') {
        AliasValue::from(target.replace('*', "$1"))
    } else {
        AliasValue::Path(root.join(target).to_string_lossy().into_owned())
    }
}

fn unresolved(root: &Path, specifier: &str, from: &Path) -> OxideError {
    let spec = specifier.split('?').next().unwrap_or(specifier);
    let pkg = spec.split('/').next().unwrap_or(spec);
    OxideError(
        Diagnostic::error(
            "BLD-RESOLVE",
            format!(
                "cannot resolve `{specifier}` from `{}` (root `{}`)",
                from.display(),
                root.display()
            ),
        )
        .with_fix(format!(
            "check the specifier spelling, add a tsconfig `paths` entry, or install `{pkg}` \
             (node_modules/{pkg})"
        )),
    )
}
