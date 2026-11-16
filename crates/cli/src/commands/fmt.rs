//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// `oxide check` (§19.1): config validation + route conventions diagnostics.
/// V2-C11 (§V2-C11): resolve an external tool for a project — the
/// project-local install first (`node_modules/.bin`, so a
/// workspace-pinned toolchain is authoritative), then `PATH`.
#[must_use]
pub fn resolve_tool(name: &str, root: &std::path::Path) -> Option<std::path::PathBuf> {
    let local = root.join("node_modules/.bin").join(name);
    if local.is_file() {
        return Some(local);
    }
    which(name)
}

/// V2-C11 (§V2-C11): the `(tool, args)` pair the configured
/// `check.typecheck` backend runs. `None` = the arm is off (`off`).
/// Public: the `check_backends` walkthrough example re-derives the
/// exact commands this plans.
#[must_use]
pub fn typecheck_backend_plan(
    backend: oxide_fw_config::TypecheckBackend,
) -> Option<(&'static str, Vec<&'static str>)> {
    match backend {
        oxide_fw_config::TypecheckBackend::Tsgo => {
            Some(("tsgo", vec!["--noEmit", "--pretty", "false"]))
        }
        oxide_fw_config::TypecheckBackend::Oxlint => {
            Some(("oxlint", vec!["--type-aware", "--type-check"]))
        }
        oxide_fw_config::TypecheckBackend::Tsrs => Some(("tsrs", vec!["--noEmit"])),
        oxide_fw_config::TypecheckBackend::Tsc => {
            Some(("tsc", vec!["--noEmit", "--pretty", "false"]))
        }
        oxide_fw_config::TypecheckBackend::Off => None,
    }
}

/// V2-C11 (§V2-C11): run one resolved external tool and print the
/// P1-17 outcome lines (`OK` / `FAILED` with the tool's own output
/// verbatim). Returns whether the tool succeeded.
pub(crate) fn run_tool(
    label: &str,
    bin: &Path,
    args: &[&str],
    rootp: &Path,
    w: Writer<'_>,
) -> anyhow::Result<bool> {
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args).current_dir(rootp);
    match cmd.output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                outln!(w, "{label}: OK");
                Ok(true)
            } else {
                outln!(w, "{label}: FAILED");
                // Emit the tool's own lines verbatim (they are already
                // human-formatted).
                for line in stdout.lines().chain(stderr.lines()).take(50) {
                    outln!(w, "  {line}");
                }
                Ok(false)
            }
        }
        Err(e) => {
            outln!(w, "{label}: could not run `{}`: {e}", bin.display());
            Ok(false)
        }
    }
}

/// `oxide fmt [--check]` (§V2-C11): shell out to `oxfmt`, fall back to
/// `biome format` when `oxfmt` is absent, and print a loud `SKIPPED`
/// line when neither is installed (the P1-17 convention).
/// `OXIDE_CHECK_STRICT=1` turns that skip into a failure. The formatter
/// has NO config key — which formatter runs is the CLI's decision, not
/// the project's.
pub(crate) async fn cmd_fmt(root: &str, check: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let strict = std::env::var_os("OXIDE_CHECK_STRICT").is_some();
    let args: Vec<&str> = if check {
        vec!["--check", "."]
    } else {
        vec!["."]
    };
    if let Some(tool) = resolve_tool("oxfmt", &rootp) {
        let ok = run_tool("fmt (oxfmt)", &tool, &args, &rootp, w)?;
        return Ok(if ok { 0 } else { 1 });
    }
    outln!(w, "fmt: `oxfmt` not found — falling back to `biome format`");
    if let Some(tool) = resolve_tool("biome", &rootp) {
        // `biome format` (no `--write`) prints diffs and exits non-zero
        // when files would change — the same contract as `oxfmt --check`;
        // the fix pass adds `--write` when the caller asked for one.
        let mut biome_args: Vec<&str> = vec!["format"];
        if !check {
            biome_args.push("--write");
        }
        biome_args.push(".");
        let ok = run_tool("fmt (biome format)", &tool, &biome_args, &rootp, w)?;
        return Ok(if ok { 0 } else { 1 });
    }
    outln!(
        w,
        "fmt: SKIPPED (neither `oxfmt` nor `biome` found — install one (npm i -D @oxfmt/cli or @biomejs/biome) or set OXIDE_CHECK_STRICT=1 to fail)"
    );
    if strict {
        outln!(w, "fmt: OXIDE_CHECK_STRICT=1 — the missing formatter fails");
        return Ok(1);
    }
    Ok(0)
}
