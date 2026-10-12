//! oxide-plugin — plugin host, JS bridge, WASM host, modules (spec §15).
//!
//! Three kinds share one hook model and ordering (§15.1): native (built in),
//! WASM component plugins (stable WIT interface), JS plugins (Rollup/Vite-shaped,
//! executed in the tooling pool). Hooks unsupported by the bridge fail at load with
//! a clear message + compatibility-table link — never silently at runtime (§15.2).

#![forbid(unsafe_code)]

use oxide_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod wasm;
pub use wasm::{hook_export_name, WasmPlugin};

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
}

/// Plugin ordering (§15.1): enforce pre/post around the native chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Enforce {
    Pre,
    Post,
}

/// All hooks (§15.2). Build hooks are Rollup/Vite-shaped; Oxide-specific hooks follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Hook {
    Config,
    ConfigResolved,
    BuildStart,
    ResolveId,
    Load,
    Transform,
    RenderChunk,
    GenerateBundle,
    BuildEnd,
    TransformHtml,
    HandleHotUpdate,
    ConfigureServer,
    // Oxide-specific:
    Routes,
    Adapter,
    ServerMiddleware,
    RenderHead,
    CacheHandler,
    Typegen,
    Diagnostics,
}

impl Hook {
    /// The subset the JS bridge supports (§15.2 "limited configureServer" etc.).
    pub fn supported_in_bridge(&self) -> bool {
        !matches!(self, Hook::ConfigureServer | Hook::CacheHandler)
    }
}

/// camelCase wire name of a hook (§15.2 table) for error messages.
fn hook_label(h: Hook) -> &'static str {
    match h {
        Hook::Config => "config",
        Hook::ConfigResolved => "configResolved",
        Hook::BuildStart => "buildStart",
        Hook::ResolveId => "resolveId",
        Hook::Load => "load",
        Hook::Transform => "transform",
        Hook::RenderChunk => "renderChunk",
        Hook::GenerateBundle => "generateBundle",
        Hook::BuildEnd => "buildEnd",
        Hook::TransformHtml => "transformHtml",
        Hook::HandleHotUpdate => "handleHotUpdate",
        Hook::ConfigureServer => "configureServer",
        Hook::Routes => "routes",
        Hook::Adapter => "adapter",
        Hook::ServerMiddleware => "serverMiddleware",
        Hook::RenderHead => "renderHead",
        Hook::CacheHandler => "cacheHandler",
        Hook::Typegen => "typegen",
        Hook::Diagnostics => "diagnostics",
    }
}

/// A plugin descriptor as it arrives from config/npm manifests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDescriptor {
    pub name: String,
    pub kind: PluginKind,
    pub enforce: Option<Enforce>,
    pub hooks: Vec<Hook>,
    pub options: serde_json::Value,
    /// Modules declare which adapters they support (§15.4).
    pub supported_adapters: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    Native,
    Wasm,
    Js,
}

/// The loaded, ordered plugin set.
#[derive(Debug, Clone, Default)]
pub struct PluginRegistry {
    pub plugins: Vec<PluginDescriptor>,
    pub load_errors: Vec<Diagnostic>,
}

impl PluginRegistry {
    /// Register a plugin. Unsupported hooks (per bridge capability) produce a load
    /// error with a compatibility-table link (§15.2) unless the plugin is native.
    ///
    /// - Native plugins bypass the bridge check (they never cross it, §15.1 kind 1).
    /// - A plugin whose name is already registered replaces the earlier entry in
    ///   place (same slot, same ordering position); the replacement is silent.
    /// - A non-native plugin with any bridge-unsupported hook is NOT added; a
    ///   `PLG-HOOK` diagnostic naming the plugin + hook + compatibility table is
    ///   pushed to `load_errors` (§15.2 "fail at load with a clear message").
    pub fn register(&mut self, plugin: PluginDescriptor) {
        if plugin.kind != PluginKind::Native {
            for hook in &plugin.hooks {
                if !hook.supported_in_bridge() {
                    self.load_errors.push(Diagnostic::error(
                        "PLG-HOOK",
                        format!(
                            "plugin `{}` declares hook `{}`, which the JS bridge does not support — see the compatibility table (§15.2/§15.3)",
                            plugin.name,
                            hook_label(*hook)
                        ),
                    ));
                    return;
                }
            }
        }
        match self.plugins.iter_mut().find(|p| p.name == plugin.name) {
            Some(slot) => *slot = plugin,
            None => self.plugins.push(plugin),
        }
    }

    /// Ordered hook chain: pre plugins, native, post — stable within a group.
    /// Only plugins that declare `hook` participate (§15.1: one hook model and
    /// ordering for all three kinds).
    pub fn chain(&self, hook: Hook) -> Vec<&PluginDescriptor> {
        let mut pre = Vec::new();
        let mut none = Vec::new();
        let mut post = Vec::new();
        for p in self.plugins.iter().filter(|p| p.hooks.contains(&hook)) {
            match p.enforce {
                Some(Enforce::Pre) => pre.push(p),
                Some(Enforce::Post) => post.push(p),
                None => none.push(p),
            }
        }
        pre.into_iter().chain(none).chain(post).collect()
    }

    pub fn has_errors(&self) -> bool {
        !self.load_errors.is_empty()
    }
}

/// JS plugin bridge protocol message (tooling pool, §15.1 kind 3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum BridgeMsg {
    HookCall {
        plugin: String,
        hook: String,
        args: serde_json::Value,
    },
    HookResult {
        plugin: String,
        hook: String,
        ok: bool,
        result: serde_json::Value,
    },
}

/// WASM plugin host stub (§15.1 kind 2): validates a WIT interface descriptor and
/// capability grants; actual execution behind the wasmtime backend (docs/deviations.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmPluginManifest {
    pub name: String,
    pub wit_world: String,
    /// Declared capabilities (§17.2 sandboxing): fs, network, env.
    pub capabilities: Vec<String>,
    pub hooks: Vec<Hook>,
}

impl WasmPluginManifest {
    /// Validate a WASM plugin manifest (§15.1 kind 2, §17.2 sandboxing):
    /// a non-empty `wit_world` (stable WIT interface), capabilities within the
    /// sandbox grant set {fs, network, env, clock} (`PLG-CAP`), at least one hook,
    /// and hooks within the bridge-supported subset — WASM plugins run the same
    /// bridge as JS plugins (`PLG-HOOK`, §15.2/§15.3 compatibility table).
    pub fn validate(&self) -> Result<()> {
        if self.wit_world.trim().is_empty() {
            return Err(err(
                "PLG-WIT",
                format!(
                    "WASM plugin `{}` declares no `wit_world` — a stable WIT interface is required (§15.1 kind 2)",
                    self.name
                ),
            ));
        }
        const SANDBOX_CAPS: [&str; 4] = ["fs", "network", "env", "clock"];
        for cap in &self.capabilities {
            if !SANDBOX_CAPS.contains(&cap.as_str()) {
                return Err(err(
                    "PLG-CAP",
                    format!(
                        "WASM plugin `{}` requests capability `{cap}` — allowed: fs, network, env, clock (§17.2 sandboxing)",
                        self.name
                    ),
                ));
            }
        }
        if self.hooks.is_empty() {
            return Err(err(
                "PLG-HOOKS",
                format!(
                    "WASM plugin `{}` declares no hooks — nothing to dispatch (§15.2)",
                    self.name
                ),
            ));
        }
        for hook in &self.hooks {
            if !hook.supported_in_bridge() {
                return Err(err(
                    "PLG-HOOK",
                    format!(
                        "WASM plugin `{}` declares hook `{}`, which the JS bridge does not support — see the compatibility table (§15.2/§15.3)",
                        self.name,
                        hook_label(*hook)
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// A module (`defineModule`, §15.4): distributable bundle of hooks, routes,
/// middleware, config defaults and typegen.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModuleManifest {
    pub name: String,
    pub adapters: Vec<String>,
    #[serde(default)]
    pub config_defaults: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub plugins: Vec<PluginDescriptor>,
    #[serde(default)]
    pub routes: Vec<String>,
}
