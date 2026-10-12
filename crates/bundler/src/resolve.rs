//! Module resolution (spec §6.2 step 1): relative specifiers with extension
//! probing, tsconfig `paths` (longest prefix wins), and node_modules package
//! `exports` with per-environment conditions, falling back to `module`/`main`.

use super::BundleEnv;
use oxide_diagnostics::{Diagnostic, OxideError};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

type Res<T> = std::result::Result<T, OxideError>;

/// Candidate extensions for extensionless specifiers (§6.2 step 1).
const EXTS: [&str; 5] = ["ts", "tsx", "js", "jsx", "mjs"];

/// Resolve `specifier` as seen from `from` (an absolute module path) under `root`.
pub fn resolve(
    root: &Path,
    specifier: &str,
    from: &Path,
    env: BundleEnv,
    paths: &BTreeMap<String, Vec<String>>,
) -> Res<PathBuf> {
    // Asset queries (`./hero.png?w=640`) resolve to the path before the `?` (§6.7).
    let spec = specifier.split('?').next().unwrap_or(specifier).trim();
    if spec.is_empty() {
        return Err(unresolved(root, specifier, from));
    }
    // 1. relative
    if spec.starts_with("./") || spec.starts_with("../") {
        let base = from.parent().unwrap_or(Path::new(".")).join(spec);
        return try_file(&base).ok_or_else(|| unresolved(root, specifier, from));
    }
    // 2. tsconfig paths — longest prefix first, ties by pattern for determinism
    let mut pats: Vec<(&String, &Vec<String>)> = paths.iter().collect();
    pats.sort_by(|a, b| {
        prefix_len(b.0)
            .cmp(&prefix_len(a.0))
            .then_with(|| a.0.cmp(b.0))
    });
    for (pat, targets) in pats {
        if let Some(sub) = match_pattern(pat, spec) {
            for t in targets {
                let cand = root.join(t.replace('*', &sub));
                if let Some(p) = try_file(&cand) {
                    return Ok(p);
                }
            }
        }
    }
    // 3. node_modules (bare specifiers)
    if let Some(p) = resolve_package(root, spec, env) {
        return Ok(p);
    }
    Err(unresolved(root, specifier, from))
}

fn prefix_len(pat: &str) -> usize {
    pat.find('*').unwrap_or(pat.len())
}

/// Match `spec` against a tsconfig pattern (`@app/*`); returns the `*` substitution.
fn match_pattern(pat: &str, spec: &str) -> Option<String> {
    match pat.find('*') {
        Some(star) => {
            let (pre, suf) = (&pat[..star], &pat[star + 1..]);
            if spec.starts_with(pre) && spec.ends_with(suf) && spec.len() >= pre.len() + suf.len() {
                Some(spec[pre.len()..spec.len() - suf.len()].to_string())
            } else {
                None
            }
        }
        None => (pat == spec).then(|| String::new()),
    }
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
                    out.push(".."); // climbing past a relative base keeps the `..`
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Probe `base`, then `base.<ext>`, then `base/index.<ext>`.
pub fn try_file(base: &Path) -> Option<PathBuf> {
    let base = &normalize(base);
    if base.is_file() {
        return Some(base.to_path_buf());
    }
    let bs = base.to_string_lossy().to_string();
    for e in EXTS {
        let p = PathBuf::from(format!("{bs}.{e}"));
        if p.is_file() {
            return Some(p);
        }
    }
    for e in EXTS {
        let p = PathBuf::from(format!("{bs}/index.{e}"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Split a bare specifier into (package, subpath) — scoped packages take 2 segments.
fn split_pkg(spec: &str) -> (String, String) {
    if let Some(rest) = spec.strip_prefix('@') {
        if let Some(slash) = rest.find('/') {
            if let Some(end) = rest[slash + 1..].find('/') {
                let e = slash + 1 + end;
                return (
                    format!("@{}/{}", &rest[..slash], &rest[slash + 1..e]),
                    spec[e + 1..].to_string(),
                );
            }
            return (spec.to_string(), ".".to_string());
        }
        return (spec.to_string(), ".".to_string());
    }
    match spec.find('/') {
        Some(i) => (spec[..i].to_string(), spec[i + 1..].to_string()),
        None => (spec.to_string(), ".".to_string()),
    }
}

fn resolve_package(root: &Path, spec: &str, env: BundleEnv) -> Option<PathBuf> {
    let (pkg, sub) = split_pkg(spec);
    let pkg_dir = root.join("node_modules").join(&pkg);
    let pkg_json = pkg_dir.join("package.json");
    if !pkg_json.is_file() {
        // no manifest: allow direct path-style resolution under node_modules
        return try_file(&root.join("node_modules").join(spec));
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&pkg_json).ok()?).ok()?;
    let conds: &[&str] = match env {
        BundleEnv::Client => &["import", "browser", "default"],
        _ => &["import", "node", "default"],
    };
    if let Some(exports) = v.get("exports") {
        let key = if sub == "." {
            ".".to_string()
        } else {
            format!("./{sub}")
        };
        if let Some(target) = exports_resolve(exports, conds, &key) {
            let t = target.trim_start_matches("./");
            if let Some(p) = try_file(&pkg_dir.join(t)) {
                return Some(p);
            }
        }
        // tolerant: fall through to main fields when `exports` does not match
    }
    for key in ["module", "main"] {
        if let Some(t) = v.get(key).and_then(|x| x.as_str()) {
            if let Some(p) = try_file(&pkg_dir.join(t.trim_start_matches("./"))) {
                return Some(p);
            }
        }
    }
    try_file(&pkg_dir.join("index.js"))
}

/// Node-style `exports` resolution: strings, fallback arrays, subpath maps
/// (exact + `*` patterns) and condition objects walked in `conds` priority order.
fn exports_resolve(v: &Value, conds: &[&str], key: &str) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(|x| exports_resolve(x, conds, key)),
        Value::Object(o) => {
            let is_submap = o.keys().any(|k| k.starts_with('.'));
            if is_submap {
                if let Some(t) = o.get(key) {
                    return exports_resolve(t, conds, key);
                }
                for (k, t) in o {
                    let Some(star) = k.find('*') else { continue };
                    let (pre, suf) = (&k[..star], &k[star + 1..]);
                    if !pre.starts_with('.') {
                        continue;
                    }
                    if key.starts_with(pre)
                        && key.ends_with(suf)
                        && key.len() >= pre.len() + suf.len()
                    {
                        let mid = &key[pre.len()..key.len() - suf.len()];
                        let target = t.as_str()?.replace('*', mid);
                        return Some(target);
                    }
                }
                None
            } else {
                for c in conds {
                    if let Some(t) = o.get(*c) {
                        return exports_resolve(t, conds, key);
                    }
                }
                None
            }
        }
        _ => None,
    }
}

fn unresolved(root: &Path, specifier: &str, from: &Path) -> OxideError {
    let (pkg, _) = split_pkg(specifier.split('?').next().unwrap_or(specifier));
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
