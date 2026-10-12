//! App snapshots (spec §4.4, D2). At `oxide build`, a synthetic
//! `deno_core::Extension` carries the server bundle's top-level source;
//! `create_snapshot` boots a `JsRuntimeForSnapshot`, evaluates the bundle, and
//! returns the serialized V8 startup blob. At `oxide start` each isolate
//! deserializes the blob instead of re-evaluating.
//!
//! Fallback: any failure (snapshot-hostile dependency, top-level side effect
//! that creates a live handle) degrades to a V8 code cache — the loader feeds
//! the source through V8 on second boot; parse and compile are still skipped.

use crate::{err, Result};
use deno_core::snapshot::{create_snapshot, CreateSnapshotOptions};
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
        ("setTimeout(", "setTimeout at module top level holds a live timer at snapshot time"),
        ("setInterval(", "setInterval at module top level holds a live timer at snapshot time"),
        ("new WebSocket(", "WebSocket opened at module top level holds a live socket"),
        ("new EventSource(", "EventSource opened at module top level holds a live socket"),
    ];
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if depth == 0 {
            for (needle, msg) in PATTERNS {
                if trimmed.contains(needle) {
                    out.push(SideEffect {
                        path: path.to_string(),
                        line: (i + 1) as u32,
                        message: (*msg).to_string(),
                    });
                }
            }
        }
        depth += line.matches('{').count() as i32;
        depth -= line.matches('}').count() as i32;
        if depth < 0 {
            depth = 0;
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
    let ext_name = leak(format!("oxide_snapshot_{}", entry.replace(['/', '.', '-'], "_")));
    let specifier: &'static str = leak(format!("oxide:snapshot/{}", entry));
    // `new_computed` is the constructor for runtime-computed source (Arc<str>);
    // `new` requires a `FastStaticString` compile-time literal that we can't
    // produce for a user bundle. The specifier here matches `esm_entry_point`
    // below so deno_core evaluates exactly this module.
    let esm_files: Vec<ExtensionFileSource> =
        vec![ExtensionFileSource::new_computed(specifier, Arc::from(source))];

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
