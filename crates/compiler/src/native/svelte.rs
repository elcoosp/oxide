//! The native Svelte compiler (§V2-C04) — `svelte-compiler` behind the
//! `native-svelte` feature.
//!
//! # Output contract
//!
//! The oracle is the `tooling:svelte-compile` handler in
//! `crates/jshost/worker/oxide-worker.mjs`: `svelte.compile(source, {
//! filename, generate: 'dom'|'ssr', css: 'injected' })`, then the
//! emitted `js.code`. The backend maps `Generate::Dom | Ssr` onto
//! `GenerateTarget::Client | Server` and passes `CssOutputMode::Injected`
//! — the exact options the worker hands the JS compiler. The worker's
//! best-effort sucrase type-strip has no native counterpart need: the
//! crate lowers the TS script itself, so the emitted module is
//! bundler-ready JS.

use crate::native::{CompileOutput, CompileRequest, Generate, NativeCompiler};
use crate::{OxideError, Result};
use svelte_compiler::{CompileOptions, CssOutputMode, GenerateTarget};

/// The native Svelte backend (`svelte-native`, `svelte-compiler 0.1.4`).
pub struct SvelteNative;

impl SvelteNative {
    /// The upstream crate version — the D40 CAS key dimension.
    #[must_use]
    pub fn upstream_version() -> &'static str {
        "0.1.4"
    }
}

impl NativeCompiler for SvelteNative {
    fn framework(&self) -> &'static str {
        "svelte"
    }

    fn id(&self) -> &'static str {
        "svelte-native"
    }

    fn version(&self) -> String {
        Self::upstream_version().to_string()
    }

    fn handles(&self, ext: &str) -> bool {
        ext == "svelte"
    }

    fn compile(&self, req: &CompileRequest<'_>) -> Result<CompileOutput> {
        let options = CompileOptions {
            filename: Some(req.path.display().to_string().into()),
            generate: if req.generate == Generate::Ssr {
                GenerateTarget::Server
            } else {
                GenerateTarget::Client
            },
            // The worker's `css: "injected"` — the style rides the emitted
            // JS, no external css artifact on this path.
            css: CssOutputMode::Injected,
            ..Default::default()
        };
        match svelte_compiler::compile(req.source, options) {
            Ok(result) => Ok(CompileOutput {
                js: result.js.code.to_string(),
                css: None,
                warnings: result
                    .warnings
                    .iter()
                    .map(|w| w.message.to_string())
                    .collect(),
                source_map: None,
            }),
            Err(e) => {
                let mut d = oxide_fw_diagnostics::Diagnostic::error(
                    "CMP-SVT-001",
                    format!(
                        "native svelte compile failed for {}: {} ({})",
                        req.path.display(),
                        e.message,
                        e.code
                    ),
                );
                d.fix = Some(
                    "set framework.compilerBackend to 'auto' or 'node' to use the Node tooling pool"
                        .into(),
                );
                Err(OxideError(Box::new(d)))
            }
        }
    }

    fn health_fixture(&self) -> (&'static str, &'static str) {
        ("health-fixture.svelte", "<p>ok</p>")
    }
}
