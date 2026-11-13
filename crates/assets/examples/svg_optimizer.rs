//! The SVG optimizer walkthrough (§V2-C07): the `SvgOptimizer` seam, the
//! preservation rules, the CAS key and the feature-aware selection — the
//! whole thing the import pipeline wires against the real `OxvgSvg`
//! backend and fakes, so the contract is readable without the bundler.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-assets --example svg_optimizer --features svg-oxvg
//! ```

use std::collections::BTreeSet;

use oxide_fw_assets::svg::{
    IdentitySvg, OxvgSvg, SvgOptimizer, assert_preserved, optimize_for_build, referenced_svg_ids,
    select_optimizer, svg_cas_key,
};

/// The WP's three named fixtures: an id referenced by `<use>`, an inline
/// `<style>`, and a `viewBox`-only shell.
const FIXTURES: [&str; 3] = [
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <!-- a build comment -->
  <path id="a" d="M0 0 L24 24 Z"/>
  <use href="#a"/>
</svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <style>.icon { fill: currentColor; }</style>
  <rect class="icon" width="24" height="24"/>
</svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
  <!-- generated icon shell -->
  <circle cx="24" cy="24" r="20"/>
</svg>"##,
];

/// A third-party backend shape: returns the document unchanged, but with
/// its own id/version — the D40 key dimensions the CAS key carries.
struct VendorSvg;

impl SvgOptimizer for VendorSvg {
    fn optimize(&self, svg: &str) -> oxide_fw_assets::Result<String> {
        Ok(svg.to_string())
    }
    fn id(&self) -> &'static str {
        "vendor"
    }
    fn version(&self) -> &'static str {
        "1.2.3"
    }
}

/// A backend that breaks the rules: it drops the `<use>` target. The
/// wrapper refuses it — the rules are the seam's contract, not a property
/// of one bundled implementation.
struct IdDropper;

impl SvgOptimizer for IdDropper {
    fn optimize(&self, _svg: &str) -> oxide_fw_assets::Result<String> {
        Ok(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M0 0 L24 24 Z"/><use href="#a"/></svg>"##.to_string())
    }
    fn id(&self) -> &'static str {
        "dropper"
    }
    fn version(&self) -> &'static str {
        "0.1.0"
    }
}

fn main() {
    println!("== the backends ==");
    for (name, opt) in [
        ("identity", Box::new(IdentitySvg) as Box<dyn SvgOptimizer>),
        ("oxvg", Box::new(OxvgSvg) as Box<dyn SvgOptimizer>),
        ("vendor", Box::new(VendorSvg) as Box<dyn SvgOptimizer>),
    ] {
        println!("  {name}: id={} version={}", opt.id(), opt.version());
    }

    println!("\n== the three named fixtures through OxvgSvg ==");
    for src in FIXTURES {
        let out = OxvgSvg
            .optimize(src)
            .expect("the bundled optimizer runs the fixture");
        assert_preserved(src, &out).expect("the rules hold on the real output");
        println!(
            "  {} bytes -> {} bytes (rules hold, viewBox/use/style intact)",
            src.len(),
            out.len()
        );
    }

    println!("\n== the referenced-id scan ==");
    let ids = referenced_svg_ids(FIXTURES[0]);
    let want: BTreeSet<String> = BTreeSet::from(["a".to_string()]);
    assert_eq!(ids, want);
    println!("  fixture 1 references {ids:?} — every one survives the optimizer");

    println!("\n== the rule refusal (AST-SVG-001) ==");
    let broken = IdDropper.optimize(FIXTURES[0]).expect("the fake runs");
    let err =
        assert_preserved(FIXTURES[0], &broken).expect_err("a dropped <use> target must be refused");
    println!("  {} : {}", err.0.code, err.0.message);

    println!("\n== the CAS key (D40: content + optimizer id + version) ==");
    let key_oxvg = svg_cas_key(&OxvgSvg, FIXTURES[0]);
    let key_vendor = svg_cas_key(&VendorSvg, FIXTURES[0]);
    println!("  oxvg   : {key_oxvg}");
    println!("  vendor : {key_vendor}");
    assert_ne!(key_oxvg, key_vendor, "the optimizer is a key dimension");

    println!("\n== the selection (svg.optimize knob) ==");
    let selected = select_optimizer(true).expect("svg-oxvg is compiled into this run");
    println!("  optimize: true  -> id={}", selected.id());
    let off = select_optimizer(false).expect("identity is always available");
    println!("  optimize: false -> id={}", off.id());

    println!("\n== the build hook (optimize_for_build) ==");
    let hooked = optimize_for_build(FIXTURES[0], true);
    println!("  knob on : {} bytes (optimized, rules hold)", hooked.len());
    let hooked_off = optimize_for_build(FIXTURES[0], false);
    assert_eq!(hooked_off, FIXTURES[0], "knob off is byte-identical");
    println!(
        "  knob off: {} bytes (byte-identical passthrough)",
        hooked_off.len()
    );

    println!("\nwalkthrough complete");
}
