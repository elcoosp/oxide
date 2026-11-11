//! The V2-E11 generated-reference seam (spec v0.2 §V2-E11): the reference
//! pages `oxide docs generate` writes, rendered from the crates the CLI
//! binary links — never from hand-maintained copies.
//!
//! The seam is a registry of [`ReferenceGenerator`]s. Each generator owns
//! one page under `docs/reference/`, renders it deterministically
//! (identical bytes on every call, the property the `--check` gate and the
//! `tests/docs_tdd.rs` pins hold together), and pulls its content from the
//! single source of truth the crate already ships:
//!
//! - [`CapabilitiesPage`] — a fold of
//!   [`oxide_fw_adapters::capability_markdown_table`] plus the documented
//!   degradation reasons (the matrix stays the single source of truth; the
//!   page is a consumer, never a second opinion — the recorded V2-R02
//!   rule, applied to docs).
//! - [`ConfigPage`] — a walk of [`oxide_fw_config::config_schema()`], one
//!   row per key, refusing with `DOC-001` when a key carries no
//!   description (the E11 acceptance: "adding a config key without a
//!   schema description fails `docs-check`").
//! - [`CliPage`] — a walk of the live `clap` command tree, so a new
//!   command or flag without a page regeneration is a red test.
//!
//! The command half lives in `cmd_docs` (lib.rs): `oxide docs generate`
//! writes the pages, `--check` compares the on-disk bytes and reports
//! `DOC-003` findings — the gate the CI `docs-check` job runs.

use std::fmt::Write as _;
use std::path::Path;

use oxide_fw_adapters::{
    Capability, FeatureSupport, Target, capability_markdown_table, degradation, support,
};
use oxide_fw_diagnostics::{Diagnostic, OxideError};

type Result<T> = std::result::Result<T, OxideError>;

/// One rendered reference page: a stable path under `docs/reference/` and
/// its deterministic bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedPage {
    /// The file name under `docs/reference/` (e.g. `capabilities.md`).
    pub path: &'static str,
    /// The page bytes (a UTF-8 markdown document).
    pub content: String,
}

/// One `docs/reference/` page generator: a stable path and a
/// deterministic render (spec v0.2 §V2-E11).
///
/// Implemented by the page structs in this module ([`CapabilitiesPage`],
/// [`ConfigPage`], [`CliPage`]) and registered in [`generators`]. A
/// fixture implementing this trait is also the seam's test vocabulary —
/// the same shape the `ConfigFeatureReport` seam uses.
pub trait ReferenceGenerator: Sync {
    /// The file name the page writes under `docs/reference/`.
    #[must_use]
    fn path(&self) -> &'static str;

    /// Render the page. Deterministic: identical bytes on every call for
    /// a given set of crate versions — `--check` compares bytes, so any
    /// nondeterminism would flip the gate run to run.
    ///
    /// # Errors
    ///
    /// [`OxideError`] with a stable `DOC-*` code when the sources the
    /// page renders from are themselves incomplete — `DOC-001` when a
    /// config key carries no schema description (the E11 acceptance).
    fn render(&self) -> Result<String>;
}

static CAPABILITIES_PAGE: CapabilitiesPage = CapabilitiesPage;
static CLI_PAGE: CliPage = CliPage;
static CONFIG_PAGE: ConfigPage = ConfigPage;
static ERRORS_PAGE: ErrorsPage = ErrorsPage;

/// The registry slice itself, so `generators` hands out a true
/// `&'static` (a body-literal array would be a temporary).
static GENERATORS: &[&dyn ReferenceGenerator] =
    &[&CAPABILITIES_PAGE, &CLI_PAGE, &CONFIG_PAGE, &ERRORS_PAGE];

/// The registry, in write order (alphabetical by page path, so the
/// command's output and the directory listing agree).
#[must_use]
pub fn generators() -> &'static [&'static dyn ReferenceGenerator] {
    GENERATORS
}

/// Render every registered page (the `oxide docs generate` payload).
///
/// # Errors
///
/// Propagates the first generator error (`DOC-001` — a config key
/// without a schema description). The error names every offending key,
/// so the failure is the work list.
pub fn generate_all() -> Result<Vec<GeneratedPage>> {
    generators()
        .iter()
        .map(|g| {
            Ok(GeneratedPage {
                path: g.path(),
                content: g.render()?,
            })
        })
        .collect()
}

/// The `--check` gate: render every page and compare against the bytes on
/// disk under `dir`. Returns one `DOC-003` [`Diagnostic`] per stale or
/// missing page (empty — clean — is the CI `docs-check` pass condition).
///
/// Read-only against the filesystem — the gate can run anywhere (CI,
/// tests, a contributor's machine) without touching the tree.
///
/// # Errors
///
/// Propagates the render errors (`DOC-001` — an undescribed config key):
/// a page that cannot render cannot be checked, and the render error is
/// the more actionable finding.
pub fn check(dir: &Path) -> Result<Vec<Diagnostic>> {
    let mut findings = Vec::new();
    for g in generators() {
        let path = dir.join(g.path());
        let fresh = g.render()?;
        let stale = match std::fs::read_to_string(&path) {
            Ok(on_disk) => on_disk != fresh,
            Err(_) => true,
        };
        if stale {
            findings.push(
                Diagnostic::error(
                    "DOC-003",
                    format!(
                        "`{}` is missing or stale against the running binary",
                        path.display()
                    ),
                )
                .with_fix(DOC_003_FIX),
            );
        }
    }
    Ok(findings)
}

const DOC_003_FIX: &str = "regenerate with `cargo run -p oxide-fw-cli -- docs generate`";

const DOC_001_FIX: &str =
    "add a `description` in `crates/config/src/schema.rs` (E11: every config key documents itself)";

const BANNER: &str = "<!-- GENERATED FILE — `oxide docs generate` (spec v0.2 §V2-E11). DO NOT EDIT.\n\
     Regenerate: cargo run -p oxide-fw-cli -- docs generate\n\
     Drift gate: crates/cli/tests/docs_tdd.rs -->\n\n";

/// The markdown cell text with GFM pipes escaped, so help strings that
/// contain `|` cannot break the table they land in.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

// ---------------------------------------------------------------------------
// capabilities.md
// ---------------------------------------------------------------------------

/// The §18 capability matrix page: the generated table plus every
/// documented degradation reason (a fold over the adapters crate — the
/// matrix is the single source of truth; this page never restates a cell).
pub struct CapabilitiesPage;

impl ReferenceGenerator for CapabilitiesPage {
    fn path(&self) -> &'static str {
        "capabilities.md"
    }

    fn render(&self) -> Result<String> {
        let mut out = String::from(BANNER);
        let _ = writeln!(out, "# Capability matrix\n");
        let _ = writeln!(
            out,
            "What each deploy target honours for every capability (spec §18). \
             `F` full · `D` degraded (the reason is in the table below) · \
             `U` unsupported — the §18 refusal rule fails the build at packaging time."
        );
        let _ = writeln!(
            out,
            "Generated from `oxide_fw_adapters::capability_markdown_table()`; \
             the crate README embeds the same bytes."
        );
        out.push('\n');
        out.push_str(&capability_markdown_table());
        out.push_str("\n## Degraded cells\n\n");
        let _ = writeln!(
            out,
            "Every `D` above with its documented reason \
             (`oxide_fw_adapters::degradation` — a `D` cell without a reason is a red pin)."
        );
        out.push('\n');
        out.push_str("| target | capability | reason |\n");
        out.push_str("| --- | --- | --- |\n");
        for t in Target::ALL {
            for c in Capability::ALL {
                if support(*t, *c) == FeatureSupport::Degraded {
                    let reason = degradation(*t, *c).unwrap_or("no documented degradation reason");
                    let _ = writeln!(
                        out,
                        "| {} | {} | {} |",
                        t.as_str(),
                        c.as_str(),
                        cell(reason)
                    );
                }
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// config.md
// ---------------------------------------------------------------------------

/// The `oxide.config.ts` reference: every key of
/// [`oxide_fw_config::config_schema()`] with its type, default and — the
/// E11 acceptance — its description. A key without a description refuses
/// the render (`DOC-001`).
pub struct ConfigPage;

/// One flattened schema row: a dotted key path (`build.sourcemap`,
/// `content.collections[].dir`) with its type, default and description.
struct SchemaRow {
    path: String,
    kind: String,
    default: Option<String>,
    description: Option<String>,
}

impl ReferenceGenerator for ConfigPage {
    fn path(&self) -> &'static str {
        "config.md"
    }

    fn render(&self) -> Result<String> {
        let schema = oxide_fw_config::config_schema();
        let mut rows = Vec::new();
        let mut missing = Vec::new();
        if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
            let mut names: Vec<&str> = props.keys().map(String::as_str).collect();
            names.sort_unstable();
            for name in names {
                walk_schema(name, &props[name], &mut rows, &mut missing);
            }
        }
        if !missing.is_empty() {
            return Err(OxideError::from(
                Diagnostic::error(
                    "DOC-001",
                    format!(
                        "{} config keys carry no schema description: {}",
                        missing.len(),
                        missing.join(", ")
                    ),
                )
                .with_fix(DOC_001_FIX),
            ));
        }
        render_config_page(&rows)
    }
}

/// Walk one schema node into flattened [`SchemaRow`]s.
///
/// Branch objects (a node with `properties`) contribute a row only when
/// they carry their own description, then recurse; arrays of objects
/// recurse with a `[]` segment; leaves contribute exactly one row, and a
/// leaf without a non-empty description lands in `missing` (the DOC-001
/// work list).
fn walk_schema(
    prefix: &str,
    node: &serde_json::Value,
    rows: &mut Vec<SchemaRow>,
    missing: &mut Vec<String>,
) {
    let Some(obj) = node.as_object() else { return };
    let description = obj
        .get("description")
        .and_then(serde_json::Value::as_str)
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    if let Some(props) = obj.get("properties").and_then(serde_json::Value::as_object) {
        if let Some(desc) = description {
            rows.push(SchemaRow {
                path: prefix.to_string(),
                kind: "object".to_string(),
                default: None,
                description: Some(desc),
            });
        }
        let mut names: Vec<&str> = props.keys().map(String::as_str).collect();
        names.sort_unstable();
        for name in names {
            let path = format!("{prefix}.{name}");
            walk_schema(&path, &props[name], rows, missing);
        }
        return;
    }
    if let Some(items) = obj.get("items")
        && let Some(props) = items
            .get("properties")
            .and_then(serde_json::Value::as_object)
    {
        if let Some(desc) = description {
            rows.push(SchemaRow {
                path: prefix.to_string(),
                kind: "array".to_string(),
                default: None,
                description: Some(desc),
            });
        }
        let mut names: Vec<&str> = props.keys().map(String::as_str).collect();
        names.sort_unstable();
        for name in names {
            let path = format!("{prefix}[].{name}");
            walk_schema(&path, &props[name], rows, missing);
        }
        return;
    }
    // Leaf: the description is REQUIRED (the E11 acceptance).
    if description.is_none() {
        missing.push(prefix.to_string());
    }
    rows.push(SchemaRow {
        path: prefix.to_string(),
        kind: schema_type(node),
        default: obj
            .get("default")
            .map(|d| d.to_string())
            .filter(|d| d != "null"),
        description,
    });
}

/// The human type of a schema node: the JSON type, the enum values when
/// the node is an enum, or the item type for arrays.
fn schema_type(node: &serde_json::Value) -> String {
    let Some(obj) = node.as_object() else {
        return "any".to_string();
    };
    let base = match obj.get("type").and_then(serde_json::Value::as_str) {
        Some(t) => t.to_string(),
        None => return "any".to_string(),
    };
    if let Some(values) = obj.get("enum").and_then(serde_json::Value::as_array) {
        let list: Vec<String> = values.iter().map(|v| format!("`{v}`")).collect();
        return format!("enum: {}", list.join(" \\| "));
    }
    if base == "array" {
        let item = obj
            .get("items")
            .and_then(|i| i.get("type"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("object");
        return format!("{item}[]");
    }
    base
}

/// Lay the flattened rows out: one `##` section per top-level key (the
/// rows arrive grouped because the walk recurses depth-first in sorted
/// order), leaves as bullets, branches as tables.
fn render_config_page(rows: &[SchemaRow]) -> Result<String> {
    let mut out = String::from(BANNER);
    let _ = writeln!(out, "# `oxide.config.ts` reference\n");
    let _ = writeln!(
        out,
        "Generated from `oxide_fw_config::config_schema()` — the §19.3 JSON Schema \
         editors consume via `$schema`. One row per key; the `default` column shows \
         the schema default when one exists."
    );
    let _ = writeln!(
        out,
        "A key without a description refuses the render (`DOC-001`) — the E11 \
         acceptance: adding a config key without a schema description fails \
         `docs-check`."
    );
    out.push_str("\n## Keys\n\n");
    let mut section = String::new();
    for row in rows {
        let top = row.path.split('.').next().unwrap_or(&row.path);
        if top != section {
            section = top.to_string();
            let _ = writeln!(out, "### `{section}`\n");
        }
        let desc = row.description.as_deref().unwrap_or("");
        let default = row
            .default
            .as_deref()
            .map(|d| format!("`{}`", cell(d)))
            .unwrap_or_else(|| "—".to_string());
        let _ = writeln!(
            out,
            "- `{}` — `{}` — default {default}: {}",
            row.path,
            row.kind,
            cell(desc)
        );
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// errors.md — the Appendix V2.K registry
// ---------------------------------------------------------------------------

/// The embedded registry: `docs/errors.toml` at the repo root — the file
/// Appendix V2.K names as the single source of truth for stable codes.
/// Embedded (not read at runtime) so the page render stays pure and the
/// binary documents the registry of the exact crates it was built from.
pub const ERRORS_TOML: &str = include_str!("../../../docs/errors.toml");

/// One registry entry (the Appendix V2.K entry format: `[CODE]` with a
/// `message`, an optional `fix`, and `spec_only` for codes whose work
/// package has not landed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorEntry {
    /// The stable code (`AST-LIV-001`, `RTR-110`, ...).
    pub code: String,
    /// The human meaning of the code.
    pub message: String,
    /// The documented remedy, when one exists.
    pub fix: Option<String>,
    /// true when the code is defined by the spec for a work package that
    /// has not landed (the coverage gate does not demand it in source).
    pub spec_only: bool,
}

/// The `DOC-002` diagnostic: the registry file carries an entry the
/// strict format rejects, with the 1-based line number.
fn doc_002(line: usize, text: &str) -> OxideError {
    OxideError::from(
        Diagnostic::error("DOC-002", format!("docs/errors.toml line {line}: {text}"))
            .with_fix(DOC_002_FIX),
    )
}

const DOC_002_FIX: &str =
    "fix the entry — the format is `[CODE]` + `message` / `fix` / `spec_only`";

/// Unescape a basic-string body (the escapes the registry emits: `\"`,
/// `\\`, `\n`, `\t`; anything else is rejected — the file is authored,
/// not machine-translated, so strictness costs nothing).
fn unescape(body: &str, line: usize) -> Result<String> {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            other => {
                return Err(doc_002(
                    line,
                    &format!(
                        "unsupported escape `\\{}` in a registry string",
                        other.map(String::from).unwrap_or_default()
                    ),
                ));
            }
        }
    }
    Ok(out)
}

/// Parse one `key = value` line into its parts, or `None` when the line
/// is not a well-formed assignment (the caller turns that into DOC-002).
/// The raw value is returned as written: `"...\"...\"..."` strings keep
/// their quotes, bare booleans arrive as `true`/`false`.
fn parse_kv(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((key.to_string(), value.trim().to_string()))
}

/// A string value: the `"..."` body between the quotes, or `None`.
fn string_value(raw: &str) -> Option<String> {
    raw.strip_prefix('"')?.strip_suffix('"').map(str::to_string)
}

/// Parse the registry (the strict Appendix V2.K entry format): `[CODE]`
/// bare-key tables with `message`, optional `fix`, optional `spec_only`
/// boolean; `#` comments and blank lines allowed. File order is kept.
///
/// # Errors
///
/// [`OxideError`] with the `DOC-002` code and the offending line number
/// for: a header that is not a bare `[CODE]` key, a key outside a table,
/// an unknown key, a non-string value, an unsupported escape, an entry
/// without a `message`, or a duplicate code.
pub fn parse_registry(src: &str) -> Result<Vec<ErrorEntry>> {
    let mut entries: Vec<ErrorEntry> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut current: Option<ErrorEntry> = None;
    for (idx, raw) in src.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let lineno = idx + 1;
        if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            let code = inner.trim();
            let bare = !code.is_empty()
                && code.starts_with(|c: char| c.is_ascii_uppercase())
                && code
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-');
            if !bare {
                return Err(doc_002(
                    lineno,
                    "the table header is not a bare `[CODE]` key",
                ));
            }
            if seen.iter().any(|s| s == code) {
                return Err(doc_002(
                    lineno,
                    &format!("duplicate registry entry `{code}`"),
                ));
            }
            seen.push(code.to_string());
            current = Some(ErrorEntry {
                code: code.to_string(),
                message: String::new(),
                fix: None,
                spec_only: false,
            });
            continue;
        }
        let Some((key, raw)) = parse_kv(line) else {
            return Err(doc_002(lineno, "expected `key = \"value\"`"));
        };
        let Some(entry) = current.as_mut() else {
            return Err(doc_002(lineno, "a key appeared outside a `[CODE]` table"));
        };
        match key.as_str() {
            "message" => {
                let Some(body) = string_value(&raw) else {
                    return Err(doc_002(lineno, "`message` must be a `\"...\"` string"));
                };
                entry.message = unescape(&body, lineno)?;
            }
            "fix" => {
                let Some(body) = string_value(&raw) else {
                    return Err(doc_002(lineno, "`fix` must be a `\"...\"` string"));
                };
                entry.fix = Some(unescape(&body, lineno)?);
            }
            "spec_only" => match raw.as_str() {
                "true" => entry.spec_only = true,
                "false" => entry.spec_only = false,
                _ => {
                    return Err(doc_002(
                        lineno,
                        "`spec_only` must be the bare `true` or `false`",
                    ));
                }
            },
            _ => return Err(doc_002(lineno, &format!("unknown registry key `{key}`"))),
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry);
    }
    if let Some(entry) = entries.iter().find(|e| e.message.is_empty()) {
        return Err(doc_002(
            1,
            &format!("`{}` carries no `message`", entry.code),
        ));
    }
    Ok(entries)
}

/// The error-code registry page: `docs/errors.toml` rendered as the
/// reference table (Appendix V2.K — "E11 generates `errors.md` from it").
/// Landed codes first (the coverage gate holds them against the source
/// tree), then the spec-only codes whose work packages have not landed.
pub struct ErrorsPage;

impl ReferenceGenerator for ErrorsPage {
    fn path(&self) -> &'static str {
        "errors.md"
    }

    fn render(&self) -> Result<String> {
        let entries = parse_registry(ERRORS_TOML)?;
        let mut out = String::from(BANNER);
        let _ = writeln!(out, "# Error code registry\n");
        let _ = writeln!(
            out,
            "Every stable diagnostic code the tool can emit (Appendix V2.K). \
             Generated from `docs/errors.toml` — the registry file, not the \
             call sites; `crates/cli/tests/errors_registry_tdd.rs` is the gate \
             that fails when a production call site raises a code this page \
             does not carry."
        );
        out.push_str("\n## Codes\n\n");
        out.push_str("| code | message | fix |\n| --- | --- | --- |\n");
        for entry in &entries {
            if entry.spec_only {
                continue;
            }
            let fix = entry
                .fix
                .as_deref()
                .map(cell)
                .unwrap_or_else(|| "—".to_string());
            let _ = writeln!(
                out,
                "| `{}` | {} | {fix} |",
                entry.code,
                cell(&entry.message)
            );
        }
        let spec: Vec<&ErrorEntry> = entries.iter().filter(|e| e.spec_only).collect();
        if !spec.is_empty() {
            out.push_str("\n## Spec-only codes\n\n");
            let _ = writeln!(
                out,
                "Defined by the spec for work packages that have not landed; \
                 no call site raises them yet."
            );
            out.push_str("\n| code | message | fix |\n| --- | --- | --- |\n");
            for entry in spec {
                let fix = entry
                    .fix
                    .as_deref()
                    .map(cell)
                    .unwrap_or_else(|| "—".to_string());
                let _ = writeln!(
                    out,
                    "| `{}` | {} | {fix} |",
                    entry.code,
                    cell(&entry.message)
                );
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// cli.md
// ---------------------------------------------------------------------------

/// The CLI reference: the live `clap` command tree — every command, its
/// about line and every documented argument. Generated from
/// `Cli::command()`, so a command or flag without a page regeneration is
/// a red pin (`tests/docs_tdd.rs` is the gate).
pub struct CliPage;

impl ReferenceGenerator for CliPage {
    fn path(&self) -> &'static str {
        "cli.md"
    }

    fn render(&self) -> Result<String> {
        use clap::CommandFactory as _;
        let cmd = crate::Cli::command();
        let mut out = String::from(BANNER);
        let _ = writeln!(out, "# CLI reference\n");
        let _ = writeln!(
            out,
            "Generated from the `oxide-fw-cli` clap command tree — the same \
             about/flag text `oxide --help` prints."
        );
        out.push_str("\n## Commands\n");
        for sub in cmd.get_subcommands() {
            if sub.is_hide_set() {
                continue;
            }
            out.push('\n');
            let _ = writeln!(out, "### `oxide {}`\n", sub.get_name());
            if let Some(about) = sub.get_about() {
                let _ = writeln!(out, "{}\n", about.to_string().trim());
            }
            let args: Vec<_> = sub
                .get_arguments()
                .filter(|a| !a.is_hide_set() && a.get_id() != "help" && a.get_id() != "version")
                .collect();
            if args.is_empty() {
                continue;
            }
            out.push_str("| argument | description |\n| --- | --- |\n");
            for a in &args {
                let help = a
                    .get_long_help()
                    .or_else(|| a.get_help())
                    .map(|h| cell(h.to_string().trim()))
                    .unwrap_or_default();
                let _ = writeln!(out, "| {} | {help} |", arg_label(a));
                out.push_str(&enum_values(a));
            }
        }
        Ok(out)
    }
}

/// The rendered left-hand cell for one clap argument: the long flag with
/// its value name for options, the id for positionals, the short flag
/// prefixed when one exists.
fn arg_label(a: &clap::builder::Arg) -> String {
    if a.is_positional() {
        return format!("`<{}>`", a.get_id());
    }
    let value = a
        .get_value_names()
        .and_then(|names| names.first())
        .map(|n| format!(" <{n}>"))
        .unwrap_or_default();
    match (a.get_short(), a.get_long()) {
        (_, Some(long)) => {
            format!("`--{long}{value}`")
        }
        (Some(short), None) => format!("`-{short}{value}`"),
        (None, None) => format!("`{}`", a.get_id()),
    }
}

/// The possible-value rows for a `ValueEnum` argument (the `op`
/// subcommands, the metric names) — an empty string when the argument
/// takes free-form values. The rows continue the command's argument
/// table: one row per variant, description = the variant's help.
fn enum_values(a: &clap::builder::Arg) -> String {
    let values = a.get_possible_values();
    if values.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for v in values {
        if v.is_hide_set() {
            continue;
        }
        let help = v
            .get_help()
            .map(|h| cell(h.to_string().trim()))
            .unwrap_or_default();
        let _ = writeln!(out, "| `{} = {}` | {help} |", a.get_id(), v.get_name());
    }
    out
}
