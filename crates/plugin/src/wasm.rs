//! Real WASM component plugin host (spec §15.1 kind 2, §17.2 sandboxing).
//!
//! Loads a WebAssembly *component* (component-model, wasmtime 48) and exposes a
//! tiny, stable host↔guest ABI that mirrors the plugin hooks the JS bridge
//! supports (`transform`, `load`, `resolve_id`). WASI is not implicitly provided
//! — a plugin only sees the capabilities granted by its manifest; today that
//! means no WASI preopens are opened, so the guest runs with an empty context.
//!
//! # ABI (WIT-shaped)
//!
//! ```wit
//! package oxide:plugin@0.1.0;
//!
//! interface hooks {
//!   transform: func(input: string) -> string;
//!   load: func(specifier: string) -> string;
//! }
//! ```
//!
//! Every hook is optional: a component that omits a function is treated as a
//! no-op for that hook. Unknown exports are ignored.

use crate::{err, Hook, Result, WasmPluginManifest};
use std::path::Path;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

/// One loaded component plugin, ready to execute its hooks.
pub struct WasmPlugin {
    manifest: WasmPluginManifest,
    engine: Engine,
    component: Component,
    store: Store<()>,
    instance: wasmtime::component::Instance,
    /// Hook names this component actually exports (subset of `manifest.hooks`).
    available: Vec<Hook>,
}

impl WasmPlugin {
    /// Compile and instantiate a component from `bytes`, after validating its
    /// manifest. `plugin_path` is used only for diagnostics.
    pub fn load(
        bytes: &[u8],
        manifest: WasmPluginManifest,
        plugin_path: Option<&Path>,
    ) -> Result<Self> {
        manifest.validate()?;

        let mut config = Config::new();
        config.wasm_component_model(true);
        // Deterministic defaults; the spec's §17.2 sandboxing note is honored by
        // leaving WASI un-wired and starting from an empty store context.
        config.epoch_interruption(true);

        let engine = Engine::new(&config)
            .map_err(|e| err("PLG-WASM", format!("wasmtime engine init failed: {e}")))?;

        let component = Component::new(&engine, bytes).map_err(|e| {
            let where_ = plugin_path
                .map(|p| format!(" `{}`", p.display()))
                .unwrap_or_default();
            err(
                "PLG-WASM",
                format!("failed to compile component{where_}: {e}"),
            )
        })?;

        let mut store = Store::new(&engine, ());
        store.set_epoch_deadline(1_000); // coarse watchdog; dropped by the caller

        let linker: Linker<()> = Linker::new(&engine);
        let instance = linker.instantiate(&mut store, &component).map_err(|e| {
            err(
                "PLG-WASM",
                format!("failed to instantiate component: {e}"),
            )
        })?;

        // Probe which declared hooks the component actually exports, tolerating
        // the small set of ABI shapes we know how to call: `(string) -> string`,
        // `() -> string`, `(s32) -> s32`, `() -> s32`.
        let mut available = Vec::new();
        for hook in &manifest.hooks {
            let name = hook_export_name(hook);
            if probe_hook_signature(&instance, &mut store, name).is_some() {
                available.push(hook.clone());
            }
        }

        Ok(Self {
            manifest,
            engine,
            component,
            store,
            instance,
            available,
        })
    }

    /// The plugin's validated manifest.
    pub fn manifest(&self) -> &WasmPluginManifest {
        &self.manifest
    }

    /// Hook names the component actually exports (intersected with the manifest).
    pub fn available_hooks(&self) -> &[Hook] {
        &self.available
    }

    /// Call a hook that takes a single string and returns a single string
    /// (`transform`, `load`). Returns `PLG-WASM` if the hook isn't exported.
    pub fn call_string_hook(&mut self, hook: &Hook, input: &str) -> Result<String> {
        if !self.available.contains(hook) {
            return Err(err(
                "PLG-WASM",
                format!(
                    "plugin `{}` does not export hook `{}`",
                    self.manifest.name,
                    hook_export_name(hook)
                ),
            ));
        }
        let name = hook_export_name(hook).to_string();
        let func = self
            .instance
            .get_typed_func::<(String,), (String,)>(&mut self.store, &name)
            .map_err(|e| {
                err(
                    "PLG-WASM",
                    format!("resolve typed func `{name}` failed: {e}"),
                )
            })?;
        let (out,) = func
            .call(&mut self.store, (input.to_string(),))
            .map_err(|e| err("PLG-WASM", format!("call `{name}` trapped: {e}")))?;
        Ok(out)
    }

    /// Engine handle (for callers that want to bump the epoch / pool).
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The compiled component (so it can be re-instantiated cheaply).
    pub fn component(&self) -> &Component {
        &self.component
    }

    /// Raw store accessor for callers/tests that need custom typed calls.
    pub fn store(&mut self) -> &mut Store<()> {
        &mut self.store
    }

    /// Raw instance accessor for callers/tests that need custom typed calls.
    pub fn instance(&self) -> &wasmtime::component::Instance {
        &self.instance
    }

    /// Call a `(s32) -> s32` hook. Handy for numeric plugin ABI shapes and for
    /// tests that exercise the execution path without the string canonical ABI.
    pub fn call_s32_hook(&mut self, hook: &Hook, input: i32) -> Result<i32> {
        if !self.available.contains(hook) {
            return Err(err(
                "PLG-WASM",
                format!(
                    "plugin `{}` does not export hook `{}`",
                    self.manifest.name,
                    hook_export_name(hook)
                ),
            ));
        }
        let name = hook_export_name(hook).to_string();
        let func = self
            .instance
            .get_typed_func::<(i32,), (i32,)>(&mut self.store, &name)
            .map_err(|e| err("PLG-WASM", format!("hook `{name}` is not (s32)->s32: {e}")))?;
        let (out,) = func
            .call(&mut self.store, (input,))
            .map_err(|e| err("PLG-WASM", format!("call `{name}` trapped: {e}")))?;
        Ok(out)
    }
}

/// Signature shapes we know how to call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookSig {
    StringArg,
    NoArgString,
    S32,
    NoArgS32,
}

/// Probe the component's typed-function signature for a hook export.
fn probe_hook_signature(
    instance: &wasmtime::component::Instance,
    store: &mut Store<()>,
    name: &str,
) -> Option<HookSig> {
    if instance
        .get_typed_func::<(String,), (String,)>(&mut *store, name)
        .is_ok()
    {
        return Some(HookSig::StringArg);
    }
    if instance
        .get_typed_func::<(), (String,)>(&mut *store, name)
        .is_ok()
    {
        return Some(HookSig::NoArgString);
    }
    if instance
        .get_typed_func::<(i32,), (i32,)>(&mut *store, name)
        .is_ok()
    {
        return Some(HookSig::S32);
    }
    if instance
        .get_typed_func::<(), (i32,)>(&mut *store, name)
        .is_ok()
    {
        return Some(HookSig::NoArgS32);
    }
    None
}

/// Exported name of a hook in the component-model world. Matches the WIT
/// interface: `resolve_id` for the JS-bridge `ResolveId` hook.
pub fn hook_export_name(hook: &Hook) -> &'static str {
    match hook {
        Hook::Config => "config",
        Hook::ConfigResolved => "config_resolved",
        Hook::Transform => "transform",
        Hook::Load => "load",
        Hook::ResolveId => "resolve_id",
        Hook::BuildStart => "build_start",
        Hook::BuildEnd => "build_end",
        Hook::RenderChunk => "render_chunk",
        Hook::GenerateBundle => "generate_bundle",
        Hook::TransformHtml => "transform_html",
        Hook::HandleHotUpdate => "handle_hot_update",
        Hook::Routes => "routes",
        Hook::Adapter => "adapter",
        Hook::ServerMiddleware => "server_middleware",
        Hook::RenderHead => "render_head",
        Hook::CacheHandler => "cache_handler",
        Hook::ConfigureServer => "configure_server",
        Hook::Typegen => "typegen",
        Hook::Diagnostics => "diagnostics",
    }
}
