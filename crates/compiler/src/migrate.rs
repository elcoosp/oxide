//! The `oxide migrate` codemod planner (§V2-E03) — the plan-is-data
//! substrate the `oxide migrate [--apply]` command drives (the command
//! is the thin CLI wrapper; the engine here is the one definition site).
//!
//! The spec's shape is kept verbatim: a [`MigrationPlan`] carries
//! exactly the four data fields (`renames`, `rewrites`, `config`,
//! `manual`) so the plan is printable, serializable and testable — the
//! default command is a dry run that prints this plan, `--apply` runs
//! [`apply_plan`] behind the CLI's clean-git-tree gate.
//!
//! The four source frameworks the spec names each get a planner behind
//! the object-safe [`MigrationPlanner`] seam:
//!
//! - **`SvelteKit`** — the pure path mapping (the highest-value, safest
//!   migration): every `+`-prefixed route basename loses its marker,
//!   endpoints (`+server.ts`) become `route.ts`, a plain
//!   `hooks.server.ts` becomes the `middleware.ts` rename, a
//!   `sequence`-composed one flags manual instead, and the `$lib`
//!   alias lands as the tsconfig path entry.
//! - **Next.js (app router)** — the compat conventions key
//!   (`routes.compat = 'next'`; the router's app-dir scanner already
//!   speaks the convention), the `next/link` / `next/image` import
//!   rewrites found through [`crate::scan_import_spans`] (never regex
//!   on source), the `'use client'` files reported as island
//!   candidates without guessing, and the next.config table mappings
//!   plus the pages-router refusal as located notes.
//! - **Nuxt** — `pages/` maps into the routes root with the bracket
//!   conventions carried verbatim; `index.vue` becomes the dir's
//!   `page.vue`, a parent route component with a same-named directory
//!   demotes to the layout, and `useFetch` / `useAsyncData` flag
//!   manual with the suggested `load`.
//! - **Vite SPA** — the wrap: the existing entry becomes the `spa`
//!   route (a note names the two lines), the routes dir is explicit,
//!   nothing renamed.
//!
//! Deltas recorded in `docs/deviations.md`: the next.config table
//! values are carried as located notes (the planner does not evaluate
//! JS, so mapping `images.remotePatterns` emits the file + line and the
//! oxide spelling — carrying the value is the apply pass's
//! human-reviewed work), and `generateStaticParams` rides
//! `routes.compat: 'next'` (the compat layer accepts the Next spelling,
//! so the plan stays pure path/import data instead of a per-file
//! identifier rewrite).
//!
//! The apply writes the plan's `config` keys (the E03 apply-in-engine
//! half): a `tsconfig.`-prefixed key merges into `tsconfig.json`
//! (`tsconfig.paths.X` lands under `compilerOptions.paths.X`), a bare
//! dotted key merges into `oxide.config.ts` (the `defineConfig` /
//! `export default` object the scaffold spells — created minimal when
//! absent, existing rows never rewritten, an already-present equal
//! value re-applies as a no-op, a diverging value or an
//! unrecognizable shape stays pending loud in the report). The manual
//! notes stay pending by definition — a note is a human decision.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use oxide_fw_diagnostics::OxideError;
use serde::{Deserialize, Serialize};

/// The diagnostic code for a tree no planner can attribute (`§V2-E03`:
/// the detection surface is the package.json dependency names plus the
/// framework config files — detect never guesses).
pub const MIG_DETECT: &str = "MIG-DETECT";

/// The diagnostic code for a plan/apply filesystem failure (a fixture
/// file vanished mid-plan, a rename crossed devices, ...).
pub const MIG_IO: &str = "MIG-IO";

/// The diagnostic code for a rename whose target already exists (the
/// apply refuses before it moves anything, so a colliding migration
/// never overwrites a file).
pub const MIG_TARGET: &str = "MIG-TARGET";

/// The migration plan (§V2-E03 verbatim shape): renames move files,
/// rewrites change import specifiers, `config` records the keys the
/// apply pass (or the human reviewing it) writes, `manual` carries the
/// constructs a codemod must not guess at — each with its file and
/// line. Serialized camelCase for the CLI's JSON surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MigrationPlan {
    /// `(from, to)` file moves, relative to the project root.
    pub renames: Vec<(PathBuf, PathBuf)>,
    /// Import-specifier rewrites found through the import-span scanner.
    pub rewrites: Vec<ImportRewrite>,
    /// Config keys the migration introduces; a key whose target file is
    /// not `oxide.config.ts` carries the target in the key
    /// (`tsconfig.paths.$lib`).
    pub config: Vec<ConfigKey>,
    /// The constructs the migration flags for a human, with locations.
    pub manual: Vec<Note>,
}

/// One planned import rewrite: the file, the specifier as it is today
/// and the specifier it becomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRewrite {
    /// The file the import lives in (project-root relative).
    pub file: PathBuf,
    /// The specifier as it is today.
    pub from: String,
    /// The specifier it becomes.
    pub to: String,
}

/// One config key the migration introduces: the key and its literal
/// value spelling (the value is data — the plan does not evaluate JS).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigKey {
    /// The dotted key spelling (the target file prefix when it is not
    /// `oxide.config.ts`).
    pub key: String,
    /// The literal value spelling (JSON-ish, ready to paste).
    pub value: String,
}

/// One manual note: the file, the 1-based line and what a human must
/// decide. Unsupported constructs appear here — never silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    /// The file the note is about (project-root relative; a directory
    /// note names the directory).
    pub file: PathBuf,
    /// The 1-based line the construct sits on (1 for whole-file notes).
    pub line: u32,
    /// What must be done by hand.
    pub text: String,
}

/// The apply report: the mechanical work performed and the work left
/// for the `--apply` CLI pass (or the human) — the plan's config keys
/// are applied (the E03 apply-in-engine half: a key the engine cannot
/// spell into its target file stays pending loud, never silent), and
/// the notes are never applied silently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    /// Renames performed (every planned rename ran).
    pub renamed: usize,
    /// Import-rewrite entries applied (a file whose scanned spans
    /// carried the `from` specifier).
    pub rewrote: usize,
    /// The planned config keys the apply wrote into their target file
    /// (`oxide.config.ts` / `tsconfig.json`), in plan order.
    #[serde(default)]
    pub config_applied: Vec<String>,
    /// The planned config keys that stayed pending: a target file the
    /// engine cannot parse, an object shape it cannot recognize, or an
    /// already-present key carrying a DIFFERENT value (never
    /// overwritten).
    #[serde(default)]
    pub config_pending: Vec<String>,
    /// The planned manual notes, pending by definition.
    pub manual: usize,
}

/// The source frameworks `oxide migrate` plans for (§V2-E03).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Framework {
    /// `SvelteKit` (`@sveltejs/kit`): the pure path mapping.
    SvelteKit,
    /// Next.js (app router): the compat conventions.
    Next,
    /// Nuxt: the `pages/` mapping.
    Nuxt,
    /// Vite SPA: the entry wrap.
    Vite,
}

impl Framework {
    /// The kebab-case spelling the detection table and the CLI surface
    /// speak.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Framework::SvelteKit => "sveltekit",
            Framework::Next => "next",
            Framework::Nuxt => "nuxt",
            Framework::Vite => "vite",
        }
    }

    /// The kebab-case spelling a user names on the CLI surface, parsed
    /// back (`sveltekit|next|nuxt|vite`). `None` for anything else — the
    /// CLI refuses an unknown source name rather than guessing.
    #[must_use]
    pub fn from_source_name(name: &str) -> Option<Self> {
        match name {
            "sveltekit" => Some(Framework::SvelteKit),
            "next" => Some(Framework::Next),
            "nuxt" => Some(Framework::Nuxt),
            "vite" => Some(Framework::Vite),
            _ => None,
        }
    }
}

/// The planner seam (§V2-E03): one implementation per source framework,
/// each producing the spec's four-field plan over the project root.
/// Object-safe so the CLI can hold the detected planner as `&dyn`.
pub trait MigrationPlanner: Send + Sync {
    /// The framework this planner migrates.
    fn framework(&self) -> Framework;

    /// The migration plan for the project rooted at `root` (the plan is
    /// data; nothing is written).
    ///
    /// # Errors
    ///
    /// A filesystem failure reading the signature files refuses
    /// [`MIG_IO`].
    fn plan(&self, root: &Path) -> Result<MigrationPlan, OxideError>;
}

/// Detect the source framework from the signature surface: the
/// `package.json` dependency names first (the framework package
/// outranks the build tool it rides on), then the framework config
/// files as the no-manifest fallback. Returns `None` for a tree it
/// cannot attribute — detect never guesses.
#[must_use]
pub fn detect_framework(root: &Path) -> Option<Framework> {
    if let Some(deps) = package_dependencies(root) {
        for (needle, fw) in [
            ("@sveltejs/kit", Framework::SvelteKit),
            ("next", Framework::Next),
            ("nuxt", Framework::Nuxt),
            ("vite", Framework::Vite),
        ] {
            if deps.contains(needle) {
                return Some(fw);
            }
        }
    }
    // The config-file fallback: a manifest that names the framework's
    // runtime but not the framework package (or no manifest at all).
    if next_config_file(root).is_some() {
        return Some(Framework::Next);
    }
    if root.join("nuxt.config.ts").is_file() || root.join("nuxt.config.js").is_file() {
        return Some(Framework::Nuxt);
    }
    let svelte_config = root.join("svelte.config.js");
    if svelte_config.is_file()
        && fs::read_to_string(&svelte_config).is_ok_and(|s| s.contains("kit"))
    {
        return Some(Framework::SvelteKit);
    }
    None
}

/// The dependency names `package.json` carries (`dependencies` plus
/// `devDependencies`), or `None` when the manifest is absent or
/// unparseable (a broken manifest is not a detection failure — the
/// config-file fallback still runs).
fn package_dependencies(root: &Path) -> Option<BTreeSet<String>> {
    let manifest = fs::read_to_string(root.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    let mut names = BTreeSet::new();
    for section in ["dependencies", "devDependencies"] {
        if let Some(map) = value.get(section).and_then(|v| v.as_object()) {
            for key in map.keys() {
                names.insert(key.clone());
            }
        }
    }
    Some(names)
}

/// The `next.config.*` spelling present at the root, project-root
/// relative (the config-file fallback's Next signature and the mapping
/// notes' target).
fn next_config_file(root: &Path) -> Option<PathBuf> {
    for name in ["next.config.js", "next.config.mjs", "next.config.ts"] {
        let candidate = root.join(name);
        if candidate.is_file() {
            return Some(PathBuf::from(name));
        }
    }
    None
}

/// The planner for a detected framework.
#[must_use]
pub fn planner_for(framework: Framework) -> Box<dyn MigrationPlanner> {
    match framework {
        Framework::SvelteKit => Box::new(SvelteKitPlanner),
        Framework::Next => Box::new(NextPlanner),
        Framework::Nuxt => Box::new(NuxtPlanner),
        Framework::Vite => Box::new(VitePlanner),
    }
}

/// The one entry the `oxide migrate` command drives: detect the source
/// framework, then plan. An unattributable tree refuses [`MIG_DETECT`]
/// naming the root and the four supported spellings.
///
/// # Errors
///
/// [`MIG_DETECT`] when no framework is detected; [`MIG_IO`] when the
/// signature walk fails.
pub fn plan_for(root: &Path) -> Result<MigrationPlan, OxideError> {
    let Some(framework) = detect_framework(root) else {
        return Err(refuse_detect(root));
    };
    planner_for(framework).plan(root)
}

/// Apply the plan's mechanical halves to the project rooted at `root`:
/// the renames (after a whole-set collision pre-check — a colliding
/// migration refuses [`MIG_TARGET`] before anything moves) and the
/// import rewrites (span surgery over [`crate::scan_import_spans`]).
/// The config keys are applied by the engine (the E03 apply-in-engine
/// half: `tsconfig.`-prefixed keys merge into `tsconfig.json`, bare
/// dotted keys merge into `oxide.config.ts`); the manual notes are
/// never applied — the report carries them pending.
///
/// # Errors
///
/// [`MIG_TARGET`] when any rename target exists; [`MIG_IO`] when a
/// file cannot be read, moved or rewritten.
pub fn apply_plan(root: &Path, plan: &MigrationPlan) -> Result<ApplyReport, OxideError> {
    // The collision pre-check runs over the WHOLE rename set before the
    // first move: a migration that half-applied and then refused would
    // be worse than one that refused up front.
    for (from, to) in &plan.renames {
        if root.join(to).exists() {
            return Err(OxideError(Box::new(
                oxide_fw_diagnostics::Diagnostic::error(
                    MIG_TARGET,
                    format!(
                        "rename target {} already exists; the migration would overwrite it",
                        to.display()
                    ),
                )
                .with_fix(format!(
                    "resolve the collision (the source is {}), or apply in a clean git tree",
                    from.display()
                )),
            )));
        }
    }
    let mut report = ApplyReport {
        renamed: 0,
        rewrote: 0,
        config_applied: Vec::new(),
        config_pending: Vec::new(),
        manual: plan.manual.len(),
    };
    for (from, to) in &plan.renames {
        let src = root.join(from);
        let dst = root.join(to);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| refuse_io(&format!("cannot create {}", parent.display()), &e))?;
        }
        fs::rename(&src, &dst)
            .map_err(|e| refuse_io(&format!("cannot move {}", src.display()), &e))?;
        report.renamed += 1;
    }
    for rewrite in &plan.rewrites {
        if rewrite_spans(root, rewrite)? {
            report.rewrote += 1;
        }
    }
    for key in apply_config_keys(root, &plan.config)? {
        match key {
            Ok(name) => report.config_applied.push(name),
            Err(name) => report.config_pending.push(name),
        }
    }
    Ok(report)
}

// ------------------------------------------------------------- config apply

/// The outcome of one config-key apply: `Ok` the key was written (or
/// already rode the file with the planned value — an idempotent no-op),
/// `Err` the key stays pending loud (an unparseable or unrecognizable
/// target, or an already-present DIFFERENT value — never overwritten).
type ConfigKeyOutcome = Result<String, String>;

/// Apply the plan's config keys (§V2-E03): every `tsconfig.`-prefixed
/// key merges into `tsconfig.json`, every bare dotted key merges into
/// `oxide.config.ts`. Plan order, one outcome per key.
///
/// # Errors
///
/// [`MIG_IO`] when a target file cannot be read or written — an
/// unparseable or unrecognizable TARGET is not an error: the key stays
/// pending (the report names it) rather than the whole apply failing
/// after the renames ran.
fn apply_config_keys(root: &Path, keys: &[ConfigKey]) -> Result<Vec<ConfigKeyOutcome>, OxideError> {
    let mut outcomes = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(rest) = key.key.strip_prefix("tsconfig.") {
            outcomes.push(apply_tsconfig_key(root, &key.key, rest, &key.value)?);
        } else {
            outcomes.push(apply_oxide_config_key(root, &key.key, &key.value)?);
        }
    }
    Ok(outcomes)
}

/// Merge one `tsconfig.` key into `tsconfig.json`. `key` is the full
/// spelling the report carries (`tsconfig.paths.$lib`); `remainder` is
/// what follows the `tsconfig.` prefix (`paths.$lib`). The `paths.X`
/// spelling lands under `compilerOptions.paths.X` — the one place
/// tsconfig path mappings live. A missing file is created minimal; an
/// unparseable file stays pending; a present equal value re-applies as
/// a no-op; a present different value is never overwritten.
///
/// # Errors
///
/// [`MIG_IO`] when the file cannot be read or written.
fn apply_tsconfig_key(
    root: &Path,
    key: &str,
    remainder: &str,
    value: &str,
) -> Result<ConfigKeyOutcome, OxideError> {
    let path = root.join("tsconfig.json");
    let mut value_json = match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    code = "MIG-CONFIG",
                    file = %path.display(),
                    error = %e,
                    "tsconfig.json does not parse; the config key stays pending"
                );
                return Ok(Err(key.to_string()));
            }
        },
        Err(_) => serde_json::json!({"compilerOptions": {}}),
    };
    // `tsconfig.paths.X` → `compilerOptions.paths.X`; any other
    // remainder spelling is a planner/engine contract mismatch — the
    // key stays pending loud instead of guessing a location.
    let Some(location) = remainder.strip_prefix("paths.") else {
        tracing::warn!(
            code = "MIG-CONFIG",
            key = %key,
            "unrecognized tsconfig key spelling (only `tsconfig.paths.X` is mapped); \
             the config key stays pending"
        );
        return Ok(Err(key.to_string()));
    };
    let parsed: serde_json::Value = match serde_json::from_str(value) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                code = "MIG-CONFIG",
                key = %key,
                value = %value,
                error = %e,
                "the planned value is not valid JSON; the config key stays pending"
            );
            return Ok(Err(key.to_string()));
        }
    };
    let Some(existing) = value_json
        .get("compilerOptions")
        .and_then(|c| c.get("paths"))
        .and_then(|p| p.get(location))
    else {
        if !set_json_path(&mut value_json, location, parsed.clone()) {
            tracing::warn!(
                code = "MIG-CONFIG",
                file = %path.display(),
                "tsconfig.json is not a JSON object; the config key stays pending"
            );
            return Ok(Err(key.to_string()));
        }
        write_json(&path, &value_json)?;
        return Ok(Ok(key.to_string()));
    };
    if existing == &parsed {
        return Ok(Ok(key.to_string()));
    }
    tracing::warn!(
        code = "MIG-CONFIG",
        key = %key,
        "tsconfig.json already carries a different value for this key; \
         it is never overwritten — the config key stays pending"
    );
    Ok(Err(key.to_string()))
}

/// Set `compilerOptions.paths.<location> = value` on the JSON document,
/// creating the intermediate objects. `false` when the document is not
/// a JSON object (the caller keeps the key pending — nothing written).
/// Runs only after the existing-value checks, so it never overwrites
/// user data.
fn set_json_path(doc: &mut serde_json::Value, location: &str, value: serde_json::Value) -> bool {
    let Some(doc) = doc.as_object_mut() else {
        return false;
    };
    let options = doc
        .entry("compilerOptions")
        .or_insert_with(|| serde_json::json!({}));
    let Some(options) = options.as_object_mut() else {
        return false;
    };
    let paths = options
        .entry("paths")
        .or_insert_with(|| serde_json::json!({}));
    let Some(paths) = paths.as_object_mut() else {
        return false;
    };
    paths.insert(location.to_string(), value);
    true
}

/// Serialize `tsconfig.json` the way the file convention spells it
/// (two-space indent, trailing newline).
///
/// # Errors
///
/// [`MIG_IO`] when the file cannot be written.
fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), OxideError> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|e| refuse_io(&format!("cannot serialize {}", path.display()), &e))?;
    text.push('\n');
    fs::write(path, text).map_err(|e| refuse_io(&format!("cannot write {}", path.display()), &e))
}

/// Merge one bare dotted key (`routes.compat` = `"next"`) into
/// `oxide.config.ts`. The file is the scaffold's own shape —
/// `export default defineConfig({ ... })` or `export default { ... }` —
/// and the merge is text surgery: existing rows are never rewritten, a
/// missing top-level object is created minimal, a leaf joins an
/// existing top-level object, a present equal value re-applies as a
/// no-op, a present different value or an unrecognizable shape stays
/// pending loud.
///
/// # Errors
///
/// [`MIG_IO`] when the file cannot be read or written.
fn apply_oxide_config_key(
    root: &Path,
    key: &str,
    value: &str,
) -> Result<ConfigKeyOutcome, OxideError> {
    let path = root.join("oxide.config.ts");
    let (text, created) = match fs::read_to_string(&path) {
        Ok(text) => (text, false),
        Err(_) => (
            "import { defineConfig } from '@oxide-fw/core'\n\n\
             export default defineConfig({\n})\n"
                .to_string(),
            true,
        ),
    };
    let Some(span) = config_object_span(&text) else {
        if created {
            tracing::warn!(
                code = "MIG-CONFIG",
                file = %path.display(),
                "the freshly created oxide.config.ts does not spell the scaffold \
                 object; the config key stays pending"
            );
        } else {
            tracing::warn!(
                code = "MIG-CONFIG",
                file = %path.display(),
                "oxide.config.ts spells neither a defineConfig object nor a \
                 plain export-default object the engine recognizes; \
                 the config key stays pending"
            );
        }
        return Ok(Err(key.to_string()));
    };
    let segments: Vec<&str> = key.split('.').collect();
    // Already present? The leaf spelling inside the object span decides:
    // the same literal is the idempotent no-op, a different literal is
    // never overwritten.
    if let Some((row_start, row_end)) = existing_leaf_row(&text, span, &segments) {
        let existing = text[row_start..row_end].trim_end_matches(',');
        let wanted = format!("{}: {}", segments.last().unwrap_or(&key), value);
        if existing.trim() == wanted {
            return Ok(Ok(key.to_string()));
        }
        tracing::warn!(
            code = "MIG-CONFIG",
            key = %key,
            "oxide.config.ts already carries a different value for this key; \
             it is never overwritten — the config key stays pending"
        );
        return Ok(Err(key.to_string()));
    }
    let Some(merged) = insert_row(&text, span, &segments, value) else {
        tracing::warn!(
            code = "MIG-CONFIG",
            key = %key,
            "the config key's path runs deeper than the engine merges into an \
             existing object; the config key stays pending"
        );
        return Ok(Err(key.to_string()));
    };
    fs::write(&path, &merged)
        .map_err(|e| refuse_io(&format!("cannot write {}", path.display()), &e))?;
    Ok(Ok(key.to_string()))
}

/// The `(start, end)` byte span of the scaffold's exported config
/// object — the `{` after `defineConfig(` (or after `export default`)
/// through its matching `}`, found by counting braces. `None` when the
/// file spells neither shape (the pending posture).
fn config_object_span(text: &str) -> Option<(usize, usize)> {
    let open = define_config_open(text).or_else(|| export_default_open(text))?;
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    for (i, byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((open, i));
                }
            }
            _ => {}
        }
    }
    None
}

/// The `{` opening the `defineConfig({...})` object literal, if the
/// file spells it.
fn define_config_open(text: &str) -> Option<usize> {
    let marker = text.find("defineConfig(")?;
    text[marker..].find('{').map(|i| marker + i)
}

/// The `{` opening a plain `export default { ... }` object literal, if
/// the file spells it.
fn export_default_open(text: &str) -> Option<usize> {
    let marker = text.find("export default")?;
    text[marker..].find('{').map(|i| marker + i)
}

/// The row a dotted key spells when its whole path is absent:
/// `routes: { compat: "next" }` for `routes.compat` — nested objects
/// for deeper paths, the value pasted verbatim.
fn nested_row(segments: &[&str], value: &str) -> String {
    let mut row = format!("{}: {value}", segments[segments.len() - 1]);
    for segment in segments[..segments.len() - 1].iter().rev() {
        row = format!("{segment}: {{ {row} }}");
    }
    row
}

/// Find the leaf row an already-applied key would occupy inside the
/// object span — the leaf segment spelled as `leaf:` at ANY depth the
/// engine merges at (the top object or one nested object down, the
/// shapes the planners emit). The row's `(start, end)` byte span
/// covers `leaf: <literal>` without the trailing comma. `None` when
/// the leaf is absent (or the path runs deeper than the engine
/// merges — which reads as absent, and the insert path stays pending
/// through [`insert_row`]).
fn existing_leaf_row(
    text: &str,
    span: (usize, usize),
    segments: &[&str],
) -> Option<(usize, usize)> {
    let leaf = *segments.last()?;
    let needle = format!("{leaf}:");
    let object = text.get(span.0..span.1)?;
    let at = span.0 + object.find(&needle)?;
    let after = at + needle.len();
    let rest = text.get(after..)?;
    let end = after + rest.find([',', '\n', '}']).unwrap_or(rest.len());
    Some((at, end))
}

/// Insert the absent key's row into the object span. The top-level
/// segment already present means the leaf joins THAT object (inline or
/// block form); otherwise the whole nested row inserts before the
/// object's closing brace. `None` when the shape cannot be merged (a
/// path deeper than one nested object, or a non-object top-level
/// segment) — the caller keeps the key pending instead of a corrupted
/// insert.
fn insert_row(text: &str, span: (usize, usize), segments: &[&str], value: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let open = span.0;
    let close = span.1;
    // The leaf-only spelling (`compat: "next"`) — the shape inserted
    // when the top-level segment already exists (the full nested row
    // would spell the segment twice).
    let leaf_only = format!("{}: {value}", segments[segments.len() - 1]);
    // The full nested row (`routes: { compat: "next" }`) — the shape
    // inserted when the whole path is absent.
    let leaf_row = nested_row(segments, value);
    // The top-level segment present: find `segment: {` and insert the
    // leaf inside that object (the `routes` shape both planners emit).
    let head = format!("{}:", segments[0]);
    let object_text = text.get(open..=close).unwrap_or_default();
    if let Some(at) = object_text.find(&head) {
        if segments.len() > 2 {
            return None;
        }
        let brace = open + at + head.len();
        let Some(inner) = text[brace..].find('{').map(|i| brace + i) else {
            // `segment:` bound to something that is not an object
            // literal — the engine does not rewrite user expressions.
            return None;
        };
        let mut depth = 0usize;
        let mut inner_close = inner;
        for (i, byte) in bytes.iter().enumerate().skip(inner) {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        inner_close = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        // Inline shape `routes: { dir: 'x' }` — the leaf joins the
        // one line; the original trailing whitespace between the last
        // value and the brace is consumed, the row carries its own
        // single-space padding, and the tail resumes AT the brace.
        let inline = !text[inner..=inner_close].contains('\n');
        let (insert_at, tail_from, row) = if inline {
            let content_end = inner + 1 + text[inner + 1..inner_close].trim_end().len();
            let trimmed = text[inner + 1..content_end].trim();
            let row = if trimmed.is_empty() || trimmed.ends_with(',') {
                format!(" {leaf_only} ")
            } else {
                format!(", {leaf_only} ")
            };
            (content_end, inner_close, row)
        } else {
            // The last content line before the closing brace: the
            // insertion point — a missing trailing comma is supplied
            // on THAT line, then the leaf joins as its own row.
            let before = &text[..inner_close];
            let line_start = before.rfind('\n').map_or(0, |i| i + 1);
            let last_line = text[line_start..inner_close].trim_end();
            let row_indent = last_line.len().saturating_sub(last_line.trim_start().len());
            let mut row = String::new();
            if !last_line.is_empty() && !last_line.ends_with(',') {
                row.push(',');
            }
            row.push('\n');
            row.push_str(&" ".repeat(row_indent));
            row.push_str(&leaf_only);
            row.push(',');
            let at = line_start + last_line.len();
            (at, at, row)
        };
        let mut merged = String::with_capacity(text.len() + row.len());
        merged.push_str(&text[..insert_at]);
        merged.push_str(&row);
        merged.push_str(&text[tail_from..]);
        return Some(merged);
    }
    // The top-level segment absent: the whole nested row inserts before
    // the object's closing brace, two-space indented.
    let before = &text[..close];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let last_line = text[line_start..close].trim_end();
    let needs_comma = !last_line.is_empty() && !last_line.ends_with(',');
    let mut merged = String::with_capacity(text.len() + leaf_row.len() + 8);
    merged.push_str(&text[..close]);
    if needs_comma {
        merged.push(',');
    }
    merged.push('\n');
    merged.push_str("  ");
    merged.push_str(&leaf_row);
    merged.push(',');
    merged.push_str(&text[close..]);
    Some(merged)
}

// --------------------------------------------------------------- SvelteKit

/// The `SvelteKit` planner (§V2-E03): the pure path mapping, the
/// `hooks.server.ts` → `middleware.ts` rename (or the `sequence`
/// manual note), and the `$lib` tsconfig path entry.
struct SvelteKitPlanner;

impl MigrationPlanner for SvelteKitPlanner {
    fn framework(&self) -> Framework {
        Framework::SvelteKit
    }

    fn plan(&self, root: &Path) -> Result<MigrationPlan, OxideError> {
        let mut plan = MigrationPlan::default();
        // The route files: every `+`-prefixed basename loses its
        // marker; endpoints become `route.ts` (the spec's `+server.ts`
        // to `route.ts` row). Oxide's conventional routes root is
        // already `src/routes`, so the moves stay inside the tree the
        // router scans.
        for file in collect_files(root, Path::new("src/routes"))? {
            let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(new_name) = oxide_route_basename(name) else {
                continue;
            };
            if new_name != name {
                let to = file.with_file_name(new_name);
                plan.renames.push((file, to));
            }
        }
        // The hooks file: a plain `handle` export IS the middleware
        // rename; a `sequence` composition flags manual instead (the
        // chain must be merged by hand first).
        let hooks = PathBuf::from("src").join("hooks.server.ts");
        if root.join(&hooks).is_file() {
            let source = fs::read_to_string(root.join(&hooks))
                .map_err(|e| refuse_io(&format!("cannot read {}", hooks.display()), &e))?;
            match line_of(&source, "sequence") {
                Some(line) => plan.manual.push(Note {
                    file: hooks.clone(),
                    line,
                    text: "`hooks.server.ts` composes with `sequence` — merge the chain into \
                           one `handle` export by hand, then rename it to `src/middleware.ts`"
                        .to_string(),
                }),
                None => {
                    plan.renames
                        .push((hooks.clone(), PathBuf::from("src/middleware.ts")));
                }
            }
        }
        // The `$lib` alias: a tsconfig path entry (the spec's "$lib
        // alias to a tsconfig path" row) — the imports stay spelled
        // `$lib/…` and resolve through the path mapping.
        if root.join("src").join("lib").is_dir() {
            plan.config.push(ConfigKey {
                key: "tsconfig.paths.$lib".to_string(),
                value: "[\"./src/lib\"]".to_string(),
            });
        }
        sort_plan(&mut plan);
        Ok(plan)
    }
}

/// The Oxide route basename for a `SvelteKit` route-file name: the
/// `+`-prefixed markers strip, `+server.*` becomes `route.*`, and a
/// name without a marker is not a route file (`None`).
fn oxide_route_basename(name: &str) -> Option<String> {
    let rest = name.strip_prefix('+')?;
    if let Some(ext) = rest.strip_prefix("server.") {
        return Some(format!("route.{ext}"));
    }
    Some(rest.to_string())
}

// -------------------------------------------------------------------- Next

/// The `next/link` / `next/image` specifiers the adapter's components
/// replace (§V2-E03's "import rewrites to the adapter's `Link`/`Image`"
/// — both publish from `@oxide/react`).
const NEXT_ADAPTER_REWRITES: &[(&str, &str)] = &[
    ("next/link", "@oxide/react"),
    ("next/image", "@oxide/react"),
];

/// The next.config keys the spec's table maps into `oxide.config.ts`,
/// in the order the notes carry them.
const NEXT_CONFIG_TABLE: &[&str] = &[
    "images.remotePatterns",
    "redirects",
    "rewrites",
    "headers",
    "i18n",
];

/// The Next.js (app router) planner (§V2-E03): the compat key, the
/// adapter import rewrites through the span scanner, the `'use client'`
/// island candidates, the next.config table notes, the pages-router
/// refusal and the middleware matcher note.
struct NextPlanner;

impl MigrationPlanner for NextPlanner {
    fn framework(&self) -> Framework {
        Framework::Next
    }

    fn plan(&self, root: &Path) -> Result<MigrationPlan, OxideError> {
        let mut plan = MigrationPlan::default();
        plan.config.push(ConfigKey {
            key: "routes.compat".to_string(),
            value: "\"next\"".to_string(),
        });
        // The app-router source set: `app/**` (or `src/app/**` when the
        // project nests it), scanned for the adapter rewrites and the
        // island candidates.
        let app_dir = if root.join("app").is_dir() {
            PathBuf::from("app")
        } else {
            PathBuf::from("src").join("app")
        };
        for file in collect_files(root, &app_dir)? {
            if !is_source_file(&file) {
                continue;
            }
            let source = fs::read_to_string(root.join(&file))
                .map_err(|e| refuse_io(&format!("cannot read {}", file.display()), &e))?;
            // The rewrites: the import-span scanner finds the exact
            // statements (never regex on source); one entry per
            // (file, from).
            let mut seen = BTreeSet::new();
            for span in crate::scan_import_spans(&source) {
                let Some((_, to)) = NEXT_ADAPTER_REWRITES
                    .iter()
                    .find(|(from, _)| *from == span.specifier)
                else {
                    continue;
                };
                if seen.insert(span.specifier.clone()) {
                    plan.rewrites.push(ImportRewrite {
                        file: file.clone(),
                        from: span.specifier.clone(),
                        to: (*to).to_string(),
                    });
                }
            }
            // The island candidates: reported, never guessed into an
            // islands config (the spec's "report, do not guess").
            for (idx, line) in source.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed == "'use client'" || trimmed == "\"use client\"" {
                    plan.manual.push(Note {
                        file: file.clone(),
                        line: line_number(idx),
                        text: "`'use client'` file — an island candidate for the compat \
                               conventions (register it in `islands` explicitly; the migration \
                               does not guess)"
                            .to_string(),
                    });
                }
            }
        }
        // The next.config table: located notes (file + line + the oxide
        // spelling) for the keys the config actually carries — the
        // planner does not evaluate JS, so carrying the VALUE is the
        // apply pass's human-reviewed work, and a key the config does
        // not have is never planned.
        if let Some(config) = next_config_file(root) {
            let text = fs::read_to_string(root.join(&config))
                .map_err(|e| refuse_io(&format!("cannot read {}", config.display()), &e))?;
            for key in NEXT_CONFIG_TABLE {
                let leaf = key.rsplit('.').next().unwrap_or(key);
                let Some(line) = line_of(&text, leaf) else {
                    continue;
                };
                plan.manual.push(Note {
                    file: config.clone(),
                    line,
                    text: format!(
                        "map `next.config` `{key}` to `{key}` in `oxide.config.ts` (carry the \
                         value by hand; the planner does not evaluate JS)"
                    ),
                });
            }
        }
        // The pages router: refused, not guessed (the compat
        // conventions cover the app router only).
        if root.join("pages").is_dir() {
            plan.manual.push(Note {
                file: PathBuf::from("pages"),
                line: 1,
                text: "the `pages` router is not migrated — port each `pages/**` file to a \
                       `routes/**` page by hand, or move the project to the app router first"
                    .to_string(),
            });
        }
        // The middleware: adopted as the project middleware; a
        // `matcher` with a complex regex goes to manual.
        let middleware = PathBuf::from("middleware.ts");
        if root.join(&middleware).is_file() {
            let text = fs::read_to_string(root.join(&middleware))
                .map_err(|e| refuse_io(&format!("cannot read {}", middleware.display()), &e))?;
            match line_of(&text, "matcher") {
                Some(line) => plan.manual.push(Note {
                    file: middleware.clone(),
                    line,
                    text: "a middleware `matcher` with a complex regex goes to manual — port it \
                           to an Oxide `rules` entry by hand"
                        .to_string(),
                }),
                None => plan.manual.push(Note {
                    file: middleware.clone(),
                    line: 1,
                    text: "`middleware.ts` adopts as the project middleware (move it under the \
                           routes root if `routes.dir` points elsewhere)"
                        .to_string(),
                }),
            }
        }
        sort_plan(&mut plan);
        Ok(plan)
    }
}

/// Whether the path's extension is one the import scanner reads
/// (`.ts`, `.tsx`, `.js`, `.jsx`, `.mts`, `.cts`).
fn is_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts")
    )
}

// -------------------------------------------------------------------- Nuxt

/// The Nuxt planner (§V2-E03): `pages/` maps into the routes root with
/// the bracket conventions carried verbatim, parent route components
/// demote to layouts, `useFetch` / `useAsyncData` flag manual, the
/// named middleware flags manual.
struct NuxtPlanner;

impl MigrationPlanner for NuxtPlanner {
    fn framework(&self) -> Framework {
        Framework::Nuxt
    }

    fn plan(&self, root: &Path) -> Result<MigrationPlan, OxideError> {
        let mut plan = MigrationPlan::default();
        // The route components: `pages/**` into `src/routes/**`.
        let files = collect_files(root, Path::new("pages"))?;
        for file in &files {
            let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // The dirs the component hangs under, RELATIVE to `pages/`
            // (the walk's paths carry the `pages` prefix).
            let under = file.strip_prefix("pages").unwrap_or(file);
            let dirs = under.parent().unwrap_or(Path::new(""));
            if name == "index.vue" {
                // `pages/index.vue` is the dir's page.
                let to = routes_target(dirs, "page.vue");
                plan.renames.push((file.clone(), to));
                continue;
            }
            let Some(component) = name.strip_suffix(".vue") else {
                // Nuxt route middleware (`pages/**` carries none — the
                // named middleware lives in `middleware/`); anything
                // else under pages/ flags manual.
                plan.manual.push(Note {
                    file: file.clone(),
                    line: 1,
                    text: format!(
                        "`{}` is not a route component the planner maps — port it by hand",
                        file.display()
                    ),
                });
                continue;
            };
            // A parent route component (the file a same-named sibling
            // directory hangs children under) is the dir's LAYOUT.
            let is_parent = root.join("pages").join(dirs).join(component).is_dir();
            let basename = if is_parent { "layout.vue" } else { "page.vue" };
            let to = routes_target(&dirs.join(component), basename);
            plan.renames.push((file.clone(), to));
        }
        // The named layouts: `default` is the routes-root layout; a
        // custom one flags manual (Oxide has the one layout chain).
        for file in collect_files(root, Path::new("layouts"))? {
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "default.vue" {
                plan.renames
                    .push((file.clone(), PathBuf::from("src/routes/layout.vue")));
            } else {
                plan.manual.push(Note {
                    file: file.clone(),
                    line: 1,
                    text: "a named Nuxt layout has no Oxide equivalent (one layout chain) — \
                           fold it into `src/routes/layout.vue` by hand"
                        .to_string(),
                });
            }
        }
        // The error component and the named middleware.
        if root.join("error.vue").is_file() {
            plan.renames.push((
                PathBuf::from("error.vue"),
                PathBuf::from("src/routes/error.vue"),
            ));
        }
        for file in collect_files(root, Path::new("middleware"))? {
            plan.manual.push(Note {
                file: file.clone(),
                line: 1,
                text: "a named Nuxt middleware runs inside ONE Oxide `middleware.ts` chain — \
                       merge it by hand (the §8.5 chain runner takes one module)"
                    .to_string(),
            });
        }
        // The data fetching: `useFetch` / `useAsyncData` move server-side.
        for file in &files {
            if file.extension().is_none_or(|e| e != "vue") {
                continue;
            }
            let source = fs::read_to_string(root.join(file))
                .map_err(|e| refuse_io(&format!("cannot read {}", file.display()), &e))?;
            for (needle, text) in [
                (
                    "useFetch(",
                    "`useFetch` has no Oxide equivalent — convert the call to a `page.server.ts` \
                     `load` export (the data moves server-side)",
                ),
                (
                    "useAsyncData(",
                    "`useAsyncData` has no Oxide equivalent — convert the call to a \
                     `page.server.ts` `load` export (the data moves server-side)",
                ),
            ] {
                if let Some(line) = line_of(&source, needle) {
                    plan.manual.push(Note {
                        file: file.clone(),
                        line,
                        text: text.to_string(),
                    });
                }
            }
        }
        sort_plan(&mut plan);
        Ok(plan)
    }
}

/// The routes-root target for a Nuxt component: `src/routes` plus the
/// dirs plus the basename (the conventional routes root the router
/// scans).
fn routes_target(dirs: &Path, basename: &str) -> PathBuf {
    let mut target = PathBuf::from("src").join("routes");
    for component in dirs.components() {
        target.push(component.as_os_str());
    }
    target.push(basename);
    target
}

// -------------------------------------------------------------------- Vite

/// The Vite SPA planner (§V2-E03): the wrap — the existing entry
/// becomes the `spa` route, the routes dir is explicit, nothing
/// renamed.
struct VitePlanner;

impl MigrationPlanner for VitePlanner {
    fn framework(&self) -> Framework {
        Framework::Vite
    }

    fn plan(&self, root: &Path) -> Result<MigrationPlan, OxideError> {
        let mut plan = MigrationPlan::default();
        plan.config.push(ConfigKey {
            key: "routes.dir".to_string(),
            value: "\"src/routes\"".to_string(),
        });
        // The entry: the first conventional Vite entry spelling found.
        let mut entry = None;
        for name in [
            "main.tsx",
            "main.ts",
            "main.jsx",
            "main.js",
            "index.tsx",
            "index.ts",
        ] {
            let candidate = PathBuf::from("src").join(name);
            if root.join(&candidate).is_file() {
                entry = Some(candidate);
                break;
            }
        }
        if let Some(entry) = entry {
            plan.manual.push(Note {
                file: entry.clone(),
                line: 1,
                text: format!(
                    "create `src/routes/page.tsx` with `export {{ default }} from '{}'` — the \
                     existing entry becomes the `spa` route (the index.html script tag moves to \
                     the built shell)",
                    entry.display()
                ),
            });
        }
        sort_plan(&mut plan);
        Ok(plan)
    }
}

// ------------------------------------------------------------------ helpers

/// Every file under the project-root-relative `dir` (recursively),
/// project-root relative, sorted for deterministic plans. A missing
/// directory yields nothing (a framework without that surface plans
/// without it).
fn collect_files(root: &Path, dir: &Path) -> Result<Vec<PathBuf>, OxideError> {
    let mut out = Vec::new();
    if !root.join(dir).is_dir() {
        return Ok(out);
    }
    // The explicit relative stack: each popped path is joined onto the
    // root for the read and pushed back complete, so no path component
    // is ever lost between recursion levels.
    let mut stack = vec![dir.to_path_buf()];
    while let Some(rel) = stack.pop() {
        let full = root.join(&rel);
        let entries = fs::read_dir(&full)
            .map_err(|e| refuse_io(&format!("cannot read {}", full.display()), &e))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| refuse_io(&format!("cannot read {}", full.display()), &e))?;
            let rel_child = rel.join(entry.file_name());
            if entry.path().is_dir() {
                stack.push(rel_child);
            } else {
                out.push(rel_child);
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Sort every plan list so two runs over the same tree serialize
/// byte-identical (the determinism pin).
fn sort_plan(plan: &mut MigrationPlan) {
    plan.renames.sort();
    plan.rewrites
        .sort_by(|a, b| (&a.file, &a.from, &a.to).cmp(&(&b.file, &b.from, &b.to)));
    plan.config.sort_by(|a, b| a.key.cmp(&b.key));
    plan.manual.sort_by(|a, b| {
        (&a.file, a.line, a.text.as_str()).cmp(&(&b.file, b.line, b.text.as_str()))
    });
}

/// The 1-based line number for a 0-based index (the house cast).
fn line_number(zero_based: usize) -> u32 {
    u32::try_from(zero_based + 1).unwrap_or(u32::MAX)
}

/// The first 1-based line containing `needle`, or `None`.
fn line_of(source: &str, needle: &str) -> Option<u32> {
    source
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(needle))
        .map(|(idx, _)| line_number(idx))
}

/// Rewrite one rewrite entry's spans in place (surgery over the scanned
/// spans, from the end backwards so the byte offsets stay valid).
/// Returns whether the file carried the `from` specifier at all.
fn rewrite_spans(root: &Path, rewrite: &ImportRewrite) -> Result<bool, OxideError> {
    let path = root.join(&rewrite.file);
    let source = fs::read_to_string(&path)
        .map_err(|e| refuse_io(&format!("cannot read {}", path.display()), &e))?;
    let spans: Vec<_> = crate::scan_import_spans(&source)
        .into_iter()
        .filter(|span| span.specifier == rewrite.from)
        .collect();
    if spans.is_empty() {
        return Ok(false);
    }
    let mut bytes = source.into_bytes();
    for span in spans.into_iter().rev() {
        // The specifier sits inside the statement's quotes; replace the
        // quoted segment so the quote characters stay balanced.
        let (start, end) = quoted_span(&bytes, span.start, span.end);
        let replacement = rewrite.to.clone().into_bytes();
        bytes.splice(start..end, replacement);
    }
    String::from_utf8(bytes)
        .map_err(|e| refuse_io(&format!("cannot rewrite {}", path.display()), &e))
        .and_then(|rewritten| {
            fs::write(&path, rewritten)
                .map_err(|e| refuse_io(&format!("cannot write {}", path.display()), &e))
        })?;
    Ok(true)
}

/// The byte span of the quoted specifier inside an import statement
/// (the first quote-delimited segment between `stmt_start` and
/// `stmt_end`): the scanner reports statement spans, the surgery
/// replaces only the quoted bytes.
fn quoted_span(bytes: &[u8], stmt_start: usize, stmt_end: usize) -> (usize, usize) {
    let window = &bytes[stmt_start..stmt_end.min(bytes.len())];
    let Some(quote_pos) = window.iter().position(|b| *b == b'\'' || *b == b'"') else {
        return (stmt_start, stmt_end);
    };
    let quote = window[quote_pos];
    let Some(close_rel) = window[quote_pos + 1..].iter().position(|b| *b == quote) else {
        return (stmt_start, stmt_end);
    };
    (
        stmt_start + quote_pos + 1,
        stmt_start + quote_pos + 1 + close_rel,
    )
}

/// The [`MIG_DETECT`] refusal: the root named, the four supported
/// spellings listed (the fix names them too).
fn refuse_detect(root: &Path) -> OxideError {
    OxideError(Box::new(
        oxide_fw_diagnostics::Diagnostic::error(
            MIG_DETECT,
            format!(
                "no migratable framework detected in {} (the signature surface: package.json \
                 dependency names, next.config.*, nuxt.config.*, svelte.config.js with `kit`)",
                root.display()
            ),
        )
        .with_fix(
            "point `oxide migrate` at the project root, or migrate by hand — supported: \
             sveltekit, next, nuxt, vite",
        ),
    ))
}

/// The [`MIG_IO`] refusal for a filesystem failure.
fn refuse_io(what: &str, e: impl std::fmt::Display) -> OxideError {
    OxideError(Box::new(
        oxide_fw_diagnostics::Diagnostic::error(MIG_IO, format!("{what}: {e}"))
            .with_fix("check the path's permissions and re-run `oxide migrate`"),
    ))
}
