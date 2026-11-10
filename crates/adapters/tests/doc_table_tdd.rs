//! TDD suite for the generated capability-matrix docs table (E11's
//! `doc_table_matches_matrix` seam, spec §v0.2-draft E11).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//! The seam: `capability_markdown_table()` renders the §18 matrix as a
//! deterministic markdown table, and the crate README embeds that exact
//! output — the docs and the function cannot disagree.

use oxide_fw_adapters::{Capability, FeatureSupport, Target, capability_markdown_table, support};

/// The E11 red-first pin: the table embedded in the crate README is the
/// emitter's exact output. A matrix change that forgets the docs is a red
/// test, not silent doc rot (the same shape the schema drift gate uses).
#[test]
fn doc_table_matches_the_function() {
    let readme = include_str!("../README.md");
    let generated = capability_markdown_table();
    assert!(
        readme.contains(&generated),
        "the README's capability table has drifted from \
         capability_markdown_table() — regenerate and re-embed the exact output"
    );
}

#[test]
fn doc_table_is_deterministic() {
    // The emitter is the docs source of truth: identical bytes on every
    // call (the E11 generator and any future docs job rely on it).
    assert_eq!(capability_markdown_table(), capability_markdown_table());
}

#[test]
fn doc_table_shape_covers_every_target_and_capability() {
    // Header: `| target |` then every capability id in declaration order.
    // Rows: one per target, in Target::ALL order. Separator in between.
    let table = capability_markdown_table();
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(
        lines.len(),
        2 + Target::ALL.len(),
        "header + separator + one row per target: {table}"
    );

    let header_cells: Vec<&str> = lines[0]
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect();
    let expected_cells: Vec<&str> = std::iter::once("target")
        .chain(Capability::ALL.iter().map(Capability::as_str))
        .collect();
    assert_eq!(header_cells, expected_cells, "header: {}", lines[0]);

    let separator = lines[1];
    assert!(
        separator.starts_with("| --- |") && separator.ends_with('|'),
        "GFM separator after the header: {separator}"
    );

    for (line, t) in lines[2..].iter().zip(Target::ALL) {
        let mut cells = line.trim_matches('|').split('|').map(str::trim);
        assert_eq!(
            cells.next(),
            Some(t.as_str()),
            "row target is `{}`: {line}",
            t.as_str()
        );
        for (cell, c) in cells.zip(Capability::ALL) {
            assert!(
                matches!(cell, "F" | "D" | "U"),
                "cell `{cell}` is F/D/U on {} × {}: {line}",
                t.as_str(),
                c.as_str()
            );
        }
    }
}

#[test]
fn doc_table_cells_equal_support() {
    // The emitter consults the matrix, never a second opinion: every cell
    // of the rendered row equals `support(target, capability)` for the
    // same coordinates.
    for t in Target::ALL {
        let table = capability_markdown_table();
        let row = table
            .lines()
            .find(|l| l.starts_with(&format!("| {} |", t.as_str())))
            .unwrap_or_else(|| panic!("row for {} present", t.as_str()));
        let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
        for (cell, c) in cells[1..].iter().zip(Capability::ALL) {
            let expected = match support(*t, *c) {
                FeatureSupport::Full => "F",
                FeatureSupport::Degraded => "D",
                FeatureSupport::Unsupported => "U",
            };
            assert_eq!(
                *cell,
                expected,
                "{} × {} rendered `{cell}`, support says `{expected}`",
                t.as_str(),
                c.as_str()
            );
        }
    }
}

#[test]
fn doc_table_keeps_the_v01_golden_cells() {
    // Spot-pins the D39-protected rows: the static target refuses live and
    // rust fns (the §18 config-key refusal's cells), ships search degraded
    // (static JSON), and the binary target is full on all three — the
    // same cells `matrix_v2_tdd` pins through `support`.
    let table = capability_markdown_table();
    let static_row = table
        .lines()
        .find(|l| l.starts_with("| static |"))
        .expect("static row");
    let live_col = 1 + Capability::ALL
        .iter()
        .position(|c| c.as_str() == "live")
        .expect("live column");
    let rust_col = 1 + Capability::ALL
        .iter()
        .position(|c| c.as_str() == "rust-fns")
        .expect("rust-fns column");
    let search_col = 1 + Capability::ALL
        .iter()
        .position(|c| c.as_str() == "search")
        .expect("search column");
    let cells: Vec<&str> = static_row
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect();
    assert_eq!(cells[live_col], "U", "static × live: {static_row}");
    assert_eq!(cells[rust_col], "U", "static × rust-fns: {static_row}");
    assert_eq!(cells[search_col], "D", "static × search: {static_row}");
}
