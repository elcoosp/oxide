//! The capability-matrix walkthrough (V2-R02): print the matrix the way
//! `oxide doctor`'s successor will, watch the map and the function agree,
//! and watch the §18 refusal rule refuse a configuration the target
//! cannot honor.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-adapters --example capability_matrix
//! ```

use oxide_fw_adapters::{BuildArtifact, Capability, FeatureSupport, Target, capability_matrix};
use std::collections::BTreeMap;

fn main() {
    // 1. One row per target class — every capability has a cell (the
    //    compiler proves the totality; this prints it).
    for t in Target::ALL {
        let matrix: BTreeMap<Capability, FeatureSupport> = capability_matrix(*t);
        let row: String = Capability::ALL
            .iter()
            .map(|c| match matrix.get(c) {
                Some(FeatureSupport::Full) => 'F',
                Some(FeatureSupport::Degraded) => 'd',
                _ => '·',
            })
            .collect();
        println!("{:>16}  {row}", t.as_str());
    }
    // A plain literal header (no format args — clippy::print_literal).
    println!(
        "             capability  L0 Is Sock Img Rules Snap Tls Live MI Res Skew Delta Rust Srch Off Brn Perm Att"
    );

    // 2. The map and the function cannot disagree (the fold guarantees it):
    let static_live = capability_matrix(Target::Static)
        .get(&Capability::Live)
        .copied();
    assert_eq!(static_live, Some(FeatureSupport::Unsupported));
    println!(
        "static × live = {static_live:?} — the build refuses `live: true` on Static (§18 refusal rule)"
    );

    // 3. The refusal rule, live: a server manifest requiring `live` on the
    //    static target fails the build instead of failing at runtime.
    let artifact = BuildArtifact {
        out_dir: std::env::temp_dir(),
        build_id: "walkthrough".into(),
        client_manifest: serde_json::json!({}),
        server_manifest: serde_json::json!({ "requires": { "live": true } }),
        prerendered: BTreeMap::new(),
        static_fallback: None,
        server_bundle: None,
    };
    let out = std::env::temp_dir().join("oxide-capability-walkthrough");
    match oxide_fw_adapters::emit(Target::Static, &artifact, &out) {
        Err(e) => println!("refused: {}: {}", e.0.code, e.0.message),
        Ok(_) => println!("unexpected: the build proceeded"),
    }

    // 4. A Degraded capability degrades — it never refuses:
    let lambda_live = capability_matrix(Target::Lambda)
        .get(&Capability::Live)
        .copied();
    let reason = oxide_fw_adapters::degradation(Target::Lambda, Capability::Live);
    println!(
        "lambda × live = {lambda_live:?} — {}",
        reason.unwrap_or("no documented reason")
    );

    // 5. The E11 docs table: the same matrix rendered as the markdown the
    //    crate README embeds (`doc_table_matches_the_function` holds the
    //    docs and this render together).
    println!("\ncapability_markdown_table() (the README's embedded table):\n");
    print!("{}", oxide_fw_adapters::capability_markdown_table());
}
