//! The migrate-planner walkthrough (§V2-E03): the committed SvelteKit
//! fixture project is planned (the dry run's data — renames, rewrites,
//! config keys, manual notes), printed in the order the `oxide migrate`
//! command will print it, then applied to a TEMPDIR COPY of the fixture
//! so the tree here stays read-only and the apply report (the mechanical
//! halves done, the config keys and notes left pending) is shown.
//!
//! The real command runs this over the user's project root behind the
//! CLI's clean-git-tree gate for `--apply`; the engine piece this
//! example drives is exactly what the command wraps.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-compiler --example migrate
//! ```

use std::fs;
use std::path::Path;

use oxide_fw_compiler::migrate::{apply_plan, detect_framework, plan_for};

/// The committed fixture project this example plans (read-only).
fn fixture(fw: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/migrate")
        .join(fw)
}

/// Copy the fixture tree into `dst` (the apply half mutates the copy).
fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("the walkthrough copy creates the destination");
    for entry in fs::read_dir(src).expect("the walkthrough copy reads the source") {
        let entry = entry.expect("the walkthrough copy entry");
        let target = dst.join(entry.file_name());
        if entry
            .file_type()
            .expect("the walkthrough copy type")
            .is_dir()
        {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("the walkthrough copy file");
        }
    }
}

fn main() {
    let fixture = fixture("sveltekit");
    let detected = detect_framework(&fixture);
    println!("=== detect: {detected:?} (package.json dependency names first)");

    // The dry run: the plan is DATA — nothing has moved yet.
    let plan = plan_for(&fixture).expect("the walkthrough plan");
    println!("=== plan: {} renames", plan.renames.len());
    for (from, to) in &plan.renames {
        println!("  rename  {}  ->  {}", from.display(), to.display());
    }
    if !plan.rewrites.is_empty() {
        println!("=== rewrites (found by the import-span scanner)");
        for r in &plan.rewrites {
            println!("  rewrite {}  {}  ->  {}", r.file.display(), r.from, r.to);
        }
    }
    if !plan.config.is_empty() {
        println!("=== config keys the apply pass writes");
        for c in &plan.config {
            println!("  {} = {}", c.key, c.value);
        }
    }
    if !plan.manual.is_empty() {
        println!("=== manual notes (never guessed)");
        for n in &plan.manual {
            println!("  {}:{}  {}", n.file.display(), n.line, n.text);
        }
    }

    // The apply half, on a copy: the mechanical work runs, the config
    // keys and the notes stay pending for the human reviewing the plan.
    let tmp = tempfile::tempdir().expect("the walkthrough tempdir");
    let sandbox = tmp.path().join("app");
    copy_tree(&fixture, &sandbox);
    let report = apply_plan(&sandbox, &plan).expect("the walkthrough apply");
    println!(
        "=== apply (on a tempdir copy): {} renamed, {} rewritten",
        report.renamed, report.rewrote
    );
    println!("  pending config keys: {:?}", report.config_pending);
    println!("  pending manual notes: {}", report.manual);
    println!(
        "=== result: {} exists, the source is gone",
        sandbox.join("src/routes/page.svelte").display()
    );
}
