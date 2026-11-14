//! V2-C11: the tool-resolution bench (`check_tool_resolve`) — the
//! `resolve_tool` lookup `oxide check` pays once per external tool arm
//! (project-local `node_modules/.bin` first, then the PATH walk) and
//! `oxide doctor` pays six times.
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! inspect` (the bench-util opt-in keeps plain `cargo test` off criterion).

use criterion::{Criterion, black_box, criterion_group, criterion_main};

/// A PATH-shaped fixture: a few plausible bin dirs (the benches run with
/// the developer's real PATH inherited, so the numbers move with the
/// machine — the baseline records the median, not a unit).
fn bench_check_tool_resolve(c: &mut Criterion) {
    let mut group = c.benchmark_group("check_tool_resolve");
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("node_modules/.bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let local = bin_dir.join("tsgo");
    std::fs::write(&local, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    group.bench_function("hit_node_modules_bin", |b| {
        b.iter(|| {
            // The project-local install short-circuits before the PATH
            // walk — the common warm-workspace shape.
            let resolved = oxide_fw_cli::resolve_tool("tsgo", tmp.path());
            assert!(resolved.is_some());
            black_box(resolved);
        })
    });
    group.bench_function("miss_full_path_walk", |b| {
        b.iter(|| {
            // No install, no PATH entry: the full walk runs and answers
            // None (the SKIPPED line's cost).
            let resolved = oxide_fw_cli::resolve_tool("oxide-fw-not-a-tool", tmp.path());
            assert!(resolved.is_none());
            black_box(resolved);
        })
    });
    group.finish();
}

criterion_group!(benches, bench_check_tool_resolve);
criterion_main!(benches);
