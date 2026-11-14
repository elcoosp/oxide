//! V2-C10 MDX spike — the adapter-neutrality question, probed against
//! `snapfire_fsr_mdx` (crates.io 0.2.3, 2026-11-09 — fresh; pins oxc
//! 0.155, matching the workspace). The decision record lives at
//! `docs/decisions/C10-mdx-kill.md`; the WP's status is spiked-killed —
//! this file is the re-runnable probe, the ONLY consumer of the
//! dev-dependency.
//!
//! Run: `cargo test -p oxide-fw-assets --test mdx_native_spike -- --ignored --nocapture`

use snapfire_fsr_mdx::{ClassAttr, Format, Options, Target};

const DOC: &str = r#"# Hello

A paragraph with **bold** and a [link](https://example.com).

- one
- two
"#;

#[test]
#[ignore = "C10 spike probe — run with --ignored --nocapture"]
fn spike_mdx_output_is_jsx_shaped() {
    let options = Options::new(Format::Mdx, Target::Browser);
    let converted = snapfire_fsr_mdx::convert(DOC, &options).expect("mdx converts");
    println!("=== browser target output (first 40 lines) ===");
    for line in converted.tsx.lines().take(40) {
        println!("{line}");
    }
    // The decisive fact for the decision record: the emitted module is
    // JSX-classed output — `className` (React) or `class` per ClassAttr —
    // consumed by a JSX runtime. Only React (and Solid, via its own
    // preset) can render it; the Vue and Svelte adapters compile SFCs and
    // have no JSX runtime at all. Adapter neutrality (the spec's
    // condition (a)) fails.
    let opts_react = Options::new(Format::Mdx, Target::Browser);
    let react_out = snapfire_fsr_mdx::convert(DOC, &opts_react).unwrap().tsx;
    let mut vue_opts = Options::new(Format::Mdx, Target::Browser);
    vue_opts.class_attr = ClassAttr::Class;
    let vue_out = snapfire_fsr_mdx::convert(DOC, &vue_opts).unwrap().tsx;
    println!(
        "=== ClassAttr::Class output differs from ClassName: {} ===",
        react_out != vue_out
    );
    // The emitted module is a JSX/TSX module (jsx-runtime calls), which is
    // the whole point: JSX is the adapter boundary the Vue and Svelte
    // adapters cannot consume.
    assert!(
        react_out.contains("_jsx") || react_out.contains("jsx-runtime") || react_out.contains("<"),
        "the emitted module is JSX-classed output, got: {}",
        &react_out[..react_out.len().min(400)]
    );
}
