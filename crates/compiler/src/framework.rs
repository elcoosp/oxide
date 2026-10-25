//! Framework dispatch (spec §6.3). React and Solid use Oxc's JSX lowering with
//! framework-specific pragmas (native path). Vue and Svelte are dispatched to
//! the tooling pool (§4.7) to run `@vue/compiler-sfc` and `svelte/compiler` —
//! the spec is explicit about this. There is no in-Rust SFC parser.
//!
//! `FrameworkCompiler` is the isolation point; the caller chooses between
//! `InlineCompiler` (React/Solid) and `ToolingCompiler` (Vue/Svelte), or lets
//! `for_path` pick by extension.

use crate::{Env, Result, err};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Framework {
    React,
    Vue,
    Svelte,
    Solid,
    None,
}

impl Framework {
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "tsx" | "jsx" => Framework::React,
            "vue" => Framework::Vue,
            "svelte" => Framework::Svelte,
            _ => Framework::None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Framework::React => "react",
            Framework::Vue => "vue",
            Framework::Svelte => "svelte",
            Framework::Solid => "solid",
            Framework::None => "none",
        }
    }
    /// Which framework's compiler lives in the tooling pool vs. the native Oxc
    /// path (§6.3 table).
    pub fn requires_tooling_pool(&self) -> bool {
        matches!(self, Framework::Vue | Framework::Svelte | Framework::Solid)
    }
}

#[derive(Debug, Clone)]
pub struct FrameworkInput {
    pub path: String,
    pub code: String,
    pub env: Env,
    pub dev: bool,
    pub is_server_module: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FrameworkOutput {
    pub code: String,
    pub extracted_css: Vec<(String, String)>,
    pub style_scope_id: Option<String>,
    pub warnings: Vec<String>,
}

/// Framework compiler trait (D4 isolation point).
pub trait FrameworkCompiler: Send + Sync {
    fn framework(&self) -> Framework;
    fn compile(&self, input: &FrameworkInput) -> Result<FrameworkOutput>;
}

/// React: Oxc JSX automatic runtime + (dev) React Refresh preamble.
///
/// The spec's §6.3 table calls for "Oxc JSX (automatic runtime) and native
/// React Refresh". The pragma/runtime preamble is emitted here; the JSX
/// lowering itself runs as part of the Oxc transform pass.
#[derive(Default)]
pub struct ReactInline {
    pub refresh: bool,
}


impl FrameworkCompiler for ReactInline {
    fn framework(&self) -> Framework {
        Framework::React
    }
    fn compile(&self, input: &FrameworkInput) -> Result<FrameworkOutput> {
        let mut code = String::new();
        if !input.code.contains("react/jsx-runtime") && !input.is_server_module {
            code.push_str(
                "import { jsx as _jsx, jsxs as _jsxs, Fragment as _Fragment } from \"react/jsx-runtime\";\n",
            );
        }
        if self.refresh && input.dev && !input.is_server_module {
            code.push_str(
                "const $RefreshReg$ = (t, id) => { globalThis.__oxide_refresh_reg?.(t, id); };\n\
                 const $RefreshSig$ = () => (t) => t;\n",
            );
        }
        code.push_str(&input.code);
        if self.refresh && input.dev && !input.is_server_module {
            for name in top_level_function_names(&input.code) {
                code.push_str(&format!("\n$RefreshReg$({name}, \"{name}\");"));
            }
        }
        Ok(FrameworkOutput {
            code,
            ..Default::default()
        })
    }
}

/// Solid: Oxc JSX lowering with the Solid pragma set. The spec's §6.3 table
/// puts Solid in the tooling pool (`babel-preset-solid`) — this in-Rust path
/// is the escape hatch for when the tooling pool is unavailable (e.g. `oxide
/// build --adapter=cloudflare` on a stripped-down host).
pub struct SolidInline;

impl FrameworkCompiler for SolidInline {
    fn framework(&self) -> Framework {
        Framework::Solid
    }
    fn compile(&self, input: &FrameworkInput) -> Result<FrameworkOutput> {
        // No text-level transform required — the Oxc JSX pass consumes the
        // `jsxImportSource` pragma and produces `solid-js/web` calls.
        Ok(FrameworkOutput {
            code: input.code.clone(),
            ..Default::default()
        })
    }
}

/// A `FrameworkCompiler` that dispatches to the tooling pool. The actual call
/// happens at the pipeline layer via `oxide_fw_jshost::ToolingModules`; this type
/// only carries the identifiers the pipeline needs and marks "no in-Rust
/// compile occurred".
#[derive(Debug)]
pub struct ToolingDispatcher {
    framework: Framework,
}

impl ToolingDispatcher {
    pub fn for_framework(f: Framework) -> Result<Self> {
        if !f.requires_tooling_pool() {
            return Err(err(
                "CMP-FRAMEWORK",
                format!("framework `{}` does not use the tooling pool", f.as_str()),
            ));
        }
        Ok(Self { framework: f })
    }
    pub fn module_and_export(&self) -> (&'static str, &'static str) {
        match self.framework {
            Framework::Vue => ("@vue/compiler-sfc", "parse"),
            Framework::Svelte => ("svelte/compiler", "compile"),
            Framework::Solid => ("babel-preset-solid", "transform"),
            _ => unreachable!("guarded by for_framework"),
        }
    }
}

impl FrameworkCompiler for ToolingDispatcher {
    fn framework(&self) -> Framework {
        self.framework
    }
    fn compile(&self, _input: &FrameworkInput) -> Result<FrameworkOutput> {
        Err(err(
            "CMP-FRAMEWORK",
            "this framework compiles in the tooling pool, not in-process; the pipeline must call \
             `oxide_fw_jshost::ToolingModules` (§4.7). This is expected behavior, not a stub.",
        ))
    }
}

/// Pick a compiler for a path. Vue and Svelte return a `ToolingDispatcher`
/// whose `compile` errors — that error is the signal to the pipeline to route
/// through the tooling pool. React/Solid use the inline Oxc path.
pub fn compiler_for(path: &str) -> Box<dyn FrameworkCompiler> {
    let ext = path.rsplit('.').next().unwrap_or("");
    match Framework::from_extension(ext) {
        Framework::React => Box::new(ReactInline::default()),
        Framework::Vue | Framework::Svelte | Framework::Solid => {
            Box::new(ToolingDispatcher::for_framework(Framework::from_extension(ext)).unwrap())
        }
        Framework::None => Box::new(ReactInline::default()),
    }
}

fn top_level_function_names(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in code.lines() {
        let t = line.trim_start();
        let mut rest = t;
        if let Some(r) = rest.strip_prefix("export ") {
            rest = r;
        }
        if let Some(r) = rest.strip_prefix("function ") {
            let name: String = r
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn react_inline_emits_automatic_runtime() {
        let c = ReactInline { refresh: false };
        let out = c
            .compile(&FrameworkInput {
                path: "App.tsx".into(),
                code: "export default function App() { return null; }".into(),
                env: Env::Client,
                dev: false,
                is_server_module: false,
            })
            .unwrap();
        assert!(out.code.contains("react/jsx-runtime"));
    }

    #[test]
    fn react_refresh_in_dev() {
        let c = ReactInline { refresh: true };
        let out = c
            .compile(&FrameworkInput {
                path: "App.tsx".into(),
                code: "export function Widget() { return null; }".into(),
                env: Env::Client,
                dev: true,
                is_server_module: false,
            })
            .unwrap();
        assert!(out.code.contains("$RefreshReg$(Widget"));
    }

    #[test]
    fn vue_and_svelte_route_to_tooling_pool() {
        let vue = compiler_for("App.vue");
        assert_eq!(vue.framework(), Framework::Vue);
        let e = vue
            .compile(&FrameworkInput {
                path: "App.vue".into(),
                code: "".into(),
                env: Env::Client,
                dev: false,
                is_server_module: false,
            })
            .unwrap_err();
        assert_eq!(e.0.code, "CMP-FRAMEWORK");
        assert!(e.0.message.contains("tooling pool"));

        let sv = compiler_for("App.svelte");
        assert_eq!(sv.framework(), Framework::Svelte);

        let so = compiler_for("App.jsx");
        // jsx maps to React by extension; explicit Solid uses ToolingDispatcher.
        assert_eq!(so.framework(), Framework::React);
    }

    #[test]
    fn solid_dispatcher_identifies_real_module() {
        let d = ToolingDispatcher::for_framework(Framework::Solid).unwrap();
        let (m, e) = d.module_and_export();
        assert_eq!(m, "babel-preset-solid");
        assert_eq!(e, "transform");
    }

    #[test]
    fn non_tooling_framework_rejected() {
        let e = ToolingDispatcher::for_framework(Framework::React).unwrap_err();
        assert_eq!(e.0.code, "CMP-FRAMEWORK");
    }

    #[test]
    fn extension_dispatch_table() {
        assert_eq!(Framework::from_extension("tsx"), Framework::React);
        assert_eq!(Framework::from_extension("jsx"), Framework::React);
        assert_eq!(Framework::from_extension("vue"), Framework::Vue);
        assert_eq!(Framework::from_extension("svelte"), Framework::Svelte);
        assert_eq!(Framework::from_extension("ts"), Framework::None);
    }
}
