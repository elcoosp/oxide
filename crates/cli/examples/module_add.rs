//! §V2-E06 consumer example: a V8-free end-to-end walkthrough of the
//! Module API v1 — the config edit (`oxide add`'s AST surgery on
//! `oxide.config.ts`) and the manifest resolution (the loader's
//! `resolve_one` → `to_manifest`).
//!
//! Runs against the fixture app under `tests/fixtures/analytics-app/`
//! (a minimal project with an `oxide.config.ts` and the `@oxide/analytics`
//! module symlinked into `node_modules/`). No V8, no pnpm, no build step —
//! the example loads the fixture, performs the AST edit, resolves the
//! module, and prints the manifest.
//!
//! ```text
//! cargo run -p oxide-fw-cli --no-default-features --example module_add -- tests/fixtures/analytics-app @oxide/analytics
//! ```

use std::path::PathBuf;

use oxide_fw_cli::module_config_edit::{add_module_to_config, default_alias_for};
use oxide_fw_cli::modules_loader::{enforce_adapters, merge_types, resolve_one, to_manifest};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "tests/fixtures/analytics-app".to_string());
    let module = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "@oxide/analytics".to_string());
    let rootp = PathBuf::from(&root);
    let config_ts = rootp.join("oxide.config.ts");

    println!("== §V2-E06 module_add example ==");
    println!("root:   {}", rootp.display());
    println!("module: {module}");

    // 1. The config edit (the `oxide add` AST surgery).
    println!("\n[1] config edit (idempotent, comment-preserving):");
    let src = std::fs::read_to_string(&config_ts)
        .unwrap_or_else(|e| panic!("read {}: {e}", config_ts.display()));
    let alias = default_alias_for(&module);
    let edit = add_module_to_config(&src, &module, Some(alias)).expect("config edit");
    println!(
        "  import_added={}, module_added={}",
        edit.import_added, edit.module_added
    );
    // Write the edited config back so a second run demonstrates idempotency.
    if edit.import_added || edit.module_added {
        std::fs::write(&config_ts, &edit.bytes).expect("write config");
        println!("  wrote {}", config_ts.display());
    } else {
        println!("  (no-op; already present)");
    }

    // 2. The manifest resolution (the loader's `resolve_one` → `to_manifest`).
    println!("\n[2] manifest resolution:");
    let pkg_dir = rootp.join("node_modules").join(&module);
    let resolved = resolve_one(&pkg_dir, module.clone()).expect("module resolves");
    println!("  name:        {}", resolved.name);
    println!("  adapters:    [{}]", resolved.adapters.join(", "));
    println!(
        "  routes_dir:  {}",
        resolved
            .routes_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    println!("  middlewares: {}", resolved.middlewares.len());
    println!("  serverFns:   {}", resolved.server_fns.len());
    println!("  types:       {}", resolved.types.len());
    println!("  hooks:       [{}]", resolved.hooks.join(", "));

    // 3. The unified manifest (the plugin-registry contract).
    println!("\n[3] unified ModuleManifest (the plugin-registry contract):");
    let manifest = to_manifest(&resolved);
    println!("  name:    {}", manifest.name);
    println!("  hooks:   [{}]", manifest.hooks.join(", "));
    println!(
        "  config_defaults keys: [{}]",
        manifest
            .config_defaults
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );

    // 4. The adapters enforcement (loud refusal on a mismatch).
    println!("\n[4] adapters enforcement (project adapter = react):");
    let project_adapter = "react";
    match enforce_adapters(std::slice::from_ref(&resolved), project_adapter) {
        Ok(()) => println!("  ok: {project_adapter} is supported"),
        Err(e) => {
            println!("  {} {}", e.0.code, e.0.message);
            if let Some(fix) = &e.0.fix {
                println!("  fix: {fix}");
            }
        }
    }

    // 5. The typegen merge (writes `.oxide/types/<name>.d.ts`).
    println!("\n[5] typegen merge:");
    let written = merge_types(&rootp, std::slice::from_ref(&resolved));
    println!("  wrote {written} type declaration(s) to .oxide/types/");
    if written > 0 {
        let types_dir = rootp.join(".oxide/types");
        if let Ok(entries) = std::fs::read_dir(&types_dir) {
            for e in entries.flatten() {
                println!("    {}", e.path().display());
            }
        }
    }

    println!("\n== end ==");
}
