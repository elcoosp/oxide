//! Criterion bench for the §V2-E03 migrate planner — the per-plan cost
//! of the discovery walks the `oxide migrate` command runs once the CLI
//! wiring lands.
//!
//! The shapes: the `SvelteKit` plan (the walk over the route tree + the
//! hooks read + the rename discovery) over a synthetic routes tree at
//! two sizes, and the apply half (the collision pre-check + the moves)
//! on a fresh fixture copy per iteration — the copy rides inside the
//! measured loop, so the number reads as an upper bound on the apply.

use std::fs;
use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_compiler::migrate::{ConfigKey, MigrationPlan, apply_plan, plan_for};

/// The fixture root the benches copy from (the committed fixture
/// project; the plan pins walk it read-only).
fn fixture(fw: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/migrate")
        .join(fw)
}

/// Copy the fixture tree into `dst` (the benches mutate the copy).
fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("the bench copy creates the destination");
    for entry in fs::read_dir(src).expect("the bench copy reads the source") {
        let entry = entry.expect("the bench copy entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("the bench copy type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("the bench copy file");
        }
    }
}

/// A synthetic `SvelteKit` tree with `routes` route files spread over
/// nested dirs (the discovery walk's shape at scale).
fn synthetic_sveltekit(root: &Path, routes: usize) {
    fs::create_dir_all(root).expect("the synthetic tree root");
    fs::write(
        root.join("package.json"),
        r#"{"devDependencies":{"@sveltejs/kit":"^2.20.0","vite":"^5.4.0"}}"#,
    )
    .expect("the synthetic package.json writes");
    for i in 0..routes {
        let dir = root
            .join("src/routes")
            .join(format!("section{}", i % 8))
            .join(format!("page{i}"));
        fs::create_dir_all(&dir).expect("the synthetic route dir");
        fs::write(dir.join("+page.svelte"), format!("<h1>{i}</h1>")).expect("the route writes");
    }
}

fn bench_plan(c: &mut Criterion) {
    let mut group = c.benchmark_group("migrate_plan");
    for routes in [8, 32] {
        let tmp = tempfile::tempdir().expect("the plan bench tempdir");
        let root = tmp.path().join("app");
        synthetic_sveltekit(&root, routes);
        group.bench_with_input(
            BenchmarkId::new("sveltekit_routes", routes),
            &root,
            |b, root| {
                b.iter(|| {
                    let plan = plan_for(root).expect("the bench plan");
                    black_box(plan.renames.len());
                });
            },
        );
    }
    group.finish();
}

fn bench_apply(c: &mut Criterion) {
    let mut group = c.benchmark_group("migrate_apply");
    // The fixture plan's rename set is small and fixed; the apply is
    // the collision pre-check plus the moves, on a fresh copy each
    // iteration (the copy cost rides the measurement — the honest
    // upper bound for `oxide migrate --apply` on a tree this size).
    group.bench_function("sveltekit_fixture", |b| {
        b.iter(|| {
            let tmp = tempfile::tempdir().expect("the apply bench tempdir");
            let root = tmp.path().join("app");
            copy_tree(&fixture("sveltekit"), &root);
            let plan = plan_for(&root).expect("the apply bench plan");
            let report = apply_plan(&root, &plan).expect("the apply bench");
            black_box(report.renamed);
        });
    });
    group.finish();
}

/// Seed a 24-path `tsconfig.json` (the merge bench's document shape).
fn seed_tsconfig(root: &Path) {
    let mut paths = serde_json::Map::new();
    for i in 0..24 {
        paths.insert(
            format!("$alias{i}"),
            serde_json::json!([format!("./src/lib{i}")]),
        );
    }
    fs::write(
        root.join("tsconfig.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "compilerOptions": { "target": "esnext", "paths": paths }
        }))
        .expect("the bench tsconfig serializes"),
    )
    .expect("the bench tsconfig writes");
}

/// Seed a 24-row `oxide.config.ts` (the insertion bench's document
/// shape).
fn seed_config(root: &Path) {
    use std::fmt::Write as _;
    let mut rows = String::new();
    for i in 0..24 {
        let _ = writeln!(rows, "  key{i}: 'value{i}',");
    }
    fs::write(
        root.join("oxide.config.ts"),
        format!(
            "import {{ defineConfig }} from '@oxide-fw/core'\n\n\
             export default defineConfig({{\n{rows}}})\n"
        ),
    )
    .expect("the bench config writes");
}

/// The E03 apply-in-engine half: the config-key surgery cost — the
/// tsconfig merge on a seeded 24-path document and the oxide.config.ts
/// insertion over an existing 24-row `defineConfig` object, on a fresh
/// copy per iteration (the same upper-bound convention as the apply
/// group).
fn bench_config_apply(c: &mut Criterion) {
    let mut group = c.benchmark_group("migrate_config_apply");

    group.bench_function("tsconfig_paths_merge_24", |b| {
        b.iter(|| {
            let tmp = tempfile::tempdir().expect("the tsconfig bench tempdir");
            let root = tmp.path().join("app");
            fs::create_dir_all(&root).expect("the bench root");
            seed_tsconfig(&root);
            let plan = MigrationPlan {
                config: vec![ConfigKey {
                    key: "tsconfig.paths.$lib".to_string(),
                    value: "[\"./src/lib\"]".to_string(),
                }],
                ..MigrationPlan::default()
            };
            let report = apply_plan(&root, &plan).expect("the tsconfig bench apply");
            black_box(report.config_applied.len());
        });
    });

    group.bench_function("oxide_config_insert_24_rows", |b| {
        b.iter(|| {
            let tmp = tempfile::tempdir().expect("the config bench tempdir");
            let root = tmp.path().join("app");
            fs::create_dir_all(&root).expect("the bench root");
            seed_config(&root);
            let plan = MigrationPlan {
                config: vec![ConfigKey {
                    key: "routes.dir".to_string(),
                    value: "\"src/routes\"".to_string(),
                }],
                ..MigrationPlan::default()
            };
            let report = apply_plan(&root, &plan).expect("the config bench apply");
            black_box(report.config_applied.len());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_plan, bench_apply, bench_config_apply);
criterion_main!(benches);
