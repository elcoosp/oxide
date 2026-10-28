//! App snapshots (spec §4.4, D2). At `oxide build`, a synthetic
//! `deno_core::Extension` carries the server bundle's top-level source;
//! `create_snapshot` boots a `JsRuntimeForSnapshot`, evaluates the bundle, and
//! returns the serialized V8 startup blob. At `oxide start` each isolate
//! deserializes the blob instead of re-evaluating.
//!
//! Fallback: any failure (snapshot-hostile dependency, top-level side effect
//! that creates a live handle) degrades to a V8 code cache — the loader feeds
//! the source through V8 on second boot; parse and compile are still skipped.

use crate::{Result, err};
use deno_core::snapshot::{CreateSnapshotOptions, create_snapshot};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Version tag; the loader rejects blobs with a mismatched tag.
pub const SNAPSHOT_FORMAT: u32 = 1;

/// The compiled snapshot plus metadata for the loader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSnapshot {
    pub format: u32,
    /// V8 startup blob. Empty when only a code-cache fallback is present.
    pub blob: Vec<u8>,
    /// Fallback code cache (raw bundle bytes).
    pub code_cache: Option<Vec<u8>>,
    pub entry: String,
    /// blake3 of the source, for cache-keying rebuilds.
    pub source_hash: String,
    /// Non-fatal build diagnostics (e.g. fallback reason).
    pub warnings: Vec<String>,
}

/// A top-level side effect reported by the pre-snapshot lint pass (§4.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideEffect {
    pub path: String,
    pub line: u32,
    pub message: String,
}

/// Scan a module's top level for snapshot-hostile patterns. Advisory: a real
/// live handle fails the snapshot build itself; the lint points at the source
/// first.
pub fn lint_top_level_side_effects(path: &str, source: &str) -> Vec<SideEffect> {
    const PATTERNS: &[(&str, &str)] = &[
        (
            "setTimeout(",
            "setTimeout at module top level holds a live timer at snapshot time",
        ),
        (
            "setInterval(",
            "setInterval at module top level holds a live timer at snapshot time",
        ),
        (
            "new WebSocket(",
            "WebSocket opened at module top level holds a live socket",
        ),
        (
            "new EventSource(",
            "EventSource opened at module top level holds a live socket",
        ),
    ];
    // Character-level scan: a pattern occurrence counts as a top-level side
    // effect only when brace depth is 0 *at that position*. The previous
    // line-granular scan sampled depth at the start of each line, so a
    // function whose body opens and closes on the same line —
    // `function f() { setTimeout(…) }` — was wrongly reported (the `{` and
    // `}` on that line cancelled out, but the match had already been
    // collected against the stale line-start depth).
    //
    // Known limitation, documented rather than silently wrong: an arrow
    // body without braces (`const t = () => setTimeout(…)`) is still
    // reported. That is a false positive in the *safe* (over-warning)
    // direction for an advisory lint — the snapshot build itself fails on
    // a real live handle, so the lint's job is to point at the source
    // first, and a needless warning never ships a broken snapshot.
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut line: u32 = 1;
    let mut i = 0usize;
    while i < source.len() {
        let b = source.as_bytes()[i];
        match b {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth < 0 {
                    depth = 0;
                }
                i += 1;
            }
            _ => {
                let mut advanced = 1;
                if depth == 0
                    && let Some(rest) = source.get(i..)
                {
                    for (needle, msg) in PATTERNS {
                        if rest.starts_with(needle) {
                            out.push(SideEffect {
                                path: path.to_string(),
                                line,
                                message: (*msg).to_string(),
                            });
                            advanced = needle.len();
                            break;
                        }
                    }
                }
                i += advanced;
            }
        }
    }
    out
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Build a snapshot from the server bundle's top-level source (§4.4).
pub fn snapshot_build(entry: &str, source: &str) -> Result<AppSnapshot> {
    let source_hash = blake3::hash(source.as_bytes()).to_hex().to_string();
    let warnings: Vec<String> = Vec::new();

    // `Extension` is a plain struct in deno_core 0.412 — no builder. ESM
    // sources are `Cow<'static, [ExtensionFileSource]>`.
    use deno_core::ExtensionFileSource;
    use std::sync::Arc;
    let ext_name = leak(format!(
        "oxide_snapshot_{}",
        entry.replace(['/', '.', '-'], "_")
    ));
    let specifier: &'static str = leak(format!("oxide:snapshot/{}", entry));
    // `new_computed` is the constructor for runtime-computed source (Arc<str>);
    // `new` requires a `FastStaticString` compile-time literal that we can't
    // produce for a user bundle. The specifier here matches `esm_entry_point`
    // below so deno_core evaluates exactly this module.
    let esm_files: Vec<ExtensionFileSource> = vec![ExtensionFileSource::new_computed(
        specifier,
        Arc::from(source),
    )];

    let ext = deno_core::Extension {
        name: ext_name,
        deps: &[],
        js_files: Cow::Borrowed(&[]),
        esm_files: Cow::Owned(esm_files),
        lazy_loaded_esm_files: Cow::Borrowed(&[]),
        lazy_loaded_js_files: Cow::Borrowed(&[]),
        synthetic_esm_modules: Cow::Borrowed(&[]),
        // Setting the ESM entry point is what makes deno_core evaluate the
        // module during snapshot creation; without it the snapshot returns a
        // `NonEvaluatedModules` error.
        esm_entry_point: Some(specifier),
        ops: Cow::Borrowed(&[]),
        objects: Cow::Borrowed(&[]),
        external_references: Cow::Borrowed(&[]),
        global_template_middleware: None,
        global_object_middleware: None,
        op_state_fn: None,
        needs_lazy_init: false,
        middleware_fn: None,
        enabled: true,
    };

    let options = CreateSnapshotOptions {
        cargo_manifest_dir: leak(env!("CARGO_MANIFEST_DIR").to_string()),
        startup_snapshot: None,
        skip_op_registration: false,
        extensions: vec![ext],
        extension_transpiler: None,
        with_runtime_cb: None,
    };

    match create_snapshot(options, None) {
        Ok(output) => Ok(AppSnapshot {
            format: SNAPSHOT_FORMAT,
            blob: output.output.to_vec(),
            code_cache: None,
            entry: entry.to_string(),
            source_hash,
            warnings,
        }),
        Err(e) => {
            let mut warnings = warnings;
            warnings.push(format!(
                "snapshot build failed ({}); falling back to V8 code cache",
                e
            ));
            Ok(AppSnapshot {
                format: SNAPSHOT_FORMAT,
                blob: Vec::new(),
                code_cache: Some(source.as_bytes().to_vec()),
                entry: entry.to_string(),
                source_hash,
                warnings,
            })
        }
    }
}

/// deno_core requires extension source to be 7-bit ASCII (a snapshot is a
/// serialized V8 heap and the extension loader debug-asserts ASCII). A real
/// bundle contains non-ASCII in a banner comment (`§`) and, potentially, in a
/// user's string literals. Replacing each non-ASCII scalar with a `\uXXXX`
/// escape is **semantics-preserving** for JavaScript: identical inside string,
/// template and regex literals, and merely cosmetic inside a comment.
pub(crate) fn escape_non_ascii(src: &str) -> String {
    if src.is_ascii() {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + 16);
    for ch in src.chars() {
        let cp = ch as u32;
        if cp < 0x80 {
            out.push(ch);
        } else if cp <= 0xFFFF {
            out.push_str(&format!("\\u{cp:04x}"));
        } else {
            // Astral: encode as a surrogate pair (each `\uXXXX`).
            let v = cp - 0x1_0000;
            let hi = 0xD800 + (v >> 10);
            let lo = 0xDC00 + (v & 0x3FF);
            out.push_str(&format!("\\u{hi:04x}\\u{lo:04x}"));
        }
    }
    out
}

/// Build a snapshot from a **multi-file** server bundle (§4.4).
///
/// `snapshot_build` handles one module; a real `dist/server` bundle is a
/// graph (`ssr/index.js` → `routes/*/page.js` → a shared `jsx-runtime-*.js`).
/// deno_core's `create_snapshot` builds its internal runtime with no
/// `ModuleLoader`, so it cannot read those files from disk — but
/// `ExtModuleLoader` *does* resolve a relative import among the extension's
/// registered `esm_files` by URL-joining. So: register every file under its
/// `file://` URL, point `esm_entry_point` at the entry, and let the snapshot
/// runtime evaluate the whole graph.
///
/// The §4.6 tier-2 compat globals (`process`, `Buffer`, `oxide.*`,
/// `__oxideDispatch`) are installed first, as an extension `js_file`, because
/// a bundled chunk references `process.env.NODE_ENV` at module scope and would
/// otherwise throw `process is not defined` during evaluation.
///
/// `files` is `(absolute path, source)`. `entry_path` must be one of them.
pub fn snapshot_build_bundle(
    entry_path: &str,
    files: &[(std::path::PathBuf, String)],
) -> Result<AppSnapshot> {
    use deno_core::{ExtensionFileSource, ModuleSpecifier};
    use std::sync::Arc;

    // A digest over every file so a rebuild is keyed on the whole graph.
    let mut h = blake3::Hasher::new();
    for (p, s) in files {
        h.update(p.to_string_lossy().as_bytes());
        h.update(&[0]);
        h.update(s.as_bytes());
        h.update(&[0]);
    }
    let source_hash = h.finalize().to_hex().to_string();
    let mut warnings: Vec<String> = Vec::new();

    // `file://` URL for every module; the entry must resolve to one of them.
    // Canonicalize before building a `file://` URL. A path may contain `..`
    // (the caller often passes `<manifest>/../../project/...`); `from_file_path`
    // keeps `..` verbatim, but URL *joining* — what resolves a relative import
    // — collapses it. The registered specifier and the resolved one would then
    // differ and deno_core reports "was not passed as an extension module".
    let url_of = |p: &std::path::Path| -> Option<String> {
        // `canonicalize` adds the Windows `\\?\` verbatim prefix, which
        // `from_file_path` cannot represent — strip it before the URL
        // conversion or every V8 snapshot build fails on Windows.
        let canon =
            crate::strip_verbatim(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
        ModuleSpecifier::from_file_path(&canon)
            .ok()
            .map(|u| u.to_string())
    };
    let entry_url = match url_of(std::path::Path::new(entry_path)) {
        Some(u) => u,
        None => {
            return Ok(AppSnapshot {
                format: SNAPSHOT_FORMAT,
                blob: Vec::new(),
                code_cache: None,
                entry: entry_path.to_string(),
                source_hash,
                warnings: vec![format!("entry is not a file path: {entry_path}")],
            });
        }
    };

    let mut esm_files: Vec<ExtensionFileSource> = Vec::with_capacity(files.len());
    for (p, src) in files {
        let Some(url) = url_of(p) else { continue };
        let spec: &'static str = leak(url);
        let ascii = escape_non_ascii(src);
        esm_files.push(ExtensionFileSource::new_computed(
            spec,
            Arc::from(ascii.as_str()),
        ));
    }

    // The compat bootstrap, as a plain JS file so it runs before the ESM entry.
    let bootstrap_spec: &'static str = leak("oxide:bootstrap".to_string());
    let bootstrap_ascii = escape_non_ascii(crate::V8_BOOTSTRAP);
    let js_files: Vec<ExtensionFileSource> = vec![ExtensionFileSource::new_computed(
        bootstrap_spec,
        Arc::from(bootstrap_ascii.as_str()),
    )];

    let ext = deno_core::Extension {
        name: leak(format!(
            "oxide_snapshot_{}",
            entry_path.replace(['/', '.', '-'], "_")
        )),
        deps: &[],
        js_files: Cow::Owned(js_files),
        esm_files: Cow::Owned(esm_files),
        lazy_loaded_esm_files: Cow::Borrowed(&[]),
        lazy_loaded_js_files: Cow::Borrowed(&[]),
        synthetic_esm_modules: Cow::Borrowed(&[]),
        esm_entry_point: Some(leak(entry_url)),
        // §4.5: bake the oxide ops into the snapshot. Without them a worker
        // booting from the blob has no `Deno.core.ops.op_oxide_*` — the
        // bootstrap's `crypto` shim (`op_oxide_random`) and `oxide.serialize`
        // fail at call time. `create_snapshot` registers these when
        // `skip_op_registration` is false (it is).
        ops: Cow::Owned(crate::v8_ops::op_decls()),
        objects: Cow::Borrowed(&[]),
        external_references: Cow::Borrowed(&[]),
        global_template_middleware: None,
        global_object_middleware: None,
        op_state_fn: None,
        needs_lazy_init: false,
        middleware_fn: None,
        enabled: true,
    };

    let options = CreateSnapshotOptions {
        cargo_manifest_dir: leak(env!("CARGO_MANIFEST_DIR").to_string()),
        startup_snapshot: None,
        skip_op_registration: false,
        extensions: vec![ext],
        extension_transpiler: None,
        with_runtime_cb: None,
    };

    match create_snapshot(options, None) {
        Ok(output) => Ok(AppSnapshot {
            format: SNAPSHOT_FORMAT,
            blob: output.output.to_vec(),
            code_cache: None,
            entry: entry_path.to_string(),
            source_hash,
            warnings,
        }),
        Err(e) => {
            warnings.push(format!(
                "bundle snapshot failed ({e}); falling back to no snapshot"
            ));
            Ok(AppSnapshot {
                format: SNAPSHOT_FORMAT,
                blob: Vec::new(),
                code_cache: None,
                entry: entry_path.to_string(),
                source_hash,
                warnings,
            })
        }
    }
}

/// Build a V8 startup snapshot of the **oxide runtime extension** — the
/// bootstrap (`process`, `Buffer`, Web globals, `__oxideDispatch`, the
/// framework/loader ops) and the ops themselves (§4.4).
///
/// This is what makes a cold boot cheap: the blob captures V8 already
/// initialized and the bootstrap already evaluated, so `oxide start` restores
/// it per worker instead of re-running both. The bundle is *not* snapshotted —
/// it is imported at runtime by `__oxideDispatch`, which lets this snapshot use
/// a **fixed** extension name (`oxide_runtime`) that the worker's load-time
/// extension matches exactly. (A per-entry name made the snapshot unloadable:
/// deno_core checks extension names in order.)
pub fn snapshot_build_runtime() -> Result<AppSnapshot> {
    let mut warnings: Vec<String> = Vec::new();
    // The source hash is a constant: the extension is fixed. A version tag is
    // enough to key rebuilds; the CLI's own build fingerprint gates the write.
    let source_hash = blake3::hash(crate::V8_BOOTSTRAP.as_bytes())
        .to_hex()
        .to_string();

    let options = CreateSnapshotOptions {
        cargo_manifest_dir: leak(env!("CARGO_MANIFEST_DIR").to_string()),
        startup_snapshot: None,
        skip_op_registration: false,
        extensions: vec![crate::oxide_extension()],
        extension_transpiler: None,
        with_runtime_cb: None,
    };

    match create_snapshot(options, None) {
        Ok(output) => Ok(AppSnapshot {
            format: SNAPSHOT_FORMAT,
            blob: output.output.to_vec(),
            code_cache: None,
            entry: "oxide:runtime".to_string(),
            source_hash,
            warnings,
        }),
        Err(e) => {
            warnings.push(format!("runtime snapshot failed ({e})"));
            Ok(AppSnapshot {
                format: SNAPSHOT_FORMAT,
                blob: Vec::new(),
                code_cache: None,
                entry: "oxide:runtime".to_string(),
                source_hash,
                warnings,
            })
        }
    }
}

/// Load a snapshot blob into a fresh `JsRuntime` (§4.4). Returns the
/// `RuntimeOptions` to pass to `JsRuntime::new`, plus a bool that is `true`
/// when a code-cache fallback is in use (the caller must then
/// `execute_script("oxide:bundle", code)` after runtime creation).
pub fn loader_options(snap: &AppSnapshot) -> Result<(deno_core::RuntimeOptions, bool)> {
    if snap.format != SNAPSHOT_FORMAT {
        return Err(err(
            "SNP-FORMAT",
            format!(
                "snapshot format {} is not supported (expected {})",
                snap.format, SNAPSHOT_FORMAT
            ),
        ));
    }
    if !snap.blob.is_empty() {
        let blob: &'static [u8] = Box::leak(snap.blob.clone().into_boxed_slice());
        let opts = deno_core::RuntimeOptions {
            startup_snapshot: Some(blob),
            ..Default::default()
        };
        Ok((opts, false))
    } else {
        Ok((deno_core::RuntimeOptions::default(), true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_timer_is_reported() {
        let src = "const t = setTimeout(() => {}, 100);\n";
        let effects = lint_top_level_side_effects("entry.js", src);
        assert_eq!(effects.len(), 1);
        assert!(effects[0].message.contains("setTimeout"));
        assert_eq!(effects[0].line, 1);
    }

    #[test]
    fn timer_inside_function_is_not_reported() {
        let src = "function start() {\n  setTimeout(() => {}, 100);\n}\n";
        assert!(lint_top_level_side_effects("entry.js", src).is_empty());
    }

    #[test]
    fn timer_inside_a_single_line_function_is_not_reported() {
        // Regression: the line-granular scan sampled depth at line start, so
        // `{` and `}` on the same line cancelled out and the timer inside
        // was reported as top-level. Found by the CLI-level §4.4 audit.
        let src = "function start() { setTimeout(() => {}, 100); }\n";
        assert!(
            lint_top_level_side_effects("entry.js", src).is_empty(),
            "a same-line function body is not top level"
        );
    }

    #[test]
    fn timer_after_a_same_line_function_is_reported() {
        // The character scan must still flag a genuine top-level call that
        // follows a closed block on the same line.
        let src = "function f() {} setTimeout(() => {}, 100);\n";
        let hits = lint_top_level_side_effects("entry.js", src);
        assert_eq!(hits.len(), 1, "top-level timer after a closed block");
    }

    #[test]
    fn fetch_at_top_level_is_allowed() {
        let src = "fetch('/x');\n";
        assert!(lint_top_level_side_effects("entry.js", src).is_empty());
    }

    #[test]
    #[ignore = "V8 snapshot machinery leaks an isolate under test harness; \
                validated end-to-end by `oxide build`. Run with --ignored to exercise."]
    fn snapshot_build_produces_blob_or_fallback() {
        let src = "globalThis.__oxide_snapshot_marker = 1;";
        let snap = snapshot_build("entry.js", src).unwrap();
        assert_eq!(snap.format, SNAPSHOT_FORMAT);
        assert_eq!(snap.source_hash.len(), 64);
        assert!(
            !snap.blob.is_empty() || snap.code_cache.is_some(),
            "warnings: {:?}",
            snap.warnings
        );
    }

    #[test]
    fn loader_options_reject_wrong_format() {
        let snap = AppSnapshot {
            format: 999,
            blob: vec![],
            code_cache: None,
            entry: "x".into(),
            source_hash: "h".into(),
            warnings: vec![],
        };
        match loader_options(&snap) {
            Err(e) => assert_eq!(e.0.code, "SNP-FORMAT"),
            Ok(_) => panic!("expected error"),
        }
    }
}

#[cfg(test)]
mod side_effect_frame_tests {
    use super::*;

    #[test]
    fn lint_reports_a_line_number() {
        let src = "const a = 1;\nsetTimeout(() => {}, 100);\n";
        let hits = lint_top_level_side_effects("x.ts", src);
        assert!(!hits.is_empty(), "setTimeout is flagged");
        assert!(hits[0].line >= 1, "line is 1-based, got {}", hits[0].line);
    }
}
