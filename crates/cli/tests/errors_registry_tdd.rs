//! The Appendix V2.K registry pins (spec v0.2 §V2-E11: "errors.md from
//! the error code registry (Appendix V2.K, kept as `docs/errors.toml` and
//! checked against `rg 'OxideError|err("' call sites by a test")` and
//! §V2-E11's E11 generator row).
//!
//! Red-first per the house TDD rule: the coverage pin failed before
//! `docs/errors.toml` carried every production code. The pins:
//!
//! 1. [`registry_covers_every_production_code_literal`] — the call-site
//!    gate: every code-shaped string literal in production sources
//!    (`crates/**.rs`, test code excluded) has a registry row. A new
//!    `err("XXX-001", ...)` without a registry row is a red test — the
//!    same shape the spec's `rg` gate names.
//! 2. [`registry_parses_and_is_wellformed`] — the strict format holds
//!    (every entry has a code + message; the file is sorted by code, so
//!    the page and diffs stay stable).
//! 3. [`errors_page_lists_every_landed_code_with_its_message`] — the
//!    page embeds every landed code with its registry message (the
//!    registry-to-page drift gate).
//! 4. [`errors_page_marks_the_spec_only_codes`] — codes whose work
//!    packages have not landed render in their own section, visibly not
//!    raised by any call site.
//! 5. [`parser_rejects_malformed_entries`] — the strict format refuses
//!    unknown keys, non-`[CODE]` headers, keys outside tables and
//!    duplicates, each as a `DOC-002` diagnostic naming the line.
//! 6. [`errors_page_is_deterministic`] — identical bytes on re-render.

use oxide_fw_cli::docs_gen::{ErrorEntry, ErrorsPage, ReferenceGenerator, parse_registry};

/// The repo root (tests run with the crate dir as CWD).
fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is crates/cli/../..")
        .to_path_buf()
}

/// Harvest the code-shaped string literals from production sources —
/// the Rust-side twin of the spec's `rg 'OxideError|err("'` gate. Test
/// code is excluded (fixture labels are not diagnostics): `tests/`
/// targets, `src/tests.rs` modules, benches, examples and fuzz.
fn production_code_literals() -> Vec<(String, String)> {
    fn is_prod(p: &std::path::Path) -> bool {
        let s = p.to_string_lossy();
        !(s.contains("/tests/")
            || s.ends_with("/tests.rs")
            || s.contains("/benches/")
            || s.contains("/examples/")
            || s.contains("/fuzz/")
            || s.ends_with("_tdd.rs"))
    }
    let pattern = |b: &str| -> bool {
        // `[A-Z]{2,5}(-[A-Z0-9]{2,5}){1,2}` — the code shape every crate
        // uses (`ODF-104`, `ADP-IO`, `ATT-KEYFMT`, `AST-LIV-001`, `DSP-110`).
        let segs: Vec<&str> = b.split('-').collect();
        if segs.len() < 2 || segs.len() > 3 {
            return false;
        }
        segs.iter().all(|s| {
            s.len() >= 2
                && s.len() <= 8
                && s.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
    };
    let mut found: Vec<(String, String)> = Vec::new();
    let crates = repo_root().join("crates");
    let mut stack = vec![crates];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "rs") && is_prod(&path) {
                let Ok(src) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let bytes = src.as_bytes();
                let mut i = 0;
                while i < bytes.len() {
                    if bytes[i] == b'"'
                        && let Some(end) = src[i + 1..].find('"')
                    {
                        let body = &src[i + 1..i + 1 + end];
                        if pattern(body) {
                            found.push((body.to_string(), path.to_string_lossy().into_owned()));
                        }
                        i += end + 2;
                        continue;
                    }
                    i += 1;
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn registry_covers_every_production_code_literal() {
    let registry = oxide_fw_cli::docs_gen::ERRORS_TOML;
    let entries = parse_registry(registry).expect("the committed registry parses");
    let missing: Vec<String> = production_code_literals()
        .into_iter()
        .filter(|(code, _)| !entries.iter().any(|e| e.code == *code && !e.spec_only))
        .map(|(code, site)| format!("{code} (raised at {site})"))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/errors.toml is missing codes that production sources raise — \
         add a `[CODE]` table with a message (and a fix when one exists):\n{}",
        missing.join("\n")
    );
}

#[test]
fn registry_parses_and_is_wellformed() {
    let entries = parse_registry(oxide_fw_cli::docs_gen::ERRORS_TOML).expect("registry parses");
    assert!(
        entries.len() > 100,
        "the registry carries the full code inventory (got {})",
        entries.len()
    );
    let mut sorted = entries.clone();
    sorted.sort_by(|a, b| a.code.cmp(&b.code));
    let codes: Vec<&str> = entries
        .iter()
        .map(|e: &ErrorEntry| e.code.as_str())
        .collect();
    let mut sorted_codes = codes.clone();
    sorted_codes.sort_unstable();
    assert_eq!(codes, sorted_codes, "the registry file is sorted by code");
}

#[test]
fn errors_page_lists_every_landed_code_with_its_message() {
    let entries = parse_registry(oxide_fw_cli::docs_gen::ERRORS_TOML).expect("registry parses");
    let page = ErrorsPage.render().expect("errors page renders");
    for entry in entries.iter().filter(|e| !e.spec_only) {
        assert!(
            page.contains(&format!("| `{}` |", entry.code)),
            "errors.md has no row for `{}`",
            entry.code
        );
        // The page is a GFM table: a message that carries a pipe is
        // escaped (`\|`) in the cell — compare in cell form.
        let cell = entry.message.replace('|', "\\|");
        assert!(
            page.contains(&cell),
            "errors.md does not carry the registry message of `{}`",
            entry.code
        );
    }
}

#[test]
fn errors_page_marks_the_spec_only_codes() {
    let entries = parse_registry(oxide_fw_cli::docs_gen::ERRORS_TOML).expect("registry parses");
    let spec: Vec<&ErrorEntry> = entries.iter().filter(|e| e.spec_only).collect();
    assert!(
        spec.iter().any(|e| e.code == "CMP-NAT-001"),
        "the V2.K additions table's not-yet-landed codes are spec-only rows"
    );
    let page = ErrorsPage.render().expect("errors page renders");
    assert!(
        page.contains("## Spec-only codes"),
        "spec-only codes render in their own section"
    );
    for entry in spec {
        assert!(
            page.contains(&format!("| `{}` |", entry.code)),
            "errors.md has no spec-only row for `{}`",
            entry.code
        );
    }
}

#[test]
fn parser_rejects_malformed_entries() {
    let unknown = "[AST-XXX-001]\nbogus = \"nope\"\n";
    let err = parse_registry(unknown).expect_err("unknown key refuses");
    assert!(err.0.code == "DOC-002" && err.0.message.contains("line 2"));

    let header = "[lowercase-not-a-code]\nmessage = \"x\"\n";
    let err = parse_registry(header).expect_err("non-bare header refuses");
    assert!(err.0.code == "DOC-002" && err.0.message.contains("bare"));

    let stray = "message = \"x\"\n";
    let err = parse_registry(stray).expect_err("key outside a table refuses");
    assert!(err.0.code == "DOC-002" && err.0.message.contains("outside a `[CODE]` table"));

    let dup = "[AST-XXX-001]\nmessage = \"a\"\n[AST-XXX-001]\nmessage = \"b\"\n";
    let err = parse_registry(dup).expect_err("duplicate codes refuse");
    assert!(err.0.message.contains("duplicate"));

    let no_message = "[AST-XXX-001]\nfix = \"x\"\n";
    let err = parse_registry(no_message).expect_err("message-less entries refuse");
    assert!(err.0.message.contains("no `message`"));
}

#[test]
fn errors_page_is_deterministic() {
    let a = ErrorsPage.render().expect("errors page renders");
    let b = ErrorsPage.render().expect("errors page renders");
    assert_eq!(a, b, "the errors page must render identical bytes");
}
