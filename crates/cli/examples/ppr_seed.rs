//! §10.4 / P0-4 walkthrough: the build-time PPR shell seed — the half the
//! runtime shell cache never had. The prerender walk renders every static
//! page at build; a page whose body carries `<template data-ox-b>`
//! boundaries yields its shell through the same `split_shell` the runtime
//! tap runs, under the SAME canonical L0 key the render stage computes.
//! `phase_ppr_seed` persists the rows as `dist/server/ppr-shells.json` (the
//! `dist/server/l0-seed.json` pattern), and the serve boot seeds the shell
//! store from it — the first request after a deploy is a shell HIT, not a
//! warm-up tap.
//!
//! The example drives the public seed contract (no Node, no pnpm, no V8):
//! the walk's source rows → the collector's rows → the manifest bytes → the
//! boot's read verdict (same-build rows / stale-build skip / corrupt warn),
//! and prints the key-fidelity fact the lib suite pins end to end
//! (`ppr_seed_boot_store_serves_the_first_request_without_a_tap`): the
//! seeded key IS the render stage's lookup key, computed with the same
//! `L0Cache::key_for` over the walk's (route, params, vary, build).
//!
//! ```text
//! cargo run -p oxide-fw-cli --no-default-features --example ppr_seed
//! ```

use oxide_fw_cli::{
    PprSeedSource, PprShellSeed, parse_ppr_shell_seed, ppr_shell_seed_manifest, ppr_shell_seed_rows,
};

/// The walk's rendered row: a hero, one boundary (fallback inside), the
/// data-ox-c fragment, all sentineled (the build nonce already swapped for
/// `__OXIDE_L0_NONCE__` — the `seed_bodies` post-processing).
fn fixture_page() -> String {
    "<!doctype html><html><head><meta nonce=\"__OXIDE_L0_NONCE__\"></head>\
<body><h1>STATIC-SHELL</h1>\
<template data-ox-b='1'><p>fallback</p></template>\
<template data-ox-c='1'><p>LIVE-HOLE</p></template>\
</body></html>"
        .to_string()
}

/// A boundary-free page — the collector contributes nothing for it (§10.4:
/// the shell exists only "up to each boundary").
fn fixture_static_page() -> String {
    "<!doctype html><html><body><p>plain</p></body></html>".to_string()
}

fn main() {
    // The vary fingerprint an EMPTY request produces under the boot's own
    // vary config — the default `cache.l0.vary` = `accept-language`. (The
    // build derives this from config; the seed_l0 manifest's hardcoded
    // `lang=` spelling predates the §11.2 fingerprint and never matches it.)
    let vary = "accept-language=";
    let build_id = "20261125.1";
    let sources = vec![
        PprSeedSource {
            route_id: "routes/index".into(),
            url_path: "/".into(),
            params: String::new(),
            sentineled_html: fixture_page(),
            locale: String::new(),
        },
        PprSeedSource {
            route_id: "routes/about".into(),
            url_path: "/about".into(),
            params: String::new(),
            sentineled_html: fixture_static_page(),
            locale: String::new(),
        },
        PprSeedSource {
            route_id: "routes/blog".into(),
            url_path: "/fr/blog".into(),
            params: String::new(),
            sentineled_html: fixture_page(),
            locale: "fr".into(),
        },
    ];
    let rows = ppr_shell_seed_rows(&sources, build_id, vary, "en");
    println!("== the walk's rendered pages become shell rows");
    for row in &rows {
        println!(
            "  key {} <- {} (url {}, tags {:?})",
            &row.key[..12],
            row.tags[0],
            row.url,
            row.tags
        );
        let body_at = row.shell.find("<body>").unwrap_or(0);
        let excerpt: String = row.shell[body_at..].chars().take(96).collect();
        println!("    shell: {excerpt}…");
    }
    println!(
        "  ({}/{} rendered pages carried boundaries; the boundary-free page and the fr variant contributed nothing)",
        rows.len(),
        sources.len()
    );

    println!("\n== phase_ppr_seed persists dist/server/ppr-shells.json");
    let bytes = ppr_shell_seed_manifest(&rows, build_id);
    println!("  manifest: {} bytes, build_id {build_id}", bytes.len());

    println!("\n== the serve boot parses the manifest at startup");
    match parse_ppr_shell_seed(&bytes, build_id) {
        PprShellSeed::Rows(parsed) => {
            println!(
                "  same build -> {} row(s) seeded into the shell store",
                parsed.len()
            );
            println!("  the store's lookup key IS this key: the render stage computes");
            println!("  key_for(route, params, vary, build) over the request — the seeded");
            println!("  row is a FIRST-REQUEST hit, not a warm-up tap.");
        }
        other => println!("  unexpected: {other:?}"),
    }
    match parse_ppr_shell_seed(&bytes, "another-build") {
        PprShellSeed::StaleBuild => {
            println!(
                "  another build's manifest -> StaleBuild (the same_build skip; a redeploy leaves it behind)"
            )
        }
        other => println!("  unexpected: {other:?}"),
    }
    match parse_ppr_shell_seed(b"{not json", build_id) {
        PprShellSeed::Corrupt(e) => {
            println!("  corrupt manifest -> Corrupt({e}) (the boot warns under CACHE-SEED)")
        }
        other => println!("  unexpected: {other:?}"),
    }
}
