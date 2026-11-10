//! The §13.3 message-schema v1 walkthrough (V2-E01 phase 3): build the
//! frames the dev loop publishes, see the `{v:1}` envelope, and watch the
//! v1 gate reject a legacy frame with the `DEV-HMR-003` story.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dev --example hmr_wire
//! ```

use oxide_fw_dev::boundary::{css_orphan_diagnostic, schema_mismatch_diagnostic};
use oxide_fw_dev::{HmrMessage, HmrWireError, UpdateEntry, UpdateKind, plan_classified};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

fn main() {
    // 1. The planner decides the fan-out for a changed module: a 2-deep
    //    chain into two component boundaries.
    let changed = Path::new("src/lib/util.ts");
    let mut importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    importers.insert(
        changed.to_path_buf(),
        vec![PathBuf::from("src/comp/a.tsx"), PathBuf::from("src/comp/b.tsx")],
    );
    let accepting: HashSet<PathBuf> = [
        PathBuf::from("src/comp/a.tsx"),
        PathBuf::from("src/comp/b.tsx"),
    ]
    .into_iter()
    .collect();
    let entries: HashSet<PathBuf> = HashSet::new();

    let classified = plan_classified(
        oxide_fw_dev::ChangeKind::Component,
        changed,
        &importers,
        &accepting,
        &entries,
    );
    println!(
        "plan_classified: kind={:?} full_reload={}",
        classified.kind,
        classified.is_full_reload()
    );

    // 2. The fan-out materializes the plan as ONE v1 Update frame — the
    //    changed file rides at the head, each boundary is an entry whose
    //    acceptedPath is the dev URL the client re-imports. (The stamps
    //    here are fixed so the walkthrough output is reproducible; the dev
    //    loop stamps publish time and hashes the file on disk.)
    let mut updates = vec![UpdateEntry {
        kind: UpdateKind::Js,
        path: changed.display().to_string(),
        accepted_path: "/src/lib/util.ts".to_string(),
        timestamp: 1_760_000_000_000,
        hash: String::new(),
    }];
    for boundary in &accepting {
        updates.push(UpdateEntry {
            kind: UpdateKind::Js,
            path: boundary.display().to_string(),
            accepted_path: format!("/{}", boundary.display()),
            timestamp: 1_760_000_000_000,
            hash: String::new(),
        });
    }
    let update = HmrMessage::Update { updates };
    println!("update frame: {}", update.to_json());

    // 3. The non-entry frames: the pushed loader payload and the reload
    //    fallback, both under the same v1 envelope.
    let data = HmrMessage::OxData {
        route: "/slow".to_string(),
        data: "T0RGMQ==".to_string(),
    };
    let reload = HmrMessage::FullReload {
        reason: "src/routes/layout.tsx has no HMR boundary".to_string(),
    };
    println!("ox:data frame: {}", data.to_json());
    println!("full-reload frame: {}", reload.to_json());

    // 4. The CSS terminal: a stylesheet collapse still warns when the sheet
    //    is orphaned (DEV-HMR-002) — the diagnostic rides an Error frame.
    let orphan = css_orphan_diagnostic(Path::new("src/styles/ghost.css"));
    println!("orphan diagnostic: {}", orphan.to_json());

    // 5. The v1 gate: a legacy v0 frame (no version field) is a Schema
    //    mismatch, not a parse error — DEV-HMR-003 carries the fix text.
    let legacy = r#"{"type":"module-update","path":"src/lib/util.ts","url":"/src/lib/util.ts"}"#;
    match HmrMessage::try_from_json(legacy) {
        Err(HmrWireError::Schema { found, expected }) => {
            println!("legacy frame rejected: found v{found}, expected v{expected}");
            let diag = schema_mismatch_diagnostic(found);
            println!("DEV-HMR-003: {} — fix: {}", diag.message, diag.fix.unwrap_or_default());
        }
        other => println!("unexpected: {other:?}"),
    }

    // 6. The round trip: a v1 frame decodes back through the same gate.
    let decoded = HmrMessage::from_json(&update.to_json());
    println!("v1 round trip: {}", decoded.is_some());
}
