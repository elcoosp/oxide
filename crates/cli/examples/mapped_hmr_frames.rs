//! §V2-E08 item 1: the mapped-frames CLI adoption walkthrough — the
//! boot's module-map store (what the dev lowering arm fills), the
//! publish-site helper (`mapped_hmr_error`: the honest generated frame +
//! the store's mapping) and the §13.4 payload the overlay renders —
//! driven in-process over a real lowered module (no server, no V8).
//!
//! Run with:
//!
//! ```sh
//! cargo run -p oxide-fw-cli --no-default-features --example mapped_hmr_frames
//! ```
//!
//! On the wire the same arc runs inside `oxide dev`: the ESM dev server's
//! lowering arm fills `EsmDevState.module_maps`, and the watcher loop's
//! three `HmrMessage::Error` sites publish through the helper — the
//! browser overlay's stack then names the ORIGINAL source.

use oxide_fw_cli::{mapped_hmr_error, src_rel_key};
use oxide_fw_compiler::lower_to_js_with_refresh;
use oxide_fw_dev::frames::{ModuleMaps, overlay_payload_with_maps};
use std::sync::{Arc, Mutex};

fn main() {
    // 1. The dev session's module map store (the EsmDevState field the
    //    lowering arm fills per served module).
    let maps = Arc::new(Mutex::new(ModuleMaps::default()));

    // 2. A module the dev server lowers (the serve arm's own transform):
    //    the map it produces joins the store keyed by the compiler path.
    let src = "export default function Page() {\n  return <main>hi</main>;\n}\n";
    let path = "src/routes/page.tsx";
    let (code, map) = lower_to_js_with_refresh(src, "tsx", path, None).expect("lower");
    println!(
        "== the lowering arm: {} produced {} bytes of lowered JS ==",
        path,
        code.len()
    );
    if let Some(map) = map {
        maps.lock().expect("unpoisoned").insert(path, map);
        println!("its oxc map joined the store under '{path}'");
    }

    // 3. The watcher's publish site: a rebuild failure for the changed
    //    module. The helper attaches the honest generated frame and maps
    //    it through the store — the payload's stack names the SOURCE.
    let changed = std::path::Path::new(path);
    println!("\n== the publish site: the src-relative frame key ==");
    println!(
        "src_rel_key({path:?}) = {:?}",
        src_rel_key(std::path::Path::new("/proj"), changed)
    );
    let diag =
        oxide_fw_diagnostics::Diagnostic::error("DEV-BUNDLE", "rebuild failed for the page module");
    let mapped = mapped_hmr_error(Some(&maps), std::path::Path::new("/proj"), changed, diag);
    let frame = &mapped.frames[0];
    println!(
        "frame: {}:{}:{} mapped={}",
        frame.file, frame.line, frame.column, frame.mapped
    );

    // 4. The §13.4 payload the overlay renders (the v1 envelope the
    //    client's `error` switch reads).
    println!("\n== the overlay payload (§13.4) ==");
    let payload = overlay_payload_with_maps(&mapped, &*maps.lock().expect("unpoisoned"));
    println!("{payload}");

    println!("\nDone — `oxide dev` publishes exactly this shape on an HMR error.");
}
