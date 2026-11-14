//! V2-C11 walkthrough: what `oxide check` and `oxide fmt` will shell out
//! to for a project — the tool inventory over `resolve_tool` (the
//! project-local `node_modules/.bin` install wins over `PATH`), the
//! configured typecheck backend (`check.typecheck`) and the formatter
//! fallback chain (`oxfmt` → `biome format` → the loud SKIPPED line).
//!
//! Run from the repo root against any project directory:
//!
//! ```text
//! cargo run -p oxide-fw-cli --example check_backends -- examples/smoke-app-react
//! ```

use oxide_fw_cli::{resolve_tool, typecheck_backend_plan};

fn main() -> anyhow::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let root = std::fs::canonicalize(&root)?;
    let cfg = match oxide_fw_config::load(&root, "development", std::collections::BTreeMap::new()) {
        Ok(loaded) => loaded.config,
        Err(e) => {
            println!(
                "config load failed ({} {}) — defaults apply",
                e.0.code, e.0.message
            );
            oxide_fw_config::OxideConfig::default()
        }
    };

    println!("project: {}", root.display());
    println!(
        "check.typecheck = {:?} (the backend `oxide check` runs)",
        cfg.check.typecheck
    );

    let backends = [
        oxide_fw_config::TypecheckBackend::Tsgo,
        oxide_fw_config::TypecheckBackend::Oxlint,
        oxide_fw_config::TypecheckBackend::Tsrs,
        oxide_fw_config::TypecheckBackend::Tsc,
    ];
    println!("\n— typecheck backends (resolve_tool: node_modules/.bin → PATH) —");
    for backend in backends {
        match typecheck_backend_plan(backend) {
            Some((bin, args)) => match resolve_tool(bin, &root) {
                Some(p) => println!(
                    "  {:?}: {} → `{bin} {}`",
                    backend,
                    p.display(),
                    args.join(" ")
                ),
                None => println!(
                    "  {:?}: `{bin}` not found — `oxide check` prints the loud SKIPPED line",
                    backend
                ),
            },
            None => println!("  {:?}: the arm is off — no typechecker runs", backend),
        }
    }

    println!("\n— formatter chain (`oxide fmt`) —");
    for name in ["oxfmt", "biome"] {
        match resolve_tool(name, &root) {
            Some(p) => println!("  {name}: {} (in the chain)", p.display()),
            None => println!("  {name}: not found"),
        }
    }
    let oxfmt = resolve_tool("oxfmt", &root);
    let biome = resolve_tool("biome", &root);
    if oxfmt.is_none() && biome.is_none() {
        println!("  → `oxide fmt` prints SKIPPED (fails under OXIDE_CHECK_STRICT=1)");
    } else if oxfmt.is_some() {
        println!("  → `oxide fmt` runs oxfmt");
    } else {
        println!("  → `oxide fmt` falls back to `biome format`");
    }

    Ok(())
}
