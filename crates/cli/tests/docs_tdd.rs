//! The V2-E11 generated-reference pins (spec v0.2 §V2-E11: "`oxide docs
//! generate` writes `docs/reference/`" and "adding a config key without a
//! schema description fails `docs-check`").
//!
//! Red-first per the house TDD rule: this file failed to compile before
//! `docs_gen` existed, then failed on the behavior it pins (the config
//! page's DOC-001 refusal listed the schema keys that carried no
//! description). The pins:
//!
//! 1. [`capabilities_page_embeds_the_matrix_table`] — the page embeds
//!    `capability_markdown_table()` byte-identical (the E11
//!    `doc_table_matches_matrix` gate, extended to the docs/reference
//!    page).
//! 2. [`capabilities_page_lists_every_degraded_cell_with_its_reason`] —
//!    every `D` cell in the matrix appears with its documented
//!    degradation reason (a fold of `support` × `degradation`, the same
//!    single-source-of-truth rule the refusal seam follows).
//! 3. [`config_page_covers_every_schema_leaf_with_a_description`] — the
//!    E11 acceptance: rendering the config page fails with `DOC-001`
//!    naming every key that lacks a schema description; with all keys
//!    described it renders one row per leaf.
//! 4. [`config_page_is_deterministic`] / [`cli_page_is_deterministic`] —
//!    identical bytes on every render (docs-check compares bytes).
//! 5. [`cli_page_covers_every_top_level_command`] and
//!    [`cli_page_covers_every_documented_argument`] — the page is
//!    generated from the live clap tree, so a new command or flag
//!    without a page regeneration is a red test.
//! 6. [`page_registry_lists_the_reference_pages`] — the registry order
//!    the command writes (and `docs/reference/` holds) is stable.
//! 7. [`check_passes_on_the_committed_pages`] and
//!    [`check_reports_drift_when_a_page_is_stale`] — the `--check` gate
//!    the CI `docs-check` job runs: empty findings against the committed
//!    pages, `DOC-003` findings against a stale one.

use oxide_fw_adapters::{
    Capability, FeatureSupport, Target, capability_markdown_table, degradation, support,
};
use oxide_fw_cli::docs_gen::{self, CapabilitiesPage, CliPage, ConfigPage, ReferenceGenerator};
use oxide_fw_config::config_schema;

/// The repo root, resolved from the crate dir (tests run with the crate
/// root as CWD, so `CARGO_MANIFEST_DIR/../..` is the workspace root).
fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is crates/cli/../..")
        .to_path_buf()
}

// ---------------------------------------------------------------------------
// capabilities.md
// ---------------------------------------------------------------------------

#[test]
fn capabilities_page_embeds_the_matrix_table() {
    let page = CapabilitiesPage
        .render()
        .expect("capabilities page renders");
    let table = capability_markdown_table();
    assert!(
        page.contains(&table),
        "capabilities.md must embed capability_markdown_table() byte-identical \
         (the E11 drift gate); page:\n{page}"
    );
}

#[test]
fn capabilities_page_lists_every_degraded_cell_with_its_reason() {
    let page = CapabilitiesPage
        .render()
        .expect("capabilities page renders");
    // Every Degraded cell in the §18 matrix must surface its documented
    // reason — the same "the matrix is the single source of truth" rule
    // the refusal seam follows: the page is a fold over `support` and
    // `degradation`, never a second opinion.
    let mut missing = Vec::new();
    for t in Target::ALL {
        for c in Capability::ALL {
            if support(*t, *c) == FeatureSupport::Degraded {
                let reason = degradation(*t, *c).unwrap_or("no documented degradation reason");
                if !page.contains(reason) {
                    missing.push(format!("{} x {}: {reason}", t.as_str(), c.as_str()));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "capabilities.md is missing degraded-cell reasons:\n{}",
        missing.join("\n")
    );
}

// ---------------------------------------------------------------------------
// config.md
// ---------------------------------------------------------------------------

/// Collect every leaf path of the schema (`build.sourcemap`,
/// `content.collections[].dir`, ...) the way the generator does.
fn schema_leaf_paths() -> Vec<String> {
    fn walk(prefix: &str, node: &serde_json::Value, out: &mut Vec<String>) {
        let Some(obj) = node.as_object() else { return };
        if let Some(props) = obj.get("properties").and_then(|v| v.as_object()) {
            for (name, child) in props {
                let path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                walk(&path, child, out);
            }
            return;
        }
        if let Some(items) = obj.get("items") {
            // Array of objects: the element's properties flatten with `[]`.
            if items.get("properties").is_some() {
                walk(&format!("{prefix}[]"), items, out);
                return;
            }
        }
        out.push(prefix.to_string());
    }
    let schema = config_schema();
    let mut out = Vec::new();
    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (name, child) in props {
            walk(name, child, &mut out);
        }
    }
    out.sort();
    out
}

#[test]
fn config_page_covers_every_schema_leaf_with_a_description() {
    // The E11 acceptance: "adding a config key without a schema
    // description fails docs-check". The render refuses with DOC-001 and
    // names every undescribed key, so the failure IS the work list.
    let page = ConfigPage
        .render()
        .expect("config page renders — every schema key must carry a description");
    for path in schema_leaf_paths() {
        assert!(
            page.contains(&format!("`{path}`")),
            "config.md has no row for schema key `{path}`"
        );
    }
}

#[test]
fn config_page_is_deterministic() {
    let a = ConfigPage.render().expect("config page renders");
    let b = ConfigPage.render().expect("config page renders");
    assert_eq!(a, b, "the config page must render identical bytes");
}

// ---------------------------------------------------------------------------
// cli.md
// ---------------------------------------------------------------------------

#[test]
fn cli_page_is_deterministic() {
    let a = CliPage.render().expect("cli page renders");
    let b = CliPage.render().expect("cli page renders");
    assert_eq!(a, b, "the cli page must render identical bytes");
}

#[test]
fn cli_page_covers_every_top_level_command() {
    use clap::CommandFactory as _;
    let page = CliPage.render().expect("cli page renders");
    let cmd = oxide_fw_cli::Cli::command();
    let mut missing = Vec::new();
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        let Some(about) = sub.get_about() else {
            missing.push(format!("{}: clap `about` is unset", sub.get_name()));
            continue;
        };
        let heading = format!("### `oxide {}`", sub.get_name());
        if !page.contains(&heading) {
            missing.push(format!("cli.md has no `{heading}` heading"));
            continue;
        }
        if !page.contains(about.to_string().trim()) {
            missing.push(format!(
                "cli.md does not carry the about of `{}`",
                sub.get_name()
            ));
        }
    }
    assert!(
        missing.is_empty(),
        "cli.md must cover the live clap command tree:\n{}",
        missing.join("\n")
    );
}

#[test]
fn cli_page_covers_every_documented_argument() {
    use clap::CommandFactory as _;
    let page = CliPage.render().expect("cli page renders");
    let cmd = oxide_fw_cli::Cli::command();
    let mut missing = Vec::new();
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        for arg in sub.get_arguments() {
            if arg.is_hide_set() || arg.get_id() == "help" || arg.get_id() == "version" {
                continue;
            }
            // The rendered form the page rows use: the long flag for
            // options, the `<id>` value cell for positionals.
            let rendered = match arg.get_long() {
                Some(long) => format!("--{long}"),
                None => format!("`<{}>`", arg.get_id()),
            };
            if !page.contains(&rendered) {
                missing.push(format!("{}: {rendered}", sub.get_name()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "cli.md must list every documented command argument:\n{}",
        missing.join("\n")
    );
}

// ---------------------------------------------------------------------------
// the registry + the --check gate
// ---------------------------------------------------------------------------

#[test]
fn page_registry_lists_the_reference_pages() {
    let paths: Vec<&str> = docs_gen_registry_paths();
    assert_eq!(
        paths,
        vec!["capabilities.md", "cli.md", "config.md", "errors.md"],
        "the registry order is the write order — it must be stable and sorted"
    );
}

fn docs_gen_registry_paths() -> Vec<&'static str> {
    oxide_fw_cli::docs_gen::generators()
        .iter()
        .map(|g| g.path())
        .collect()
}

#[test]
fn check_passes_on_the_committed_pages() {
    let dir = repo_root().join("docs/reference");
    let drift = docs_gen::check(&dir).expect("pages render");
    assert!(
        drift.is_empty(),
        "docs/reference is stale — regenerate with `cargo run -p oxide-fw-cli -- docs generate`:\n{}",
        drift
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn check_reports_drift_when_a_page_is_stale() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Nothing on disk: every page is reported missing with a DOC-003
    // diagnostic that carries the regeneration fix.
    let drift = docs_gen::check(tmp.path()).expect("pages render");
    assert_eq!(
        drift.len(),
        docs_gen::generators().len(),
        "an empty directory drifts on every page"
    );
    assert!(
        drift.iter().all(|d| d.code == "DOC-003"),
        "drift findings carry the DOC-003 code"
    );
}
