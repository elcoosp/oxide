//! TDD pins for the §V2-E03 `oxide migrate` codemod PLANNER — the
//! plan-is-data substrate the `oxide migrate [--apply]` command drives
//! once the CLI wiring lands (the V8-free half of E03: framework
//! detection, the per-source-framework planners over the four spellings
//! the spec names, and the apply half the `--apply` flag runs behind
//! the CLI's clean-tree gate).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - [`detect_framework`] reads the signature surface (package.json
//!   dependency names first, then the framework config files) and never
//!   guesses: a tree it cannot attribute returns `None`.
//! - [`plan_for`] is the one entry: detect → the framework's planner →
//!   a [`MigrationPlan`] whose four fields are exactly the spec's
//!   `renames` / `rewrites` / `config` / `manual` data. The plan is
//!   deterministic: two runs over the same tree serialize byte-identical
//!   (sorted everywhere, no map iteration order).
//! - The **SvelteKit** planner is the pure path mapping the spec pins:
//!   every `+`-prefixed route basename loses its marker (`+page.svelte`
//!   → `page.svelte`, `+server.ts` → `route.ts`), a plain
//!   `hooks.server.ts` becomes the `middleware.ts` rename, a `sequence`
//!   one flags manual instead, and the `$lib` alias lands as the
//!   tsconfig path entry the spec names.
//! - The **Next.js (app router)** planner enables the compat conventions
//!   (`routes.compat = 'next'`), finds the `next/link` / `next/image`
//!   import rewrites through the compiler's own
//!   [`scan_import_spans`](oxide_fw_compiler::scan_import_spans) (never
//!   regex), reports `'use client'` files as island candidates without
//!   guessing, and carries the next.config table mappings and the pages
//!   -router refusal as located notes.
//! - The **Nuxt** planner maps `pages/` to the routes root with the
//!   bracket conventions carried verbatim, demotes a parent route
//!   component with a same-named directory to a layout, and flags
//!   `useFetch` / `useAsyncData` manual with the suggested `load`.
//! - The **Vite SPA** planner wraps the existing entry: the spa route
//!   note plus the explicit routes dir, nothing renamed.
//! - [`apply_plan`] performs the plan's mechanical halves — renames
//!   (create-dir + rename, refusing an existing target with
//!   `MIG-TARGET`) and import rewrites (span surgery over the scanned
//!   spans) — and reports the config keys and notes as the work the
//!   `--apply` CLI pass (or a human) finishes.

use std::fs;
use std::path::{Path, PathBuf};

use oxide_fw_compiler::migrate::{
    ApplyReport, ConfigKey, Framework, ImportRewrite, MIG_DETECT, MIG_TARGET, MigrationPlan,
    apply_plan, detect_framework, plan_for,
};

// --------------------------------------------------------------- fixtures

/// The committed fixture project for one framework (read-only; apply
/// pins copy it into a tempdir first).
fn fixture_root(fw: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/migrate")
        .join(fw)
}

/// Copy a fixture tree into a fresh tempdir (the apply pins mutate).
fn copy_fixture(fw: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("the apply fixture tempdir");
    let dst = tmp.path().join("app");
    copy_tree(&fixture_root(fw), &dst);
    (tmp, dst)
}

/// Recursive copy of the fixture's files (dirs + files only; the
/// fixtures carry no symlinks).
fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("the fixture copy creates the destination dir");
    for entry in fs::read_dir(src).expect("the fixture copy reads the source dir") {
        let entry = entry.expect("the fixture copy entry");
        let ty = entry.file_type().expect("the fixture copy entry type");
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("the fixture copy file");
        }
    }
}

/// The golden plan JSON for one framework fixture (the committed
/// `plan.json` the fixture dir carries).
fn golden_plan(fw: &str) -> String {
    fs::read_to_string(fixture_root(fw).join("plan.json")).expect("the golden plan fixture")
}

fn serialize(plan: &MigrationPlan) -> String {
    serde_json::to_string_pretty(plan).expect("the plan serializes")
}

/// The golden-regen path (run explicitly:
/// `cargo test -p oxide-fw-compiler --test migrate_tdd -- --ignored
/// regen_goldens`): replans every fixture and rewrites its committed
/// `plan.json`. The goldens are reviewed artifacts — after a regen,
/// diff them before committing.
#[test]
#[ignore]
fn regen_goldens() {
    for fw in ["sveltekit", "next", "nuxt", "vite"] {
        let plan = plan_for(&fixture_root(fw)).expect("the regen plan");
        fs::write(
            fixture_root(fw).join("plan.json"),
            format!("{}\n", serialize(&plan)),
        )
        .expect("the regen writes the golden");
    }
}

// ------------------------------------------------------- detection table

/// The spec's signature surface: package.json dependency names first,
/// the framework config files as the no-manifest fallback.
#[test]
fn detect_framework_table() {
    let cases: [(&str, Option<Framework>); 4] = [
        ("sveltekit", Some(Framework::SvelteKit)),
        ("next", Some(Framework::Next)),
        ("nuxt", Some(Framework::Nuxt)),
        ("vite", Some(Framework::Vite)),
    ];
    for (fw, expected) in cases {
        assert_eq!(detect_framework(&fixture_root(fw)), expected, "fw={fw}");
    }
    // An empty tree detects nothing.
    let tmp = tempfile::tempdir().expect("the empty tree tempdir");
    assert_eq!(detect_framework(tmp.path()), None);
}

/// Package.json names outrank the config-file signatures: a Next app
/// that also carries a vite dev dependency still detects Next.
#[test]
fn detect_framework_prefers_the_framework_package_over_vite() {
    let tmp = tempfile::tempdir().expect("the mixed-tree tempdir");
    fs::write(
        tmp.path().join("package.json"),
        r#"{"devDependencies":{"vite":"^5.0.0"},"dependencies":{"next":"^15.0.0"}}"#,
    )
    .expect("the mixed package.json writes");
    assert_eq!(detect_framework(tmp.path()), Some(Framework::Next));
}

/// The config-file fallback: a tree whose manifest omits the framework
/// dep but whose signature config carries it still detects.
#[test]
fn detect_framework_falls_back_to_the_signature_configs() {
    let tmp = tempfile::tempdir().expect("the signature tempdir");
    fs::write(
        tmp.path().join("package.json"),
        r#"{"dependencies":{"react":"^19.0.0"}}"#,
    )
    .expect("the package.json writes");
    fs::write(tmp.path().join("next.config.js"), "module.exports = {}").expect("the next config");
    assert_eq!(detect_framework(tmp.path()), Some(Framework::Next));
}

// ------------------------------------------------------------ plan shape

/// Every planner's plan round-trips the spec's four-field data model.
#[test]
fn plan_is_the_spec_four_field_model() {
    let plan = plan_for(&fixture_root("vite")).expect("the vite plan");
    assert!(plan.renames.is_empty(), "the vite wrap renames nothing");
    assert!(plan.rewrites.is_empty(), "the vite wrap rewrites nothing");
    assert!(!plan.config.is_empty(), "the vite wrap records config");
    assert!(!plan.manual.is_empty(), "the vite wrap notes the entry");
    let ConfigKey { key, value } = &plan.config[0];
    assert_eq!(key, "routes.dir");
    assert_eq!(value, "\"src/routes\"");
}

/// `plan_for` on an unattributable tree refuses with `MIG-DETECT`
/// naming the root (in the message) and the supported spellings (in
/// the fix).
#[test]
fn plan_for_unknown_root_refuses_detect() {
    let tmp = tempfile::tempdir().expect("the unknown tree tempdir");
    let err = plan_for(tmp.path()).expect_err("an empty tree has no plan");
    assert_eq!(err.0.code, MIG_DETECT);
    assert!(
        err.0
            .message
            .contains(tmp.path().to_string_lossy().as_ref()),
        "the refusal names the root: {}",
        err.0.message
    );
    let fix = err.0.fix.as_deref().unwrap_or_default();
    for spelling in ["sveltekit", "next", "nuxt", "vite"] {
        assert!(fix.contains(spelling), "the fix names `{spelling}`: {fix}");
    }
}

/// Determinism: two runs over the same fixture serialize byte-identical
/// (sorted walks, no map iteration order leaks into the plan).
#[test]
fn plans_are_deterministic() {
    for fw in ["sveltekit", "next", "nuxt"] {
        let first = serialize(&plan_for(&fixture_root(fw)).expect("the first plan"));
        let second = serialize(&plan_for(&fixture_root(fw)).expect("the second plan"));
        assert_eq!(first, second, "fw={fw} replans byte-identical");
    }
}

// ------------------------------------------------------ SvelteKit pins

/// The pure path mapping: every `+`-prefixed basename loses its marker,
/// endpoints become `route.ts`, the plain `hooks.server.ts` becomes the
/// middleware rename, and the plan stays inside `src/routes`.
#[test]
fn sveltekit_renames_strip_the_route_markers() {
    let plan = plan_for(&fixture_root("sveltekit")).expect("the sveltekit plan");
    let want: &[(&str, &str)] = &[
        ("src/hooks.server.ts", "src/middleware.ts"),
        ("src/routes/+layout.svelte", "src/routes/layout.svelte"),
        ("src/routes/+page.svelte", "src/routes/page.svelte"),
        (
            "src/routes/about/+page.server.ts",
            "src/routes/about/page.server.ts",
        ),
        (
            "src/routes/about/+page.svelte",
            "src/routes/about/page.svelte",
        ),
        ("src/routes/api/+server.ts", "src/routes/api/route.ts"),
        (
            "src/routes/blog/[slug]/+page.svelte",
            "src/routes/blog/[slug]/page.svelte",
        ),
    ];
    let got: Vec<(String, String)> = plan
        .renames
        .iter()
        .map(|(from, to)| {
            (
                from.to_string_lossy().into_owned(),
                to.to_string_lossy().into_owned(),
            )
        })
        .collect();
    assert_eq!(got.len(), want.len(), "the rename set: {got:?}");
    for (i, (from, to)) in want.iter().enumerate() {
        assert_eq!(got[i].0, *from, "rename {i} source");
        assert_eq!(got[i].1, *to, "rename {i} target");
    }
}

/// A plain `hooks.server.ts` (exports `handle`, no `sequence`) is the
/// middleware rename the spec pins.
#[test]
fn sveltekit_hooks_plain_becomes_the_middleware_rename() {
    let plan = plan_for(&fixture_root("sveltekit")).expect("the sveltekit plan");
    assert!(
        plan.renames.contains(&(
            PathBuf::from("src/hooks.server.ts"),
            PathBuf::from("src/middleware.ts")
        )),
        "hooks.server.ts renames to middleware.ts: {:?}",
        plan.renames
    );
    assert!(
        !plan
            .manual
            .iter()
            .any(|n| n.file == PathBuf::from("src/hooks.server.ts")),
        "the plain hooks file carries no manual note"
    );
}

/// A `sequence`-composed hooks file flags manual instead of renaming
/// (the spec's one SvelteKit manual note).
#[test]
fn sveltekit_hooks_sequence_flags_manual() {
    let (tmp, root) = copy_fixture("sveltekit");
    fs::write(
        root.join("src/hooks.server.ts"),
        "import { sequence } from '@sveltejs/kit/hooks';\nexport const handle = sequence(a, b);\n",
    )
    .expect("the sequence hooks file writes");
    let plan = plan_for(&root).expect("the sequence plan");
    let note = plan
        .manual
        .iter()
        .find(|n| n.file == PathBuf::from("src/hooks.server.ts"))
        .expect("the sequence hooks note");
    assert_eq!(note.line, 1, "the note names the import line");
    assert!(
        note.text.contains("sequence"),
        "the note names the construct: {}",
        note.text
    );
    assert!(
        !plan.renames.contains(&(
            PathBuf::from("src/hooks.server.ts"),
            PathBuf::from("src/middleware.ts")
        )),
        "a sequence hooks file never renames"
    );
    drop(tmp);
}

/// The `$lib` alias lands as the tsconfig path entry the spec names.
#[test]
fn sveltekit_lib_alias_lands_as_a_tsconfig_path() {
    let plan = plan_for(&fixture_root("sveltekit")).expect("the sveltekit plan");
    let lib = plan
        .config
        .iter()
        .find(|c| c.key == "tsconfig.paths.$lib")
        .expect("the $lib tsconfig path entry");
    assert_eq!(lib.value, "[\"./src/lib\"]");
}

/// The committed golden plan (the fixture's `plan.json`) matches.
#[test]
fn sveltekit_plan_golden() {
    let plan = plan_for(&fixture_root("sveltekit")).expect("the sveltekit plan");
    assert_eq!(serialize(&plan) + "\n", golden_plan("sveltekit"));
}

// ----------------------------------------------------------- Next pins

/// The compat conventions key plus the `next/link` / `next/image`
/// rewrites the import-span scanner finds (never regex).
#[test]
fn next_plan_rewrites_and_compat_key() {
    let plan = plan_for(&fixture_root("next")).expect("the next plan");
    assert!(
        plan.config
            .iter()
            .any(|c| c.key == "routes.compat" && c.value == "\"next\""),
        "routes.compat = 'next' is planned: {:?}",
        plan.config
    );
    let find = |from: &str| {
        plan.rewrites
            .iter()
            .find(|r| r.from == from)
            .unwrap_or_else(|| panic!("the {from} rewrite is planned"))
    };
    let link = find("next/link");
    assert_eq!(link.to, "@oxide/react");
    assert_eq!(link.file, PathBuf::from("app/blog/[slug]/page.tsx"));
    let image = find("next/image");
    assert_eq!(image.to, "@oxide/react");
    assert_eq!(image.file, PathBuf::from("app/page.tsx"));
}

/// `'use client'` files are island candidates — reported, never guessed
/// into an island config.
#[test]
fn next_use_client_reports_island_candidates() {
    let plan = plan_for(&fixture_root("next")).expect("the next plan");
    let note = plan
        .manual
        .iter()
        .find(|n| n.file == PathBuf::from("app/page.tsx"))
        .expect("the use-client note");
    assert!(note.text.contains("island candidate"), "{}", note.text);
}

/// The next.config table mappings and the pages-router refusal are
/// located notes (file + line), not guessed values.
#[test]
fn next_config_and_pages_router_notes() {
    let plan = plan_for(&fixture_root("next")).expect("the next plan");
    let images = plan
        .manual
        .iter()
        .find(|n| {
            n.file == PathBuf::from("next.config.js") && n.text.contains("images.remotePatterns")
        })
        .expect("the images.remotePatterns mapping note");
    assert_eq!(images.line, 2, "the note names the key's line");
    let pages = plan
        .manual
        .iter()
        .find(|n| n.file == PathBuf::from("pages"))
        .expect("the pages-router note");
    assert!(pages.text.contains("not migrated"), "{}", pages.text);
}

/// The committed golden plan.
#[test]
fn next_plan_golden() {
    let plan = plan_for(&fixture_root("next")).expect("the next plan");
    assert_eq!(serialize(&plan) + "\n", golden_plan("next"));
}

// ----------------------------------------------------------- Nuxt pins

/// `pages/` maps into the routes root: `index.vue` becomes the dir's
/// `page.vue`, bracket dirs carry over verbatim, and a parent route
/// component with a same-named directory demotes to the layout.
#[test]
fn nuxt_renames_map_pages_into_the_routes_root() {
    let plan = plan_for(&fixture_root("nuxt")).expect("the nuxt plan");
    let want: &[(&str, &str)] = &[
        ("error.vue", "src/routes/error.vue"),
        ("layouts/default.vue", "src/routes/layout.vue"),
        ("pages/about.vue", "src/routes/about/page.vue"),
        ("pages/index.vue", "src/routes/page.vue"),
        ("pages/users/[id].vue", "src/routes/users/[id]/page.vue"),
        ("pages/users.vue", "src/routes/users/layout.vue"),
    ];
    let got: Vec<(String, String)> = plan
        .renames
        .iter()
        .map(|(from, to)| {
            (
                from.to_string_lossy().into_owned(),
                to.to_string_lossy().into_owned(),
            )
        })
        .collect();
    assert_eq!(got.len(), want.len(), "the rename set: {got:?}");
    for (i, (from, to)) in want.iter().enumerate() {
        assert_eq!(got[i].0, *from, "rename {i} source");
        assert_eq!(got[i].1, *to, "rename {i} target");
    }
}

/// `useFetch` / `useAsyncData` flag manual with the suggested `load`.
#[test]
fn nuxt_data_fetching_flags_manual() {
    let plan = plan_for(&fixture_root("nuxt")).expect("the nuxt plan");
    let note = plan
        .manual
        .iter()
        .find(|n| n.file == PathBuf::from("pages/index.vue"))
        .expect("the useFetch note");
    assert!(note.text.contains("load"), "{}", note.text);
    assert!(
        plan.manual
            .iter()
            .any(|n| n.file == PathBuf::from("pages/users/[id].vue")),
        "the useAsyncData file carries a note too"
    );
}

/// The committed golden plan.
#[test]
fn nuxt_plan_golden() {
    let plan = plan_for(&fixture_root("nuxt")).expect("the nuxt plan");
    assert_eq!(serialize(&plan) + "\n", golden_plan("nuxt"));
}

// ------------------------------------------------------------ Vite pins

/// The committed golden plan.
#[test]
fn vite_plan_golden() {
    let plan = plan_for(&fixture_root("vite")).expect("the vite plan");
    assert_eq!(serialize(&plan) + "\n", golden_plan("vite"));
}

// ---------------------------------------------------------- apply pins

/// Apply performs the SvelteKit renames for real: the sources are gone,
/// the targets carry the bytes, and the report counts the work while
/// leaving the config keys and notes pending.
#[test]
fn apply_sveltekit_moves_the_files_and_reports() {
    let (tmp, root) = copy_fixture("sveltekit");
    let plan = plan_for(&root).expect("the sveltekit plan");
    let report = apply_plan(&root, &plan).expect("the apply");
    let ApplyReport {
        renamed,
        rewrote,
        config_pending,
        manual,
    } = report;
    assert_eq!(renamed, plan.renames.len(), "every rename applied");
    assert_eq!(rewrote, plan.rewrites.len());
    assert_eq!(
        config_pending.len(),
        plan.config.len(),
        "config keys stay pending"
    );
    assert_eq!(manual, plan.manual.len());
    assert!(
        !root.join("src/routes/+page.svelte").exists(),
        "the source is gone"
    );
    assert!(
        root.join("src/routes/page.svelte").exists(),
        "the target exists"
    );
    assert!(
        root.join("src/routes/api/route.ts").exists(),
        "the endpoint target exists"
    );
    assert!(
        root.join("src/middleware.ts").exists(),
        "the middleware target exists"
    );
    drop(tmp);
}

/// Apply rewrites import specifiers by span surgery: the planned `from`
/// no longer appears as an import in the rewritten file and the `to`
/// does, byte-precise (nothing else on the line moves).
#[test]
fn apply_rewrites_import_specifiers_by_span() {
    let (tmp, root) = copy_fixture("next");
    let plan = plan_for(&root).expect("the next plan");
    apply_plan(&root, &plan).expect("the apply");
    let page =
        fs::read_to_string(root.join("app/blog/[slug]/page.tsx")).expect("the rewritten page");
    assert!(
        !page.contains("from 'next/link'"),
        "the old specifier is gone: {page}"
    );
    assert!(
        page.contains("from '@oxide/react'"),
        "the new specifier is in: {page}"
    );
    // Span surgery keeps the rest of the file byte-identical.
    let original = fs::read_to_string(fixture_root("next").join("app/blog/[slug]/page.tsx"))
        .expect("the original page");
    let original_line: Vec<&str> = original
        .lines()
        .filter(|l| l.contains("import") && !l.contains("next/link"))
        .collect();
    for line in original_line {
        assert!(page.contains(line), "the untouched line survives: {line}");
    }
    drop(tmp);
}

/// Apply refuses a rename whose target already exists (`MIG-TARGET`)
/// and leaves the tree untouched (the first pinned rename runs last).
#[test]
fn apply_refuses_an_existing_target() {
    let (tmp, root) = copy_fixture("sveltekit");
    // The collision: the first rename target exists before the apply.
    fs::create_dir_all(root.join("src/routes")).expect("the routes dir");
    fs::write(root.join("src/routes/page.svelte"), "<h1>existing</h1>")
        .expect("the colliding target writes");
    let plan = plan_for(&root).expect("the sveltekit plan");
    let err = apply_plan(&root, &plan).expect_err("a colliding rename refuses");
    assert_eq!(err.0.code, MIG_TARGET, "{}", err.0.message);
    assert!(
        err.0.message.contains("already exists"),
        "the refusal names the collision: {}",
        err.0.message
    );
    // The source was never moved (the collision is discovered before
    // any rename runs).
    assert!(
        root.join("src/routes/+page.svelte").exists(),
        "the source stays put"
    );
    drop(tmp);
}

/// Apply is a no-op for a plan with no mechanical halves (the Vite
/// wrap): the report carries the pending config keys and notes only.
#[test]
fn apply_vite_wrap_reports_pending_only() {
    let (tmp, root) = copy_fixture("vite");
    let plan = plan_for(&root).expect("the vite plan");
    let report = apply_plan(&root, &plan).expect("the apply");
    assert_eq!(report.renamed, 0);
    assert_eq!(report.rewrote, 0);
    assert_eq!(report.config_pending.len(), plan.config.len());
    assert_eq!(report.manual, plan.manual.len());
    drop(tmp);
}

/// The plan's import rewrites round-trip through serde (the golden
/// JSON's wire shape) — pinned on one constructed value so the
/// camelCase field spellings stay stable for the CLI's JSON surface.
#[test]
fn import_rewrite_serializes_camel_case() {
    let rewrite = ImportRewrite {
        file: PathBuf::from("app/page.tsx"),
        from: "next/image".into(),
        to: "@oxide/react".into(),
    };
    let json = serde_json::to_value(&rewrite).expect("the rewrite serializes");
    assert_eq!(json["file"], "app/page.tsx");
    assert_eq!(json["from"], "next/image");
    assert_eq!(json["to"], "@oxide/react");
}
