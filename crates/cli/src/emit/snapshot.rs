//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// §4.4: lint each route's server module for top-level side effects that
/// break V8 snapshots — a `setTimeout`, `setInterval`, `new WebSocket` or
/// `new EventSource` at module scope holds a live handle at snapshot time.
/// The spec requires `oxide build` to report these with source locations so
/// a user can move the work behind a lazy initializer. Advisory: it never
/// fails the build, because the snapshot fallback (a V8 code cache) is a
/// valid degradation.
pub(crate) fn persist_diagnostics(root: &Path, diags: &[oxide_fw_diagnostics::Diagnostic]) {
    let dir = root.join(".oxide");
    let _ = std::fs::create_dir_all(&dir);
    let json = serde_json::to_string_pretty(diags).unwrap_or_else(|_| "[]".to_string());
    let _ = std::fs::write(dir.join("last-build-diagnostics.json"), json);
}

pub(crate) fn emit_snapshot_lint(
    cfg: &oxide_fw_config::OxideConfig,
    root: &Path,
    tree: &oxide_fw_router::RouteTree,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let routes_root = root.join(&cfg.routes.dir);
    let mut count = 0usize;
    // 19-E: each side effect becomes a real Diagnostic — location plus a
    // code frame — so the persisted diagnostics file (and the editor / MCP
    // tool reading it) shows the offending line, not just a stderr line.
    let mut diags: Vec<oxide_fw_diagnostics::Diagnostic> = Vec::new();
    for e in &tree.entries {
        if !matches!(e.kind, oxide_fw_router::RouteKind::Page) {
            continue;
        }
        let server = routes_root.join(e.file.with_file_name("page.server.ts"));
        let Ok(source) = std::fs::read_to_string(&server) else {
            continue;
        };
        let rel = server
            .strip_prefix(root)
            .unwrap_or(&server)
            .to_string_lossy()
            .to_string();
        for fx in oxide_fw_jshost::lint_top_level_side_effects(&rel, &source) {
            outln!(
                w,
                "warn: snapshot side effect at {}:{} - {}",
                fx.path,
                fx.line,
                fx.message
            );
            let mut d = oxide_fw_diagnostics::Diagnostic::warning(
                "SNP-SIDEEFFECT",
                format!("{}: {}", fx.path, fx.message),
            )
            .with_fix(
                "move the call behind a lazy initializer (a function called on first \
                 request) so the module body has no live timer/socket at snapshot time",
            );
            let line = fx.line as usize;
            if line > 0 {
                d = d.at(oxide_fw_diagnostics::Location::file(&fx.path, fx.line, 1));
                let frame = oxide_fw_diagnostics::render_code_frame(&source, line, None);
                if !frame.is_empty() {
                    d = d.with_code_frame(frame);
                }
            }
            diags.push(d);
            count += 1;
        }
    }
    if count > 0 {
        outln!(
            w,
            "snapshot: {count} top-level side effect(s) - move them behind a lazy initializer for a clean snapshot"
        );
        persist_diagnostics(root, &diags);
    }
    Ok(())
}

/// §4.4: build a V8 startup snapshot from the emitted server bundle and
/// write it to `<root>/.oxide/snapshot.bin`. `oxide start` with
/// `server.backend = "v8"` loads it, restoring the evaluated bundle heap
/// instead of re-evaluating it per worker.
///
/// A snapshot failure (a bundle that does not evaluate cleanly in V8) is
/// reported and skipped — the server still runs without one.
///
/// M1 (capability matrix): the deno_core snapshot machinery rides the
/// crate's `v8` feature. Without it there is no V8 isolate in this binary
/// to snapshot, and no `server.backend = "v8"` server to load the blob —
/// the write is skipped with a loud `SNP-V8OFF` line instead.
#[cfg(not(feature = "v8"))]
pub(crate) fn emit_snapshot_blob(
    root: &Path,
    server_out: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let _ = (&root, &server_out);
    let entry = server_out.join("ssr/index.js");
    if entry.is_file() {
        outln!(
            w,
            "snapshot: skipped (SNP-V8OFF: the `.oxide/snapshot.bin` write needs the `v8` feature; \
             rebuild with `cargo build -p oxide-fw-cli --features v8`)"
        );
    }
    Ok(())
}

/// §4.4: build a V8 startup snapshot from the emitted server bundle and
/// write it to `<root>/.oxide/snapshot.bin`. The `v8`-featured twin of the
/// `SNP-V8OFF` skip above.
#[cfg(feature = "v8")]
pub(crate) fn emit_snapshot_blob(
    root: &Path,
    server_out: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let entry = server_out.join("ssr/index.js");
    if !entry.is_file() {
        return Ok(());
    }
    // BUG-12: the old bundle walk is gone — the emitted snapshot is the
    // runtime bootstrap only (imported at runtime by `__oxideDispatch`);
    // only `entry` matters (its existence gates the snapshot at all).
    // `create_snapshot` builds its own Tokio runtime internally and drops it;
    // dropping a runtime from inside an async context panics ("Cannot drop a
    // runtime in a context where blocking is not allowed"). `cmd_build` is
    // async, so run the snapshot on a dedicated thread where the inner runtime
    // is dropped outside any async context.
    let built = std::thread::spawn(oxide_fw_jshost::snapshot_build_runtime)
        .join()
        .unwrap_or_else(|_| {
            Err(OxideError(Box::new(
                oxide_fw_diagnostics::Diagnostic::error(
                    "SNP-PANIC",
                    "snapshot builder thread panicked",
                ),
            )))
        });

    match built {
        Ok(snap) if !snap.blob.is_empty() => {
            let dir = root.join(".oxide");
            std::fs::create_dir_all(&dir)
                .map_err(|e| anyhow::anyhow!("create {}: {e}", dir.display()))?;
            // BUG-12: versioned `AppSnapshot` envelope (not a raw blob), so
            // `from_snapshot_file` can reject stale/foreign snapshots with
            // `JSH-SNAPVER` instead of an opaque deno_core error at spawn.
            let env = serde_json::json!({
                "format": oxide_fw_jshost::SNAPSHOT_FORMAT,
                "oxide_version": env!("CARGO_PKG_VERSION"),
                "source_hash": snap.source_hash,
                "blob_b64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &snap.blob,
                ),
            });
            std::fs::write(
                dir.join("snapshot.bin"),
                serde_json::to_vec(&env)
                    .map_err(|e| anyhow::anyhow!("serialize snapshot envelope: {e}"))?,
            )
            .map_err(|e| anyhow::anyhow!("write snapshot.bin: {e}"))?;
            outln!(w, "snapshot: {} bytes", snap.blob.len());
        }
        Ok(snap) => {
            // No blob; the warnings say why. Not fatal.
            for warn in &snap.warnings {
                outln!(w, "snapshot: skipped ({warn})");
            }
        }
        Err(e) => {
            outln!(w, "snapshot: skipped ({} {})", e.0.code, e.0.message);
        }
    }
    Ok(())
}
