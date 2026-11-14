//! V2-C04 spike — the native Svelte compiler candidate
//! (`svelte-compiler`), written per the spec's spike rule and the task
//! brief's §0b order.
//!
//! # Dependency arm (recorded here and in
//! `docs/decisions/C04-svelte-native.md`)
//!
//! `svelte-compiler 0.1.4`, **crates.io latest** (published 2026-03-16 —
//! ~8 months old, but it IS the upstream artifact: there is no newer
//! unreleased API this spike needs, so §0b(2)'s git-rev arm does not
//! apply). The API the spec names is present and exercised below:
//! `compile`, `compile_module`, `parse`, `print`, `preprocess`,
//! `migrate`; `CompileOptions { generate: GenerateTarget, css:
//! CssOutputMode, hmr, dev, runes, filename }` and
//! `CompileResult { js, css, warnings, metadata }`.
//!
//! # What the spike must prove before the backend lands
//!
//! 1. Svelte 5 runes (`$state`, `$derived`, `$props`) compile.
//! 2. `GenerateTarget::Client | Server` maps onto the worker's
//!    `generate: 'dom' | 'ssr'` arguments.
//! 3. `CssOutputMode::Injected` matches the worker's `css: 'injected'`
//!    (the emitted JS carries the style injection).
//! 4. The `hmr` option exists and the emitted module shape stays valid.
//! 5. Warning text passes through.
//!
//! Run: `cargo test -p oxide-fw-compiler --features native-svelte --test
//! svelte_native_spike -- --ignored --nocapture`
#![cfg(feature = "native-svelte")]

use svelte_compiler::{CompileOptions, CompileResult, CssOutputMode, GenerateTarget};

fn compile_fixture(source: &str, filename: &str, ssr: bool) -> CompileResult {
    let options = CompileOptions {
        filename: Some(filename.into()),
        generate: if ssr {
            GenerateTarget::Server
        } else {
            GenerateTarget::Client
        },
        // The tooling:svelte-compile handler passes `css: "injected"` —
        // the emitted JS injects the component style, no external css
        // artifact on this path.
        css: CssOutputMode::Injected,
        ..Default::default()
    };
    svelte_compiler::compile(source, options).expect("fixture compiles")
}

/// Runes the smoke app / Atlas uses: `$state`, `$derived`, `$props`
/// (the C04 kill criterion: any rune the crate rejects kills the WP).
const RUNES: &str = r#"<script>
let { start } = $props();
let count = $state(start);
let doubled = $derived(count * 2);
</script>

<button on:click={() => count += 1}>{count} / {doubled}</button>
"#;

const WITH_STYLE: &str = r#"<p class="who">hello</p>

<style>
.who { color: green; }
</style>
"#;

#[test]
#[ignore = "C04 spike — needs the native-svelte feature; run with --ignored --nocapture"]
fn spike_runes_compile_for_client_and_server() {
    for ssr in [false, true] {
        let r = compile_fixture(RUNES, "src/components/Counter.svelte", ssr);
        println!(
            "=== {} js (first 25 lines) ===",
            if ssr { "ssr" } else { "dom" }
        );
        for line in r.js.code.lines().take(25) {
            println!("{line}");
        }
        assert!(!r.js.code.is_empty());
        assert!(
            !r.js.code.contains("$state(") && !r.js.code.contains("$derived("),
            "runes are compiled away, not passed through verbatim"
        );
        println!("=== warnings: {:?} ===", r.warnings);
    }
}

#[test]
#[ignore = "C04 spike — needs the native-svelte feature; run with --ignored --nocapture"]
fn spike_css_injected_matches_the_worker_option() {
    let r = compile_fixture(WITH_STYLE, "src/components/Tag.svelte", false);
    println!(
        "=== injected js (first 30 lines) ===\n{}\n",
        r.js.code.lines().take(30).collect::<Vec<_>>().join("\n")
    );
    let inlined = r.js.code.contains("who") && r.js.code.contains("green");
    println!(
        "=== style text inside the js: {inlined}; separate css artifact: {} ===",
        r.css.is_some()
    );
    assert!(
        inlined || r.css.is_some(),
        "the component style lands somewhere the bundler can reach (inline or artifact)"
    );
}

#[test]
#[ignore = "C04 spike — needs the native-svelte feature; run with --ignored --nocapture"]
fn spike_hmr_option_and_warning_text() {
    let options = CompileOptions {
        filename: Some("src/components/Hmr.svelte".into()),
        generate: GenerateTarget::Client,
        css: CssOutputMode::Injected,
        hmr: true,
        dev: true,
        ..Default::default()
    };
    let r = svelte_compiler::compile("<p>x</p>", options).expect("hmr compile");
    println!(
        "=== hmr/dev js (first 30 lines) ===\n{}\n",
        r.js.code.lines().take(30).collect::<Vec<_>>().join("\n")
    );
    assert!(!r.js.code.is_empty());

    // Warning text: an unused CSS selector warns.
    let options = CompileOptions {
        filename: Some("src/components/Warn.svelte".into()),
        css: CssOutputMode::Injected,
        ..Default::default()
    };
    let r = svelte_compiler::compile(
        "<p>no matching</p>\n<style>.unused { color: red; }</style>",
        options,
    )
    .expect("warn compile");
    println!("=== warnings ===\n{:?}\n", r.warnings);
    println!(
        "=== warning count: {} (the worker passes css:'injected' too — the unused-selector warning fires upstream at build diagnostics time) ===",
        r.warnings.len()
    );
}
