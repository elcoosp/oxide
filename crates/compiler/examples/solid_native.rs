//! The native Solid JSX compiler walkthrough (§V2-C02, re-opened): how a
//! Solid file compiles in-process to the `@solidjs/web` runtime on both
//! targets, what the emitted module looks like, the D40 CAS version the
//! precompile cache folds, and the loud refusal the v1 scope carries for
//! constructs the Node tooling pool still compiles.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-compiler --features native-solid --example solid_native
//! ```

use oxide_fw_compiler::native::{
    Backend, CompileRequest, Generate, NativeCompiler, NativeRegistry, cas_tool_version,
};

/// The spec's spike fixture (a signal, an event handler, a `<Show>` — the
/// committed `tests/fixtures/native/solid/Counter.tsx`).
const COUNTER: &str = include_str!("../tests/fixtures/native/solid/Counter.tsx");

fn main() {
    let backend = oxide_fw_compiler::solid::SolidNative;
    println!("=== the backend (the seam metadata) ===");
    println!(
        "framework={} id={} version={}",
        backend.framework(),
        backend.id(),
        backend.version()
    );
    println!(
        "handles tsx={} jsx={} — the CAS key spells {}",
        backend.handles("tsx"),
        backend.handles("jsx"),
        cas_tool_version(&backend)
    );

    let path = std::path::Path::new("src/components/Counter.tsx");
    println!("\n=== DOM target (`generate: 'dom'`) ===");
    let dom = backend
        .compile(&CompileRequest {
            path,
            source: COUNTER,
            generate: Generate::Dom,
            dev: false,
        })
        .expect("the fixture compiles");
    println!("{}", dom.js);

    println!("=== SSR target (`generate: 'ssr'`) ===");
    let ssr = backend
        .compile(&CompileRequest {
            path,
            source: COUNTER,
            generate: Generate::Ssr,
            dev: false,
        })
        .expect("the fixture compiles");
    println!("{}", ssr.js);

    println!("=== the selection seam (`NativeRegistry`) ===");
    let mut reg = NativeRegistry::empty();
    reg.register(Box::new(oxide_fw_compiler::solid::SolidNative));
    let picked = reg
        .pick("solid", "tsx", Backend::Auto)
        .expect("`auto` consults the health fixture")
        .expect("a healthy compiled-in backend picks");
    println!(
        "pick(solid, tsx, auto) → {} @ {} (the health fixture `{}` compiled once, cached)",
        picked.id(),
        picked.version(),
        picked.health_fixture().0
    );
    for row in reg.report() {
        println!(
            "doctor row: native compiler {} {} ok={}",
            row.id, row.version, row.healthy
        );
    }

    println!("\n=== the v1 refusal (loud, with the Node-pool fix) ===");
    let spread = "export const A = (props) => <div {...props} />;";
    match backend.compile(&CompileRequest {
        path: std::path::Path::new("src/A.tsx"),
        source: spread,
        generate: Generate::Dom,
        dev: false,
    }) {
        Ok(_) => unreachable!("spreads refuse"),
        Err(e) => {
            println!("{}: {}", e.0.code, e.0.message);
            println!("fix: {}", e.0.fix.as_deref().unwrap_or("-"));
        }
    }
    println!(
        "\nEverything the v1 scope refuses compiles on the Node tooling pool —\n`framework.compilerBackend: 'auto'` falls back, `'native'` surfaces the refusal."
    );
}
