//! The React Compiler port (§V2-C05) — `oxc_react_compiler` behind the
//! `native-react-compiler` feature, applied BEFORE the JSX lowering.
//!
//! # The seam
//!
//! The crate pins its own oxc 0.143 stack while the workspace transform
//! rides oxc 0.155, so the boundary is STRING IN / STRING OUT: parse
//! with the 0.143 parser, compile on the pristine AST
//! (`SemanticBuilder::with_build_nodes(true)` — the crate's own doc
//! contract), print, and hand the text back to the normal oxc 0.155
//! transform. No 0.143 type crosses into the workspace. The backend
//! gates on the existing `framework.compiler: true` flag at the call
//! site (the precompile integration checks `cfg.framework.compiler`);
//! React's existing native path is unchanged — this only adds the
//! optional memoizer.

use crate::native::{CompileOutput, CompileRequest, NativeCompiler};
use crate::{OxideError, Result};
use oxc_allocator_0_143 as oxc_allocator;
use oxc_codegen_0_143 as oxc_codegen;
use oxc_diagnostics_0_143 as oxc_diagnostics;
use oxc_parser_0_143 as oxc_parser;
use oxc_semantic_0_143 as oxc_semantic;
use oxc_span_0_143 as oxc_span;

/// The native React Compiler backend (`react-compiler-native`,
/// `oxc_react_compiler 0.143.0`).
pub struct ReactCompilerNative;

impl ReactCompilerNative {
    /// The upstream crate version — the D40 CAS key dimension.
    #[must_use]
    pub fn upstream_version() -> &'static str {
        "0.143.0"
    }
}

impl NativeCompiler for ReactCompilerNative {
    fn framework(&self) -> &'static str {
        "react"
    }

    fn id(&self) -> &'static str {
        "react-compiler-native"
    }

    fn version(&self) -> String {
        Self::upstream_version().to_string()
    }

    fn handles(&self, ext: &str) -> bool {
        ext == "tsx" || ext == "jsx"
    }

    fn compile(&self, req: &CompileRequest<'_>) -> Result<CompileOutput> {
        let allocator = oxc_allocator::Allocator::default();
        let parser_ret =
            oxc_parser::Parser::new(&allocator, req.source, oxc_span::SourceType::tsx()).parse();
        let program = parser_ret.program;
        if parser_ret
            .diagnostics
            .iter()
            .any(|d| d.severity == oxc_diagnostics::Severity::Error)
        {
            // The compiler needs a parseable module; a broken source is a
            // hard failure (the JSX lowering would fail on it too).
            let first = parser_ret
                .diagnostics
                .iter()
                .find(|d| d.severity == oxc_diagnostics::Severity::Error)
                .map_or_else(|| "parse failed".to_string(), ToString::to_string);
            let mut d = oxide_fw_diagnostics::Diagnostic::error(
                "CMP-RCC-001",
                format!(
                    "native react-compiler parse failed for {}: {first}",
                    req.path.display()
                ),
            );
            d.fix = Some(
                "set framework.compilerBackend to 'auto' or 'node' to use the Node tooling pool"
                    .into(),
            );
            return Err(OxideError(Box::new(d)));
        }
        // `compile` must run FIRST, on the pristine AST, with the
        // semantic built from the same tree (the crate's doc contract).
        let result = {
            let builder = oxc_semantic::SemanticBuilder::new().with_build_nodes(true);
            let semantic = builder.build(&program).semantic;
            oxc_react_compiler::compile(
                &program,
                &semantic,
                &allocator,
                oxc_react_compiler::PluginOptions::default(),
            )
        };
        match result {
            oxc_react_compiler::CompileResult::Success {
                output,
                diagnostics,
            } => {
                let warnings: Vec<String> =
                    diagnostics.into_iter().map(|d| d.to_string()).collect();
                if output.is_none() {
                    // Nothing to rewrite — the pass-through is the output
                    // (the JSX lowering consumes the original source).
                    return Ok(CompileOutput {
                        js: req.source.to_string(),
                        css: None,
                        warnings,
                        source_map: None,
                    });
                }
                let printed = oxc_codegen::Codegen::new()
                    .with_options(oxc_codegen::CodegenOptions::default())
                    .build(&program)
                    .code;
                Ok(CompileOutput {
                    js: printed,
                    css: None,
                    warnings,
                    source_map: None,
                })
            }
            oxc_react_compiler::CompileResult::Fatal { diagnostics } => {
                let mut d = oxide_fw_diagnostics::Diagnostic::error(
                    "CMP-RCC-001",
                    format!(
                        "native react-compiler aborted for {}: {:?}",
                        req.path.display(),
                        diagnostics
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
        (
            "health-fixture.tsx",
            "import { useState } from \"react\";\nexport function H() { const [n] = useState(0); return <b>{n}</b>; }\n",
        )
    }
}
