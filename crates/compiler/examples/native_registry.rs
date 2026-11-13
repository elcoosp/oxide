//! The native-compiler registry walkthrough (§V2-C01, D26/D40): how the
//! precompile pass chooses between a compiled-in native backend and the
//! Node tooling pool per file, what the health probe guarantees, and the
//! CAS tool-version spelling that keeps artifacts from crossing
//! implementations.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-compiler --example native_registry
//! ```

use std::path::Path;

use oxide_fw_compiler::native::{
    Backend, CompileOutput, CompileRequest, Generate, NativeCompiler, NativeRegistry,
    cas_tool_version, native_err,
};
use oxide_fw_diagnostics::OxideError;

/// A stand-in for a real C02 backend (the vendored `solidjs-compiler`
/// build): it names the framework, the extensions it handles and the
/// version the CAS key folds in. The same shape a host crate registers a
/// compiled-in backend with.
struct SolidNative {
    healthy: bool,
}

impl NativeCompiler for SolidNative {
    fn framework(&self) -> &'static str {
        "solid"
    }
    fn id(&self) -> &'static str {
        "solid-native"
    }
    fn version(&self) -> String {
        "0.1.0@vendored-rev".to_string()
    }
    fn handles(&self, ext: &str) -> bool {
        matches!(ext, "tsx" | "jsx")
    }
    fn compile(&self, req: &CompileRequest<'_>) -> Result<CompileOutput, OxideError> {
        let target = match req.generate {
            Generate::Dom => "dom",
            Generate::Ssr => "ssr",
        };
        if !self.healthy {
            return Err(oxide_compiler_err());
        }
        Ok(CompileOutput {
            js: format!("// solid {target} module for {}", req.path.display()),
            css: None,
            warnings: Vec::new(),
        })
    }
    fn health_fixture(&self) -> (&'static str, &'static str) {
        (
            "health-fixture.tsx",
            if self.healthy {
                "export default () => null;"
            } else {
                "not a valid fixture"
            },
        )
    }
}

fn oxide_compiler_err() -> OxideError {
    oxide_fw_compiler::err("CMP-NAT-001", "the backend failed its own compile")
}

/// One compile decision, the way the precompile integration makes it.
fn compile_file(reg: &NativeRegistry, backend: Backend, path: &str, source: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or("");
    match reg.pick("solid", ext, backend) {
        // The native path: compile in-process (spawn_blocking at the real
        // call site).
        Ok(Some(c)) => match c.compile(&CompileRequest {
            path: Path::new(path),
            source,
            generate: Generate::Dom,
            dev: false,
        }) {
            Ok(out) => format!("native({}): {}", c.id(), out.js),
            Err(e) => format!("native({}) failed: [{}] {}", c.id(), e.0.code, e.0.message),
        },
        // `auto`'s fallback: the Node tooling pool answers this file.
        Ok(None) => format!("node pool: {path}"),
        // `native`'s hard request refused (CMP-NAT-001).
        Err(e) => format!("refused: [{}] {} (fix: {})", e.0.code, e.0.message, {
            match &e.0.fix {
                Some(f) => f.clone(),
                None => "<none>".to_string(),
            }
        }),
    }
}

fn main() {
    // The registry as this crate ships it today: no backend is compiled
    // in (C02-C04 land the real ones), so `auto` is the Node pool and
    // `native` refuses.
    let empty = NativeRegistry::from_enabled_features();
    println!("== from_enabled_features (the default build) ==");
    println!(
        "  registered backends: {} {}",
        empty.len(),
        if empty.is_empty() { "(none yet)" } else { "" }
    );
    println!(
        "  auto:  {}",
        compile_file(&empty, Backend::Auto, "src/app/Counter.tsx", "counter")
    );
    println!(
        "  node:  {}",
        compile_file(&empty, Backend::Node, "src/app/Counter.tsx", "counter")
    );
    println!(
        "  native: {}",
        compile_file(&empty, Backend::Native, "src/app/Counter.tsx", "counter")
    );
    let refused = native_err("solid");
    println!("  the refusal: [{}] {}", refused.0.code, refused.0.message);
    println!("  its fix: {}", refused.0.fix.clone().unwrap_or_default());

    // A registered, healthy backend: `auto` picks it, the health probe
    // ran exactly once, and the CAS key names the implementation.
    let mut healthy = NativeRegistry::empty();
    healthy.register(Box::new(SolidNative { healthy: true }));
    println!("\n== a healthy solid-native backend registered ==");
    println!("  doctor rows: {:?}", {
        let rows = healthy.report();
        rows.iter()
            .map(|r| format!("{} {} healthy={}", r.id, r.version, r.healthy))
            .collect::<Vec<_>>()
    });
    println!(
        "  auto:  {}",
        compile_file(&healthy, Backend::Auto, "src/app/Counter.tsx", "counter")
    );
    if let Ok(Some(c)) = healthy.pick("solid", "tsx", Backend::Auto) {
        println!(
            "  cas tool version: {} (the D40 key dimension)",
            cas_tool_version(c)
        );
    }

    // A broken backend: `auto` silently falls back, `native` refuses
    // loudly — the honest-degradation contract.
    let mut broken = NativeRegistry::empty();
    broken.register(Box::new(SolidNative { healthy: false }));
    println!("\n== an unhealthy solid-native backend registered ==");
    println!(
        "  auto:   {}",
        compile_file(&broken, Backend::Auto, "src/app/Counter.tsx", "counter")
    );
    println!(
        "  native: {}",
        compile_file(&broken, Backend::Native, "src/app/Counter.tsx", "counter")
    );
}
