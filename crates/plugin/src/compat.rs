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
/// - WASM: `transform` is wired end to end; `load`/`resolveId` are
///   host-callable (the option-returning string ABI) with the bundler-feed
///   call site pending; everything else has no host path.
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
            Hook::Transform => SupportLevel::Wired,
            Hook::Load | Hook::ResolveId => SupportLevel::Partial,
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
