//! The machine-checked plugin compatibility matrix (SPEC-15.2/§15.3, the E05
//! substrate half).
//!
//! The published table (`docs/plugin-compat.md`) used to be hand-maintained
//! prose: its rows drifted from the code twice (the JS column claimed
//! `configureServer`/`cacheHandler` ✅ although [`Hook::supported_in_bridge`]
//! refuses both, and the WASM column carried 🚫 for hooks the bridge accepts).
//! This module makes the kind × hook support level a *function of the code's
//! own model* — [`compat`] reads [`Hook::supported_in_bridge`] and the WASM
//! host's landed ABI — and [`compat_markdown_table`] renders the published
//! table. `docs/plugin-compat.md` embeds the render byte-identical, pinned by
//! `tests/compat_tdd.rs::plugin_compat_doc_embeds_the_generated_table` (the
//! E11 `doc_table_matches_the_function` pattern), so a support flip is a code
//! change plus a table re-embed, never a silent prose edit.

use crate::{Hook, PluginKind, hook_label};

/// Every hook, in the order the published table lists them. The matrix and
/// the generated table iterate this slice, so the table can never silently
/// re-sort or drop a row.
pub const HOOKS: &[Hook] = &[
    Hook::Config,
    Hook::ConfigResolved,
    Hook::BuildStart,
    Hook::BuildEnd,
    Hook::ResolveId,
    Hook::Load,
    Hook::Transform,
    Hook::RenderChunk,
    Hook::GenerateBundle,
    Hook::TransformHtml,
    Hook::HandleHotUpdate,
    Hook::Routes,
    Hook::Typegen,
    Hook::Diagnostics,
    Hook::ServerMiddleware,
    Hook::ConfigureServer,
    Hook::Adapter,
    Hook::RenderHead,
    Hook::CacheHandler,
];

/// One kind × hook cell of the published table. The markers mirror
/// `docs/plugin-compat.md`'s legend exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportLevel {
    /// The hook is invoked by the pipeline and its result is consumed.
    Wired,
    /// The host can call the hook, but a call site is still missing. Today:
    /// the WASM host speaks the `load`/`resolveId` ABI, and the CLI wire that
    /// feeds them into the bundler's `BundleOptions.load_hook`/`resolve_hook`
    /// is the remaining half.
    Partial,
    /// No host path. For the JS bridge — and for `configureServer`/
    /// `cacheHandler` on every non-native kind — declaring it fails at load
    /// (`PLG-HOOK`); a WASM declaration of any other hook loads but nothing
    /// ever invokes it.
    Unsupported,
    /// The kind does not expose this hook (native plugins are feature
    /// backends; they do not participate in the JS hook chain).
    NotApplicable,
}

impl SupportLevel {
    /// The published-table glyph (`docs/plugin-compat.md`'s legend).
    #[must_use]
    pub fn marker(self) -> &'static str {
        match self {
            SupportLevel::Wired => "✅",
            SupportLevel::Partial => "🟡",
            SupportLevel::Unsupported => "🚫",
            SupportLevel::NotApplicable => "n/a",
        }
    }
}

/// The support level for one `kind × hook` cell — the single source of truth
/// the published table renders.
///
/// - Native: `n/a` for every hook (feature backends never cross the bridge).
/// - JS: a hook the bridge supports is wired; the two the bridge refuses are
///   the fail-at-load cells ([`Hook::supported_in_bridge`] is the model).
/// - WASM: `transform`, `load` and `resolveId` are wired end to end (the
///   §V2-E05 CLI feed composes the host's `call_resolve_hook`/`call_load_hook`
///   into the bundler's `BundleOptions.load_hook`/`resolve_hook` beside the
///   JS bridge); everything else has no host path.
#[must_use]
pub fn compat(kind: PluginKind, hook: Hook) -> SupportLevel {
    match kind {
        PluginKind::Native => SupportLevel::NotApplicable,
        PluginKind::Js => {
            if hook.supported_in_bridge() {
                SupportLevel::Wired
            } else {
                SupportLevel::Unsupported
            }
        }
        PluginKind::Wasm => match hook {
            Hook::Transform | Hook::Load | Hook::ResolveId => SupportLevel::Wired,
            _ => SupportLevel::Unsupported,
        },
    }
}

/// The per-hook notes column (verbatim from the published table).
fn hook_note(hook: Hook) -> &'static str {
    match hook {
        Hook::Config => "Load-time patch, merged before the build.",
        Hook::ConfigResolved => "Read-only view after the config is frozen.",
        Hook::BuildStart => "Once per build.",
        Hook::BuildEnd => "Once per successful build with a summary.",
        Hook::ResolveId => "`(specifier, from) → id \\| null`.",
        Hook::Load => "`(id) → code \\| null`; virtual modules work.",
        Hook::Transform => "`(code, id) → code`; also reaches `compile_module`.",
        Hook::RenderChunk => "`(file, code) → code`.",
        Hook::GenerateBundle => "Called with the final bundle.",
        Hook::TransformHtml => "`(route, html) → html` in the prerender finalizer.",
        Hook::HandleHotUpdate => "`({path}) → {reload: bool}` in dev.",
        Hook::Routes => "Contributes route entries at scan time.",
        Hook::Typegen => "Contributes `.d.ts` fragments after a build.",
        Hook::Diagnostics => "Adds project diagnostics to `oxide check`.",
        Hook::ServerMiddleware => "Contributes middleware source; alias `middleware`.",
        Hook::ConfigureServer => "Called with the dev server handle.",
        Hook::Adapter => "May select the deploy target; last result wins.",
        Hook::RenderHead => "Merges meta/link/style/script into the render head.",
        Hook::CacheHandler => "Selects the cache backend before the stack is built.",
    }
}

/// The published hooks table, byte-identical to the block
/// `docs/plugin-compat.md` embeds. Deterministic by construction: the rows
/// iterate [`HOOKS`], the columns the fixed kind order, and every cell is
/// [`compat`]'s verdict.
#[must_use]
pub fn compat_markdown_table() -> String {
    const KINDS: [PluginKind; 3] = [PluginKind::Js, PluginKind::Wasm, PluginKind::Native];
    let mut out = String::from("| Hook | § | `js` | `wasm` | `native` | Notes |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    for hook in HOOKS {
        out.push_str("| `");
        out.push_str(hook_label(*hook));
        out.push_str("` | 15.2 |");
        for kind in KINDS {
            out.push(' ');
            out.push_str(compat(kind, *hook).marker());
            out.push_str(" |");
        }
        out.push(' ');
        out.push_str(hook_note(*hook));
        out.push_str(" |\n");
    }
    out
}

// ---------------------------------------------------------------------------
// V2-E05: the load-time unsupported-hook report (the runner's
// `unsupportedHooks`). The bridge already refuses `configureServer`/
// `cacheHandler` at load (`PLG-HOOK`); the gap this closes is the SILENT
// one — a plugin declaring a hook the host simply does not invoke (a WASM
// component declaring `buildStart`, which the bridge accepts but no host
// call site feeds). The model below is the single source of truth the
// compat runner records and the published verified-plugins table renders.
// ---------------------------------------------------------------------------

/// The hooks the host can actually *invoke* for `kind` — the `Wired` and
/// `Partial` cells of [`compat`] (a `Partial` hook is host-callable; the
/// call site that feeds it is the remaining half, but the host speaks the
/// ABI). A declared hook outside this set has no host path, so a plugin
/// declaring it is reported at load (not silently dropped).
///
/// - `Js`: every bridge-supported hook (the JS bridge calls every declared
///   hook the bridge supports).
/// - `Wasm`: `transform`, `load`, `resolveId` (the three the [`WasmPlugin`]
///   host has typed call methods for).
/// - `Native`: none (feature backends never cross the bridge — `n/a`).
#[must_use]
pub fn host_invocable_hooks(kind: PluginKind) -> Vec<Hook> {
    HOOKS
        .iter()
        .copied()
        .filter(|h| {
            matches!(
                compat(kind, *h),
                SupportLevel::Wired | SupportLevel::Partial
            )
        })
        .collect()
}

/// The declared hooks the host will **not** invoke — the runner's
/// `unsupportedHooks` list (§V2-E05). A declared hook is supported iff it
/// is *available* (exported in a callable shape for WASM; declared for JS,
/// where the bridge calls every declared hook) AND *host-invocable* (in
/// [`host_invocable_hooks`]). Everything else the host skips is reported.
///
/// Native plugins never cross the bridge (§15.1 kind 1), so their report is
/// empty — the feature gate is the unit, not a bridge hook.
///
/// # Examples
///
/// ```
/// use oxide_fw_plugin::{Hook, PluginKind, unsupported_hooks};
/// // A JS plugin declaring `configureServer` (bridge-unsupported) + `transform`.
/// let declared = [Hook::ConfigureServer, Hook::Transform];
/// assert_eq!(unsupported_hooks(PluginKind::Js, &declared, &declared),
///            vec![Hook::ConfigureServer]);
/// // A WASM plugin that declares `buildStart` (host has no call path) +
/// // `transform` (exported): `buildStart` is the silent gap this reports.
/// assert_eq!(unsupported_hooks(PluginKind::Wasm,
///             &[Hook::BuildStart, Hook::Transform],
///             &[Hook::Transform]),
///            vec![Hook::BuildStart]);
/// ```
#[must_use]
pub fn unsupported_hooks(kind: PluginKind, declared: &[Hook], available: &[Hook]) -> Vec<Hook> {
    if kind == PluginKind::Native {
        return Vec::new();
    }
    let invocable = host_invocable_hooks(kind);
    declared
        .iter()
        .copied()
        .filter(|h| !(available.contains(h) && invocable.contains(h)))
        .collect()
}

/// The load-time report for one loaded plugin (§V2-E05): the declared hooks
/// the host will not invoke (`unsupported_hooks`) and the subset it wires
/// into the chain. The compat runner records this as `{ status,
/// unsupportedHooks }`; the published verified-plugins table renders the
/// `unsupportedHooks` column from it.
///
/// Serialized camelCase so the runner's report parser reads the exact
/// `unsupportedHooks`/`wiredHooks` field names the spec names, and the
/// `Hook` values ride their camelCase wire names (`configureServer`,
/// `transform`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadTimeReport {
    /// The plugin name (the manifest/config `name`).
    pub plugin: String,
    /// The plugin kind (`js` / `wasm` / `native`).
    pub kind: PluginKind,
    /// Declared hooks the host will not invoke — the runner's
    /// `unsupportedHooks`.
    pub unsupported_hooks: Vec<Hook>,
    /// Declared hooks the host wires into the chain (available AND
    /// host-invocable).
    pub wired_hooks: Vec<Hook>,
}

impl LoadTimeReport {
    /// Build the report for one loaded plugin. `declared` is the manifest's
    /// hook list; `available` is what the host actually observed — for a JS
    /// plugin the bridge calls every declared hook, so pass `declared` again;
    /// for a WASM component pass [`crate::WasmPlugin::available_hooks`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxide_fw_plugin::{Hook, PluginKind, LoadTimeReport};
    /// let r = LoadTimeReport::new(
    ///     PluginKind::Js, "my-plug",
    ///     &[Hook::Transform, Hook::ConfigureServer],
    ///     &[Hook::Transform, Hook::ConfigureServer]);
    /// assert_eq!(r.unsupported_hooks, vec![Hook::ConfigureServer]);
    /// assert_eq!(r.wired_hooks, vec![Hook::Transform]);
    /// ```
    #[must_use]
    pub fn new(
        kind: PluginKind,
        name: impl Into<String>,
        declared: &[Hook],
        available: &[Hook],
    ) -> Self {
        let unsupported = unsupported_hooks(kind, declared, available);
        let wired = if kind == PluginKind::Native {
            Vec::new()
        } else {
            let invocable = host_invocable_hooks(kind);
            declared
                .iter()
                .copied()
                .filter(|h| available.contains(h) && invocable.contains(h))
                .collect()
        };
        Self {
            plugin: name.into(),
            kind,
            unsupported_hooks: unsupported,
            wired_hooks: wired,
        }
    }

    /// Whether the host wires every declared hook (no unsupported gap).
    #[must_use]
    pub fn is_fully_wired(&self) -> bool {
        self.unsupported_hooks.is_empty()
    }

    /// The structured JSON the runner parses from the build log. The line the
    /// bridge emits is `oxide-plugin-load<TAB><this json>`; the runner's
    /// `parseReport` scans for that prefix (and falls back to the existing
    /// `PLG-HOOK` messages the JS bridge raises for `configureServer`/
    /// `cacheHandler`).
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| String::from("{}"))
    }
}
