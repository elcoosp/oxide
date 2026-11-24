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
use oxide_fw_plugin::compat_markdown_table;

type Result<T> = std::result::Result<T, OxideError>;

/// One rendered docs page: a stable path (relative to the docs root) and its
/// deterministic bytes. Most pages live under `docs/reference/`; the
/// `base` field names the subdirectory (`"reference"` or `""` for a top-level
/// docs page like `plugin-compat.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedPage {
    /// The subdirectory under the docs root (`"reference"` or `""`).
    pub base: &'static str,
    /// The file name under `base` (e.g. `capabilities.md`, `plugin-compat.md`).
    pub path: &'static str,
    /// The page bytes (a UTF-8 markdown document).
    pub content: String,
}

/// One docs page generator: a stable path and a deterministic render
/// (spec v0.2 §V2-E11). Most pages write under `docs/reference/`; a generator
/// whose page lives at the docs root overrides [`ReferenceGenerator::base`]
/// to return `""`.
///
/// Implemented by the page structs in this module ([`CapabilitiesPage`],
/// [`ConfigPage`], [`CliPage`], [`PluginCompatPage`]) and registered in
/// [`generators`]. A fixture implementing this trait is also the seam's test
/// vocabulary — the same shape the `ConfigFeatureReport` seam uses.
pub trait ReferenceGenerator: Sync {
    /// The file name the page writes under its [`base`] dir.
    #[must_use]
    fn path(&self) -> &'static str;

    /// The subdirectory under the docs root the page writes to. Defaults to
    /// `"reference"`; a top-level docs page (e.g. `plugin-compat.md`)
    /// overrides this to `""` so it writes to `docs/<path>` directly.
    #[must_use]
    fn base(&self) -> &'static str {
        "reference"
    }

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
static ENDPOINTS_PAGE: EndpointsPage = EndpointsPage;
static ERRORS_PAGE: ErrorsPage = ErrorsPage;
static PLUGIN_COMPAT_PAGE: PluginCompatPage = PluginCompatPage;

/// The registry slice itself, so `generators` hands out a true
/// `&'static` (a body-literal array would be a temporary).
static GENERATORS: &[&dyn ReferenceGenerator] = &[
    &CAPABILITIES_PAGE,
    &CLI_PAGE,
    &CONFIG_PAGE,
    &ENDPOINTS_PAGE,
    &ERRORS_PAGE,
    &PLUGIN_COMPAT_PAGE,
];

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
                base: g.base(),
                path: g.path(),
                content: g.render()?,
            })
        })
        .collect()
}

/// The `--check` gate: render every page and compare against the bytes on
/// disk. `dir` is the docs root; each page resolves to
/// `dir/<base>/<path>` (`base` is `"reference"` for the reference pages, `""`
/// for a top-level page like `plugin-compat.md`). Returns one `DOC-003`
/// [`Diagnostic`] per stale or missing page (empty — clean — is the CI
/// `docs-check` pass condition).
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
        let path = dir.join(g.base()).join(g.path());
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
// endpoints.md
// ---------------------------------------------------------------------------

/// The §V2.J endpoint map page (the E11 generator's fifth page): a fold
/// of [`oxide_fw_server::endpoint_markdown_table`] plus the plane rule
/// the map's rows embody — the server crate's map is the single source of
/// truth; the page is a consumer, never a second opinion (the
/// capability-matrix rule, applied to endpoints). A new HTTP surface that
/// joins [`oxide_fw_server::ENDPOINT_MAP`] without regenerating this page
/// is a red `--check`.
pub struct EndpointsPage;

impl ReferenceGenerator for EndpointsPage {
    fn path(&self) -> &'static str {
        "endpoints.md"
    }

    fn render(&self) -> Result<String> {
        let mut out = String::from(BANNER);
        let _ = writeln!(out, "# Endpoint map\n");
        let _ = writeln!(
            out,
            "Every HTTP surface the framework serves (Appendix V2.J), generated \
             from the `oxide_fw_server::ENDPOINT_MAP` data the server crate ships. \
             The plane rule is structural: `/__oxide/*` is the **data plane** (the \
             build-header check guards it, A04), `/_oxide/*` is the **ops plane**, \
             the dev-only surfaces are dev-token guarded (D41), and the V2-F49 \
             posture probe rides the `/-/` ops-probe convention (`/-/health` — \
             deliberately beside, not under, the framework's own `/_oxide` \
             namespace, so a platform's uniform probe-path config finds it)."
        );
        out.push_str("\n## Endpoints\n\n");
        out.push_str(&oxide_fw_server::endpoint_markdown_table());
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

// ---------------------------------------------------------------------------
// plugin-compat.md — the §V2-E05 page (the kind×hook table + the verified
// third-party plugins matrix). Top-level (not under docs/reference/), so it
// overrides `base()` to `""` and writes to `docs/plugin-compat.md`.
// ---------------------------------------------------------------------------

/// The runner's recorded results — the single source of truth for the
/// verified-plugins table (§V2-E05). Embedded (not read at runtime) so the
/// page render stays pure and the binary documents the matrix of the exact
/// build it was compiled from; `docs-check` fails on drift when the runner
/// refreshes it.
const PLUGIN_COMPAT_RESULTS: &str = include_str!("../../../conformance/plugins/results.json");

/// The `DOC-004` diagnostic: the runner's results payload is malformed (the
/// committed `conformance/plugins/results.json` is not the `{ results, matrix }`
/// shape the runner writes). A render that cannot read its source cannot be
/// checked, and the error is the work list — re-run
/// `node scripts/plugin-compat.mjs --dry-run`.
fn doc_004(msg: &str) -> OxideError {
    OxideError::from(
        Diagnostic::error(
            "DOC-004",
            format!("conformance/plugins/results.json: {msg}"),
        )
        .with_fix(DOC_004_FIX),
    )
}

const DOC_004_FIX: &str =
    "re-run `node scripts/plugin-compat.mjs --dry-run` to refresh results.json";

/// Extract the verified-plugins matrix markdown from the runner's results.
/// The runner renders the table (`report.mjs::renderMatrix`) and stores it
/// as the `matrix` field; this page embeds it verbatim — one source of truth.
fn plugin_compat_matrix() -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(PLUGIN_COMPAT_RESULTS)
        .map_err(|e| doc_004(&format!("invalid JSON — {e}")))?;
    let matrix = v
        .get("matrix")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| doc_004("missing `matrix` string field"))?;
    Ok(matrix.to_string())
}

/// The prose before the kind×hook table — verbatim from the published page
/// (header, status legend, the Kinds table, the `## Hooks` heading). The
/// table itself is [`compat_markdown_table()`], so a support flip is a code
/// change plus a regenerate, never a prose edit.
const PLUGIN_COMPAT_PRE: &str = "\
# Plugin compatibility (SPEC-15.3)

Oxide plugins come in three kinds (SPEC §15.1). This table documents which
hooks each kind supports and whether the support is wired end-to-end
(load → invoke → result consumed) or only partially implemented.

The hooks table below is **generated, not hand-maintained**:
`oxide_fw_plugin::compat_markdown_table()` renders it from the
kind × hook support model (`compat`), and
`crates/plugin/tests/compat_tdd.rs::plugin_compat_doc_embeds_the_generated_table`
pins this file to the render byte-for-byte — a support flip is a code change
plus a table re-embed, never a silent prose edit. The verified-plugins table
below is generated from `conformance/plugins/results.json` (the compat
runner's recorded output) and pinned by `docs-check` (regenerate with
`oxide docs generate`).

**Status legend:**

- ✅ **wired** — the hook is invoked by the pipeline and its result is used
  (the WASM `load`/`resolveId` rows ride the §V2-E05 CLI wire since it
  composed the host into the bundler's `BundleOptions.load_hook`/
  `resolve_hook` beside the JS bridge)
- 🟡 **partial** — the host can call the hook, but a call site is still
  missing (no cell carries this today)
- 🚫 **unsupported** — no host path. For the JS bridge — and for
  `configureServer`/`cacheHandler` on every non-native kind — declaring it
  fails at load (`PLG-HOOK`); a WASM declaration of any other hook loads but
  nothing ever invokes it
- n/a — the kind does not expose this hook

## Kinds

| Kind | What it is | Host |
|---|---|---|
| `js` | A Node ESM module exporting hook functions; runs in the tooling pool. | `plugin:load` / `plugin:call` (Node and V8 backends) |
| `wasm` | A `wasmtime` component satisfying `oxide:plugin/hooks@0.1.0`. | `oxide_plugin::WasmPlugin` |
| `native` | A built-in implementation (images, fonts, i18n, mdx). | Rust crate |

## Hooks

";

/// The prose after the verified-plugins table (the argument-shape table, the
/// route/middleware pool note, the partial-cell explanation, the testing
/// surface and the load-failure rules). Verbatim from the published page
/// except the testing-surface list now carries the V2-E05 node suite +
/// runner + dry-run consumer example.
const PLUGIN_COMPAT_POST: &str = "\
## Argument shape

JS hooks are positional on the plugin side, as in Vite. The Rust bridge sends
a named object, and the worker maps it through a documented table:

| Hook | Arguments |
|---|---|
| `transform` | `code, id` |
| `resolveId` | `specifier, from` |
| `load` | `id` |
| `renderChunk` | `file, code` |
| `generateBundle` | `files` |
| `transformHtml` | `route, html` |
| `config`, `configResolved` | `config` |
| `buildStart` | — |
| `buildEnd` | `result` |
| `handleHotUpdate` | `ctx` |
| other single-argument hooks | the one payload value |

The WASM component ABI mirrors the same shapes with canonical names
(kebab-case — the component model refuses underscore extern names):
`transform: func(code: string) -> string`,
`load: func(specifier: string) -> option<string>` and
`resolve-id: func(specifier: string, from: string) -> option<string>`.
A `none` return declines — the next plugin, or the default, wins.

## Route and middleware pools

`routes` and `serverMiddleware` run during `scan_routes`. A lazily-created
tooling pool is passed by `build`, `dev` (boot and HMR rescan), `start`/serve,
`routes`, `analyze`, and `check`. A project that declares neither hook never
starts an extra Node child.

## What \"partial\" means concretely

WASM `transform`, `load` and `resolveId` are all wired end to end: the
host speaks their ABI (`call_load_hook`/`call_resolve_hook`, the
object-safe `HookHost` seam carries both), and the §V2-E05 CLI wire
composes every loaded component into `BundleOptions.load_hook`/
`resolve_hook` alongside the JS bridge's hooks — build and dev paths both,
first non-declining answer wins, a decline falls through to the default
resolver / disk read. Native plugins are feature backends selected by config
(`images`, `fonts`, `i18n`, `mdx`, `tailwind`, `manifest`); they do not
participate in the JS hook chain.

Declaring a bridge-unsupported hook (`configureServer`, `cacheHandler`) at
load time is a **fail-at-load** today for both JS plugins and WASM manifests
(`PLG-HOOK` naming the plugin + hook + this table). A WASM component that
declares a hook its bytes do not export (or export in an uncallable shape)
**loads** — the host records the gap (`available_hooks`/`missing_hooks`,
folded into the `LoadTimeReport` the compat runner records as
`unsupportedHooks`) and the call site skips the hook it cannot invoke.

## Testing the compat surface

- `crates/plugin/tests/compat_tdd.rs` — the support matrix, the generated
  table's byte-identical embed, the WASM `load`/`resolveId`/string
  `transform` ABI end to end, and the V2-E05 load-time `unsupported_hooks` +
  `LoadTimeReport` pins
- `crates/plugin/tests/wasm_tdd.rs` — WASM hook export probing + transform
- `crates/plugin/benches/compat.rs` — the matrix fold + render cost
  (`cargo bench -p oxide-fw-plugin --features bench-util -- compat`)
- `crates/plugin/examples/compat_matrix.rs` — the walkthrough
  (`cargo run -p oxide-fw-plugin --example compat_matrix`)
- `conformance/plugins/test/*.test.mjs` — the V2-E05 node --test gate: the
  case schema, the assert evaluator, and the report parser (zero deps,
  V8-free; `node --test conformance/plugins/test/*.test.mjs`)
- `scripts/plugin-compat.mjs` — the runner (`--dry-run` evaluates recorded
  build logs V8-free; the consumer example:
  `node scripts/plugin-compat.mjs --dry-run --out -` prints the matrix)
- `crates/cli/tests.rs::build_transform_hook_*` — JS-side `transform`
- `crates/cli/src/tests.rs` — route/middleware hook fixtures and adapters
- `crates/jshost/tests/integration.rs::v8_plugin_ops_*` — V8 `plugin:load` /
  `plugin:call` parity, including positional argument order

## Plugin kinds and load failure

- A `js` plugin that declares only unsupported hooks **loads fine**; the
  pipeline just never calls it.
- A `wasm` plugin that declares a bridge-unsupported hook fails at load
  (`PLG-HOOK`); a declared-but-unexported hook loads and rides
  `missing_hooks()` until a call site would need it.
- A `native` plugin whose `name` is not a known built-in fails at config
  load (`CFG-PLUGIN`).

These rules keep a misconfigured plugin visible without breaking a build
that would have silently ignored it.
";

/// The §V2-E05 plugin-compat page: the kind×hook table (generated from the
/// compat model) plus the verified third-party plugins matrix (generated from
/// the runner's `results.json`). One source of truth, never hand-maintained:
/// `oxide docs generate` writes this file, `docs-check` fails on drift.
pub struct PluginCompatPage;

impl ReferenceGenerator for PluginCompatPage {
    fn path(&self) -> &'static str {
        "plugin-compat.md"
    }

    /// Top-level (`docs/plugin-compat.md`), not under `docs/reference/`.
    fn base(&self) -> &'static str {
        ""
    }

    fn render(&self) -> Result<String> {
        let mut out = String::new();
        out.push_str(PLUGIN_COMPAT_PRE);
        out.push_str(&compat_markdown_table());
        out.push_str("\n## Verified third-party plugins (§V2-E05)\n\n");
        out.push_str(
            "The compat runner (`scripts/plugin-compat.mjs`) installs each \
             plugin into a temp copy of its fixture, builds with `oxide build`, \
             evaluates the case's `assert` against the emitted `dist/`, and \
             records `{ status, unsupportedHooks }` from the bridge's \
             load-time report (`oxide_fw_plugin::LoadTimeReport`). The matrix \
             below is generated from `conformance/plugins/results.json` — the \
             runner's recorded output — so the published table is the exact \
             set the runner verified. Failures are documented, not hidden \
             (the §V2-E05 acceptance: at least ten third-party plugins \
             verified).\n\n",
        );
        out.push_str(&plugin_compat_matrix()?);
        out.push_str("\n\n");
        out.push_str(PLUGIN_COMPAT_POST);
        Ok(out)
    }
}
