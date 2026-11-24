//! V2-A02 closing leg — the workspace lint-uniformity drift guard.
//!
//! The workspace's lint policy lives once, at the root: `[workspace.lints.rust]
//! warnings = "deny"` and `[workspace.lints.clippy] pedantic = { level = "warn",
//! priority = -1 }`. Every workspace member opts in by declaring
//!
//! ```toml
//! [lints]
//! workspace = true
//! ```
//!
//! and nothing else. A member that carries a bare `[lints]` header (no
//! `workspace = true`) opts OUT of pedantic silently; a member that adds a
//! `[lints.clippy]` / `[lints.rust]` override diverges from the one contract.
//! Both classes of drift were the documented reason the workspace clippy gate
//! stayed RED across five sessions (docs/deviations.md, the V2-A02 deferral
//! record and its five follow-on postscripts).
//!
//! This test is the net-new capability that makes re-deferral impossible: it
//! parses every workspace member's `Cargo.toml`, finds the `[lints]` table,
//! and asserts it inherits the workspace policy and carries no override. It
//! mirrors the `matrix_json.rs` / `compat_tdd.rs` drift-guard shape — a
//! machine-checked invariant over a workspace artifact, with a clear
//! regenerate/fix instruction in the failure message.
//!
//! The companion `scripts/check-lint-uniformity.sh` is the CI-callable form
//! (the rust/lint job can invoke it before `cargo clippy`); this test is the
//! `cargo test`-discoverable form that fails the suite the moment a crate
//! re-introduces a divergent table or a new member opts out.

use std::fs;
use std::path::PathBuf;

/// The workspace root, reached up from this crate's manifest dir
/// (`crates/diagnostics` → repo root). Mirrors `matrix_json.rs`'s
/// `../../conformance/matrix.json` reach-across-the-boundary pattern.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/diagnostics has a parent (the crates/ dir)")
        .parent()
        .expect("crates/ has a parent (the workspace root)")
        .to_path_buf()
}

/// The root `Cargo.toml`'s `[workspace] members` array, parsed as a list of
/// member directory paths (relative to the workspace root). The members list
/// uses explicit `crates/<name>` paths (no globs), so each entry resolves
/// directly to `<root>/<member>/Cargo.toml`.
fn workspace_members(root: &std::path::Path) -> Vec<String> {
    let toml = fs::read_to_string(root.join("Cargo.toml")).unwrap_or_else(|e| {
        panic!(
            "workspace root Cargo.toml unreadable at {}: {e}",
            root.join("Cargo.toml").display()
        )
    });
    // The `members = [ ... ]` array is multi-line, one quoted path per line.
    // Pull every `"crates/..."` (or other quoted path) between `members = [`
    // and the closing `]` on its own line.
    let members_header = toml
        .find("members = [")
        .or_else(|| toml.find("members=["))
        .expect("the root Cargo.toml declares a [workspace] members array");
    let after = &toml[members_header..];
    let end = after.find(']').expect("the members array closes with ]");
    let body = &after[..end];
    let mut out = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        // Each member line is `"crates/<name>",` (trailing comma optional on
        // the last entry). Strip a trailing comma, then the surrounding quotes.
        let no_comma = trimmed.trim_end_matches(',');
        if let Some(stripped) = no_comma.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
            && !stripped.is_empty()
        {
            out.push(stripped.to_string());
        }
    }
    assert!(
        !out.is_empty(),
        "parsed zero workspace members from the root Cargo.toml — the parser is stale"
    );
    out
}

/// The `[lints]` table body of one member's `Cargo.toml`: every non-comment,
/// non-empty line between the `[lints]` header and the next top-level table
/// header (a line starting with `[` that is NOT a `[lints.*]` sub-table).
///
/// `[lints.clippy]` / `[lints.rust]` sub-table headers ARE part of the lints
/// table and are returned as body lines so the caller can flag them as
/// divergence — an own-table override is exactly what this guard refuses.
fn lints_table_body(toml: &str) -> Option<String> {
    // Find the `[lints]` header line (exact — `[lints.clippy]` must NOT match
    // the bare-table header scan). A line is the header iff it trims to
    // exactly `[lints]`.
    let mut lines = toml.lines();
    let mut found = false;
    for line in &mut lines {
        if line.trim() == "[lints]" {
            found = true;
            break;
        }
    }
    if !found {
        return None;
    }
    let mut body = String::new();
    for line in lines {
        let trimmed = line.trim();
        // The next top-level table header ends the lints table. A
        // `[lints.<sub>]` header is part of the lints table (it is an
        // override and must be surfaced); any other `[<table>]` header ends
        // the scan.
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if trimmed.starts_with("[lints.") {
                body.push_str(trimmed);
                body.push('\n');
                continue;
            }
            break;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        body.push_str(trimmed);
        body.push('\n');
    }
    Some(body)
}

/// The uniformity verdict for one member's `[lints]` table.
///
/// `Ok(())` — the table is exactly `[lints] workspace = true`, no overrides.
/// `Err(reason)` — the table is missing, bare (opted out), or divergent (an
/// override sub-table or an extra key alongside `workspace = true`).
fn lint_uniformity_verdict(toml: &str) -> Result<(), String> {
    let Some(body) = lints_table_body(toml) else {
        return Err("no [lints] table — the crate opts out of the workspace lint policy".into());
    };
    if body.is_empty() {
        return Err(
            "bare [lints] table (header only, no `workspace = true`) — the crate opts out of \
             the workspace lint policy"
                .into(),
        );
    }
    let lines: Vec<&str> = body.lines().collect();
    // The one accepted shape: a single `workspace = true` line.
    let accepted = lines
        .iter()
        .filter(|l| {
            // `workspace = true` (with optional spaces) is the only accepted key.
            l.replace(' ', "") == "workspace=true"
        })
        .count();
    if accepted == 1 && lines.len() == 1 {
        return Ok(());
    }
    // Anything else — an override sub-table (`[lints.clippy]`), an extra key,
    // or `workspace = false` — is a divergent own-table.
    Err(format!(
        "divergent [lints] table — expected exactly `workspace = true`, got:\n{body}",
    ))
}

use std::fmt::Write as _;

#[test]
fn every_workspace_member_inherits_the_workspace_lint_policy() {
    let root = workspace_root();
    let members = workspace_members(&root);
    let mut offenders: Vec<(String, String)> = Vec::new();
    for member in &members {
        let manifest = root.join(member).join("Cargo.toml");
        let toml = match fs::read_to_string(&manifest) {
            Ok(s) => s,
            Err(e) => {
                offenders.push((
                    member.clone(),
                    format!("Cargo.toml unreadable at {}: {e}", manifest.display()),
                ));
                continue;
            }
        };
        if let Err(reason) = lint_uniformity_verdict(&toml) {
            offenders.push((member.clone(), reason));
        }
    }
    if offenders.is_empty() {
        return;
    }
    let mut msg = String::from(
        "workspace lint-policy drift — these members do not inherit \
         `[lints] workspace = true` (the one workspace lint contract; see \
         crates/diagnostics/tests/lint_uniformity.rs and the root Cargo.toml's \
         [workspace.lints]). Fix each by replacing its [lints] table with:\n  \
         [lints]\n  workspace = true\n\nOffenders:\n",
    );
    for (member, reason) in &offenders {
        let _ = writeln!(msg, "  - {member}: {reason}");
    }
    panic!("{msg}");
}

#[test]
fn lints_table_body_finds_the_bare_header() {
    // The accepted shape.
    assert!(lint_uniformity_verdict("[lints]\nworkspace = true\n").is_ok());
}

#[test]
fn lints_table_body_flags_a_bare_header() {
    // The 8 divergent crates' current shape: header only.
    assert!(lint_uniformity_verdict("[lints]\n").is_err());
    assert!(lint_uniformity_verdict("[lints]\n\n[features]\n").is_err());
}

#[test]
fn lints_table_body_flags_an_override_subtable() {
    // A divergent own-table: a [lints.clippy] override.
    let divergent =
        "[lints]\nworkspace = true\n[lints.clippy]\nneedless_pass_by_value = \"allow\"\n";
    assert!(lint_uniformity_verdict(divergent).is_err());
}

#[test]
fn lints_table_body_flags_a_missing_table() {
    assert!(lint_uniformity_verdict("[package]\nname = \"x\"\n").is_err());
}
