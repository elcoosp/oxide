//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// `oxide docs generate` (§V2-E11): render the reference pages via
/// [`docs_gen`] and write them under `<root>/docs/reference` (or verify
/// the on-disk bytes with `--check`, exit 1 on drift — the CI
/// `docs-check` gate).
///
/// The `root` is canonicalized once at entry (the doubled-path rule in
/// CONVENTIONS.md) before `docs/reference` is joined onto it.
pub(crate) fn cmd_docs(
    root: &str,
    out: Option<&str>,
    check: bool,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let dir = match out {
        Some(o) => PathBuf::from(o),
        None => rootp.join("docs").join("reference"),
    };
    if check {
        let findings = docs_gen::check(&dir)?;
        if findings.is_empty() {
            outln!(
                w,
                "docs/reference is up to date ({} pages)",
                docs_gen::generators().len()
            );
            return Ok(0);
        }
        for finding in &findings {
            outln!(w, "{finding}");
        }
        outln!(
            w,
            "{} page(s) stale; run `oxide docs generate` to regenerate",
            findings.len()
        );
        return Ok(1);
    }
    for page in docs_gen::generate_all()? {
        let path = dir.join(page.path);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, page.content.as_bytes())?;
        outln!(w, "wrote {} ({} bytes)", path.display(), page.content.len());
    }
    Ok(0)
}
