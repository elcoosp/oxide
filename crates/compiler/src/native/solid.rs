//! The native Solid compiler (§V2-C02, the dependency arm) — behind the
//! `native-solid` feature.
//!
//! # What this backend is
//!
//! A thin adapter around **Solid 2.0's official Oxc-based Rust compiler
//! core** — the `solidjs-compiler` crate, git-pinned at
//! `solidjs/solid@next` `packages/compiler` (the tree behind npm's
//! `@solidjs/compiler` 2.0.0-rc.21, the release the Solid 2.0 RC ships).
//! The earlier kill/re-open records (`docs/decisions/C02-solid-*.md`)
//! pinned "no sourceable Cargo tree" as the blocker for a dependency arm;
//! the `next` branch now publishes the crate, so the in-repo JSX/reactivity
//! transform the re-open landed was deleted and the seam wraps upstream —
//! the re-open record's own re-run condition, executed.
//!
//! The upstream crate pins its own oxc 0.144 stack; the workspace
//! transforms ride oxc 0.155. The seam is string in / string out (the C01
//! rule), so no oxc type crosses — the same shape as the `verter_core`
//! 0.117 and `oxc_react_compiler` 0.143 skews. `default-features = false`
//! keeps the build pure Rust (`rlib`): the `node` NAPI feature stays off
//! (no Node host in the Rust build) and the `tsrx` feature stays off (it
//! would drag a second, git-pinned oxc 0.140 fork; a `.tsrx` source
//! therefore refuses loudly, mapped onto `CMP-SOL-001` like every other
//! upstream failure).
//!
//! # The contract (the worker oracle)
//!
//! `tooling:solid-compile` in `crates/jshost/worker/oxide-worker.mjs`
//! passes the native binding `moduleName: "@solidjs/web"` (the Solid 2.0
//! runtime location), `generate: 'dom' | 'ssr'`, `hydratable: true` and
//! the source filename, then runs a TypeScript strip and the
//! `solid-js/web` → `@solidjs/web` import rewrite around the transform.
//! The upstream core IS the compiler the worker's `solid-jsx-oxc` binding
//! wraps, so the emitted shapes match the pool's native path by
//! construction; the adapter mirrors the worker's surrounding passes:
//!
//! - **The TypeScript strip runs BEFORE the compile** (the worker's babel
//!   fallback strips first too): the upstream core parses `.tsx` but does
//!   not strip annotations (its emitted module keeps `props: { start:
//!   number }` — the worker type-strips after the binding for the same
//!   reason). Stripping first keeps the upstream source map valid (it
//!   indexes the text the core actually compiled) — stripping after would
//!   shift every mapping. The strip rides the workspace oxc transformer
//!   (0.155) with `only_remove_type_imports` — the sucrase parity the
//!   re-open established — and JSX untouched (the Solid compile owns the
//!   JSX).
//! - **DOM** (`Generate::Dom`): `generate: Dom`, `hydratable: true` —
//!   `_$template` + `_$getNextElement`/`_$getNextMarker` walks,
//!   `_$scope` effects, `_$insert`, delegated `$$` events with the
//!   `_$delegateEvents`/`_$runHydrationEvents` tails, `<Show>` lowered to
//!   the memo + ternary in production.
//! - **SSR** (`Generate::Ssr`): `generate: Ssr`, `hydratable: true` —
//!   string-array `_$ssr` templates with `_$escape` text holes,
//!   `_$ssrAttribute` dynamic attributes and `_$ssrHydrationKey()` after
//!   every tag name. Client-only constructs (events, refs) simply do not
//!   emit on the SSR path (upstream's supported surface — the compile
//!   stays silent, no warning channel exists on its API).
//! - **Dev vs production**: `dev` maps onto the upstream `dev` flag, the
//!   `sourceNames` surface (the JS surface defaults each kind to the dev
//!   flag) and `lowerShow` (dev keeps the `<Show>` component and its
//!   stale-read error, production lowers). Dev builds also carry the
//!   upstream V3 source map (`CompileOutput::source_map` — the first
//!   native backend to emit one; the production pipeline does not consume
//!   per-file native maps yet, the pool emits none).
//! - **The import rewrite** runs AFTER the compile, verbatim the worker's
//!   post-transform rule: `solid-js/web` (the Solid 1.x runtime location
//!   some sources still import) → `@solidjs/web`, both quote spellings.
//!   The rewrite is offset-neutral — both specifiers are 12 chars — so
//!   the upstream source map stays valid through it.
//!
//! # Refusals
//!
//! Every upstream failure (parse, configuration, transform) maps onto
//! `CMP-SOL-001` — the Solid backend's one failure shape the errors
//! registry holds — with the upstream diagnostic riding the message and
//! the fix naming the Node tooling pool, the fallback that compiles what
//! the core refuses. The v1-scope refusal list the in-repo transform
//! carried (spreads, SSR fragments, object `style`/`classList`) is gone:
//! the 2.0 core compiles those natively; `use:` directives refuse (they
//! were removed in Solid 2.0) with the upstream migration note.

use crate::native::{CompileOutput, CompileRequest, Generate, NativeCompiler};
use crate::{OxideError, Result};
use solidjs_compiler::{
    CompileErrorKind, CompileOptions, Generate as UpstreamGenerate, SourceNames,
    compile as upstream_compile,
};

/// The npm release the pinned rev carries (the `@solidjs/compiler`
/// version) plus the short upstream rev — the D40 CAS dimension, bumped
/// with every upstream re-pin.
const BACKEND_VERSION: &str = "2.0.0-rc.21+7ea2a02";

/// The `@solidjs/web` runtime module the emitted code imports (the worker
/// passes `moduleName: "@solidjs/web"` — the Solid 2.0 runtime location).
const RUNTIME_MODULE: &str = "@solidjs/web";

/// The Solid 1.x runtime location the import rewrite moves (the worker's
/// post-transform rule; 12 chars, same as [`RUNTIME_MODULE`] — the
/// rewrite is offset-neutral so the upstream source map stays valid).
const LEGACY_RUNTIME_MODULE: &str = "solid-js/web";

/// The native Solid backend (`solid-native`, the upstream adapter).
pub struct SolidNative;

impl SolidNative {
    /// The wrapped upstream artifact — the D40 CAS key dimension (bump it
    /// with every upstream re-pin so the precompile cache invalidates).
    #[must_use]
    pub fn backend_version() -> &'static str {
        BACKEND_VERSION
    }
}

impl NativeCompiler for SolidNative {
    fn framework(&self) -> &'static str {
        "solid"
    }

    fn id(&self) -> &'static str {
        "solid-native"
    }

    fn version(&self) -> String {
        Self::backend_version().to_string()
    }

    fn handles(&self, ext: &str) -> bool {
        ext == "tsx" || ext == "jsx"
    }

    fn compile(&self, req: &CompileRequest<'_>) -> Result<CompileOutput> {
        // The TypeScript strip runs BEFORE the compile (the worker strips
        // around its transform too): the upstream core parses `.tsx` but
        // does not strip annotations, and stripping after the compile
        // would shift every source-map mapping. JSX untouched — the Solid
        // compile owns the JSX.
        let source = strip_types(req)?;
        // The JS-surface mappings (the Node adapter applies the same
        // shapes): dev names every binding and keeps the `<Show>`
        // component (its stale-read error), production lowers; dev also
        // carries the upstream V3 source map (the production pipeline
        // does not consume per-file native maps yet — the pool emits
        // none).
        let options = CompileOptions {
            filename: Some(req.path.display().to_string()),
            module_name: RUNTIME_MODULE.to_string(),
            hydratable: true,
            generate: match req.generate {
                Generate::Dom => UpstreamGenerate::Dom,
                Generate::Ssr => UpstreamGenerate::Ssr,
            },
            dev: req.dev,
            source_names: SourceNames::all(req.dev),
            lower_show: !req.dev,
            source_map: req.dev,
            ..CompileOptions::default()
        };

        let out = upstream_compile(&source, &options).map_err(|e| refusal(req, &e))?;
        // The worker's post-transform rewrite, verbatim: the Solid 1.x
        // runtime location → the 2.0 module, both quote spellings (the
        // upstream codegen normalizes to double quotes; the single-quote
        // arm is the worker's parity). Offset-neutral — the map survives.
        let js = out
            .code
            .replace(
                &format!("from \"{LEGACY_RUNTIME_MODULE}\""),
                &format!("from \"{RUNTIME_MODULE}\""),
            )
            .replace(
                &format!("from '{LEGACY_RUNTIME_MODULE}'"),
                &format!("from '{RUNTIME_MODULE}'"),
            );
        Ok(CompileOutput {
            js,
            css: None,
            warnings: Vec::new(),
            source_map: out.source_map,
        })
    }

    fn health_fixture(&self) -> (&'static str, &'static str) {
        ("health-fixture.tsx", "export default () => <p>ok</p>;\n")
    }
}

/// The `CMP-SOL-001` refusal (the Solid backend's one failure shape — the
/// upstream stage and diagnostic ride the message; the fix names the Node
/// tooling pool, the fallback every refused construct compiles with).
fn refusal(req: &CompileRequest<'_>, e: &solidjs_compiler::CompileError) -> OxideError {
    let stage = match e.kind() {
        CompileErrorKind::Parse => "parse",
        CompileErrorKind::Configuration => "configuration",
        CompileErrorKind::Transform => "transform",
    };
    let mut d = oxide_fw_diagnostics::Diagnostic::error(
        "CMP-SOL-001",
        format!(
            "native solid compile failed for {}: the upstream core's {stage} failed ({})",
            req.path.display(),
            e.message()
        ),
    );
    d.fix = Some(
        "set framework.compilerBackend to 'auto' or 'node' to use the Node tooling pool".into(),
    );
    OxideError(Box::new(d))
}

/// The TypeScript strip over the request's source (the worker's sucrase
/// `typescript` pass, on the workspace oxc stack): annotations removed at
/// the AST level, JSX untouched (the JSX lowering is the separate React
/// pass, which this pipeline does NOT run — the Solid compile owns the
/// JSX), unused VALUE imports kept (`only_remove_type_imports` — the
/// sucrase parity the re-open established: the strip never elides
/// imports). The source prints back and the printed text is what the
/// upstream core compiles, so its diagnostics and source map index the
/// text the pipeline actually carried.
fn strip_types(req: &CompileRequest<'_>) -> Result<String> {
    let source_type = if req.path.extension().is_some_and(|e| e == "jsx") {
        oxc_span::SourceType::jsx()
    } else {
        oxc_span::SourceType::tsx()
    };
    let allocator = oxc_allocator::Allocator::default();
    let parsed = oxc_parser::Parser::new(&allocator, req.source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        let first = parsed
            .diagnostics
            .iter()
            .map(ToString::to_string)
            .next()
            .unwrap_or_else(|| "no diagnostic detail".to_string());
        return Err(type_strip_err(
            req,
            &format!("the source does not parse ({first})"),
        ));
    }
    let mut program = parsed.program;
    let semantic = oxc_semantic::SemanticBuilder::new().build(&program);
    let scoping = semantic.semantic.into_scoping();
    let options = oxc_transformer::TransformOptions {
        // `only_remove_type_imports` keeps unused VALUE imports — the
        // sucrase parity (the worker's strip never elides imports).
        typescript: oxc_transformer::TypeScriptOptions {
            only_remove_type_imports: true,
            ..oxc_transformer::TypeScriptOptions::default()
        },
        // The React JSX lowering is the one pass this pipeline must NOT
        // run (the Solid compile owns the JSX; the worker's sucrase pass
        // keeps JSX verbatim too).
        jsx: oxc_transformer::JsxOptions {
            jsx_plugin: false,
            development: false,
            display_name_plugin: false,
            jsx_self_plugin: false,
            jsx_source_plugin: false,
            pure: false,
            ..oxc_transformer::JsxOptions::default()
        },
        ..oxc_transformer::TransformOptions::default()
    };
    let stripped = oxc_transformer::Transformer::new(&allocator, req.path, &options)
        .build_with_scoping(scoping, &mut program);
    if !stripped.diagnostics.is_empty() {
        let first = stripped
            .diagnostics
            .iter()
            .map(ToString::to_string)
            .next()
            .unwrap_or_else(|| "no diagnostic detail".to_string());
        return Err(type_strip_err(
            req,
            &format!("the TypeScript strip failed ({first})"),
        ));
    }
    Ok(oxc_codegen::Codegen::new()
        .with_options(oxc_codegen::CodegenOptions::default())
        .build(&program)
        .code)
}

/// The strip-stage refusal — the same `CMP-SOL-001` shape the upstream
/// errors map onto, with the strip's own diagnostic riding the message.
fn type_strip_err(req: &CompileRequest<'_>, detail: &str) -> OxideError {
    let mut d = oxide_fw_diagnostics::Diagnostic::error(
        "CMP-SOL-001",
        format!(
            "native solid compile failed for {}: {detail}",
            req.path.display()
        ),
    );
    d.fix = Some(
        "set framework.compilerBackend to 'auto' or 'node' to use the Node tooling pool".into(),
    );
    OxideError(Box::new(d))
}
