//! Writes the golden ODF fixtures (spec §9.5) consumed by the Rust stability
//! test (`tests/integration.rs`) and the TS decoder suite
//! (`packages/oxide/test/odf.test.ts`).
//!
//! Run once whenever the wire format changes deliberately, then commit the
//! updated files:
//!
//! ```text
//! cargo run -p oxide-odf --example odf_fixture
//! ```

fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    oxide_odf::fixtures::write_all(dir).expect("writing golden fixtures");
    println!("wrote golden fixtures to {dir}");
}
