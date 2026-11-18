//! The diagnostics walkthrough: the one error shape every Oxide crate
//! speaks (spec §19.4) — a stable code, a severity, a source-mapped code
//! frame, the cause chain, and a fix hint when one is known, serialized
//! as JSON over the JS boundary.
//!
//! The walkthrough composes the whole builder chain the way a subsystem
//! does it in production (`crates/cache/src/lib.rs`'s `err()` helper is
//! the smallest consumer, `crates/server`'s `SRV-*` refusals the coded
//! end):
//!
//! 1. build a diagnostic with the stable `CAC-*`-style code and message;
//! 2. attach the source location and render the code frame from the
//!    module source (`render_code_frame` — the `^` marker under the
//!    offending column, the 1-based gutter);
//! 3. append the cause chain and the known fix;
//! 4. render it (the `Display` shape an operator reads) and round-trip it
//!    through JSON (the wire shape a worker or the browser overlay
//!    receives);
//! 5. wrap it in `OxideError` — the lossless conversion every crate's
//!    `Result` alias rides.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-diagnostics --example diagnostic
//! ```

use oxide_fw_diagnostics::{Diagnostic, Location, OxideError, render_code_frame};

/// The module source the diagnostic points into — a producer reads the
/// real file; the walkthrough keeps a snippet inline.
const SOURCE: &str = r"export async function loader({ params }) {
  const post = await fetchPost(params.slug);
  return { post };
}
";

fn main() {
    // 1-3. Build: code, message, location, code frame, causes, fix.
    let source_line = 2u32;
    let source_col = 28u32;
    let frame = render_code_frame(SOURCE, source_line as usize, Some(source_col as usize));
    let diagnostic = Diagnostic::error("CAC-KEY-001", "cache key derived from a mutable parameter")
        .at(Location::file(
            "routes/blog/[slug]/page.tsx",
            source_line,
            source_col,
        ))
        .with_code_frame(frame)
        .with_cause("`params.slug` is user-controlled and not part of the stable key parts")
        .with_cause("the key hash mixes route, params and build id (§11.2)")
        .with_fix("derive the key from `Key::canonical_parts` instead of the raw param");

    // 4a. The operator-facing render.
    println!("== the operator reads ==\n{diagnostic}\n");

    // 4b. The wire shape: JSON over the JS boundary, then back.
    let json = diagnostic.to_json();
    println!("== the wire carries ==\n{json}\n");
    let back = Diagnostic::from_json(&json).expect("the wire shape round-trips");
    assert_eq!(back, diagnostic, "the JSON round-trip is lossless");

    // 5. The error shape every crate's `Result` alias rides: the
    //    diagnostic converts losslessly, the code stays greppable.
    let err = OxideError::from(diagnostic);
    let rendered = err.to_string();
    assert!(rendered.starts_with("[CAC-KEY-001]"));
    println!("== the error carries ==\n{rendered}");
}
