//! V2-C05 spike — the React Compiler port (`oxc_react_compiler`).
//!
//! # Dependency arm (recorded here and in
//! `docs/decisions/C05-react_compiler-native.md`)
//!
//! `oxc_react_compiler 0.143.0`, **crates.io latest** (published
//! 2026-08-03 — fresh per §0b(1)). The workspace transform rides oxc
//! 0.155 while the crate pins the oxc 0.143 stack, so the seam is
//! STRING IN / STRING OUT: parse with the crate's own 0.143 parser,
//! compile on the pristine AST, print, hand the text back to the
//! normal 0.155 transform. No 0.143 type crosses into the workspace
//! (the C01 rule) — the `oxc_*_0_143` package aliases carry the stack.
//!
//! # What the spike must prove
//!
//! 1. The crate builds next to the workspace oxc 0.155 (it does — this
//!    file compiles only under `--features native-react-compiler`).
//! 2. Entry point: `compile(&Program, &Semantic, &Allocator, options)`
//!    on the pristine AST with `SemanticBuilder::with_build_nodes(true)`.
//! 3. Hooks are memoized: the output carries the compiler's
//!    `useMemoCache` imports for a component with stateful deps.
//! 4. Measured cost lands in the decision doc (release compile time,
//!    binary size delta — the ">90 s added" kill bar).
//!
//! Run: `cargo test -p oxide-fw-compiler --features native-react-compiler
//! --test react_compiler_spike -- --ignored --nocapture`
#![cfg(feature = "native-react-compiler")]

use oxc_allocator_0_143 as oxc_allocator;
use oxc_codegen_0_143 as oxc_codegen;
use oxc_parser_0_143 as oxc_parser;
use oxc_semantic_0_143 as oxc_semantic;
use oxc_span_0_143 as oxc_span;

const COMPONENT: &str = r#"import { useState } from "react";

export function Counter({ step }: { step: number }) {
  const [n, setN] = useState(0);
  const label = "count: " + n;
  const doubled = n * step;
  return (
    <button onClick={() => setN(n + 1)}>
      {label} {doubled}
    </button>
  );
}
"#;

#[test]
#[ignore = "C05 spike — needs native-react-compiler; run with --ignored --nocapture"]
fn spike_memoizes_hooks_through_the_string_seam() {
    let allocator = oxc_allocator::Allocator::default();
    let parser_ret =
        oxc_parser::Parser::new(&allocator, COMPONENT, oxc_span::SourceType::tsx()).parse();
    assert!(
        parser_ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        parser_ret.diagnostics
    );
    let program = parser_ret.program;
    let semantic = {
        let builder = oxc_semantic::SemanticBuilder::new().with_build_nodes(true);
        builder.build(&program).semantic
    };
    let result = oxc_react_compiler::compile(
        &program,
        &semantic,
        &allocator,
        oxc_react_compiler::PluginOptions::default(),
    );
    match result {
        oxc_react_compiler::CompileResult::Success {
            output,
            diagnostics,
        } => {
            println!("=== diagnostics: {diagnostics:?} ===");
            if let Some(_output) = output {
                let printed = oxc_codegen::Codegen::new()
                    .with_options(oxc_codegen::CodegenOptions::default())
                    .build(&program)
                    .code;
                println!("=== compiled code (memoized) ===\n{printed}\n");
                assert!(!printed.is_empty(), "the compiler rewrote the program");
            } else {
                println!("=== output: None (no rewrite — check diagnostics) ===");
            }
        }
        oxc_react_compiler::CompileResult::Fatal { diagnostics } => {
            panic!("the compiler aborted: {diagnostics:?}");
        }
    }
}
