//! The native Vue SFC compiler (§V2-C03) — `verter_core` behind the
//! `native-vue` feature.
//!
//! # Output contract
//!
//! The oracle is the `tooling:vue-compile` handler in
//! `crates/jshost/worker/oxide-worker.mjs`: parse the SFC, compile the
//! script with the template inlined (`compileScript`'s
//! `inlineTemplate: true` — verter's `CodegenOptions::inline`), derive
//! the component/scope id from the FILENAME (the worker passes the
//! base64-alnum short id; verter derives its stable id from the same
//! filename — the spike pins the id identical across SSR and client
//! runs), and strip the TypeScript annotations (`force_js` — the
//! worker's sucrase pass). SSR mode maps `Generate::Ssr` onto
//! `VerterCompileOptions::ssr` (the string-concatenation render fn).
//!
//! The worker returns only the script module and drops `<style>` blocks
//! on this path; the backend mirrors that exactly (the extracted CSS is
//! NOT returned — the CLI's `css` contract on this op is empty), so the
//! `compiler-equiv.sh vue` diff stays byte-honest.

use crate::native::{CompileOutput, CompileRequest, Generate, NativeCompiler};
use crate::{OxideError, Result};
use verter_core::compile::compile;
use verter_core::compile::types::{CodegenOptions, CompileTarget, VerterCompileOptions};

/// The native Vue backend (`vue-native`, verter `0.0.1-beta.1`).
pub struct VueNative;

impl VueNative {
    /// The upstream crate version — the D40 CAS key dimension.
    #[must_use]
    pub fn upstream_version() -> &'static str {
        "0.0.1-beta.1"
    }
}

impl NativeCompiler for VueNative {
    fn framework(&self) -> &'static str {
        "vue"
    }

    fn id(&self) -> &'static str {
        "vue-native"
    }

    fn version(&self) -> String {
        Self::upstream_version().to_string()
    }

    fn handles(&self, ext: &str) -> bool {
        ext == "vue"
    }

    fn compile(&self, req: &CompileRequest<'_>) -> Result<CompileOutput> {
        let allocator = oxc_allocator_0_117::Allocator::default();
        let options = CodegenOptions {
            filename: Some(req.path.display().to_string()),
            // Dev keeps the render fn out of setup(); production inlines
            // it (the compileScript `inlineTemplate` shape the worker
            // compiles with). The precompile pass is a production build.
            inline: Some(!req.dev),
            target: CompileTarget::BUNDLER,
            ..Default::default()
        };
        let verter_options = VerterCompileOptions {
            ssr: req.generate == Generate::Ssr,
            // `<script setup lang="ts">` keeps annotations — force_js
            // strips them (the worker's sucrase pass).
            force_js: true,
            ..Default::default()
        };
        let result = compile(req.source, &options, &verter_options, &allocator);
        let errors: Vec<String> = result.errors.iter().map(|e| e.message.clone()).collect();
        if let Some(first) = errors.first() {
            let mut d = oxide_fw_diagnostics::Diagnostic::error(
                "CMP-VUE-001",
                format!(
                    "native vue compile failed for {}: {first}",
                    req.path.display()
                ),
            );
            d.fix = Some(
                "set framework.compilerBackend to 'auto' or 'node' to use the Node tooling pool"
                    .into(),
            );
            return Err(OxideError(Box::new(d)));
        }
        let script = result.script.as_ref().ok_or_else(|| {
            let mut d = oxide_fw_diagnostics::Diagnostic::error(
                "CMP-VUE-001",
                format!(
                    "native vue compile produced no script module for {}",
                    req.path.display()
                ),
            );
            d.fix = Some(
                "set framework.compilerBackend to 'auto' or 'node' to use the Node tooling pool"
                    .into(),
            );
            OxideError(Box::new(d))
        })?;
        Ok(CompileOutput {
            js: script.code.clone(),
            css: None,
            warnings: errors,
        })
    }

    fn health_fixture(&self) -> (&'static str, &'static str) {
        ("health-fixture.vue", "<template><p>ok</p></template>")
    }
}
