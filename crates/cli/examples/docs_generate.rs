//! Consumer example for the V2-E11 generated-reference seam: the registry
//! of reference-page generators a contributor (or an agent) drives when
//! the docs need to catch up with the code.
//!
//! Shows the three pieces the command composes:
//!
//! 1. [`oxide_fw_cli::docs_gen::generators`] — the registry: one
//!    [`ReferenceGenerator`](oxide_fw_cli::docs_gen::ReferenceGenerator)
//!    per page under `docs/reference/`, each pulling from the single
//!    source of truth the crates already ship (the §18 matrix, the config
//!    schema, the clap tree, `docs/errors.toml`).
//! 2. [`oxide_fw_cli::docs_gen::generate_all`] — render every page in
//!    memory; `DOC-001` fires here when a config key carries no schema
//!    description (the E11 acceptance), so the failure is the work list.
//! 3. The deterministic-bytes contract `--check` (and the CI
//!    `docs-check` job) compares against the committed pages.
//!
//! The real command writes the pages:
//!
//! ```text
//! cargo run -p oxide-fw-cli -- docs generate --root .
//! cargo run -p oxide-fw-cli -- docs generate --root . --check
//! ```
//!
//! This example renders in memory and prints what each page carries —
//! no filesystem writes.

use oxide_fw_cli::docs_gen::{generate_all, generators};

fn main() {
    println!(
        "the docs_gen registry ({} pages, write order):",
        generators().len()
    );
    for g in generators() {
        println!("  docs/reference/{} —", g.path());
    }

    match generate_all() {
        Ok(pages) => {
            println!("\nrendered (deterministic bytes; `--check` compares these):");
            for page in pages {
                let lines = page.content.lines().count();
                println!(
                    "  {} — {lines} lines, {} bytes",
                    page.path,
                    page.content.len()
                );
            }
            println!("\nwrite them with: cargo run -p oxide-fw-cli -- docs generate --root .");
        }
        Err(e) => {
            // The DOC-001 work list: every config key that carries no
            // schema description, named by the diagnostic.
            println!("\nrender refused: {e}");
        }
    }
}
