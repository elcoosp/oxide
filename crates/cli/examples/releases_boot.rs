//! The F11 `releases/` layout boot walkthrough (§V2-F11's store layout
//! consumed by §V2-F10's serve boot) — the `--releases` leg's two pieces
//! driven in-process over a synthetic deployed store, plus the
//! `--watch-dist` tick the serve boot spawns (no server, no toolchain).
//!
//! Run with:
//!
//! ```sh
//! cargo run -p oxide-fw-cli --no-default-features --example releases_boot
//! ```
//!
//! The real commands over a deployed store (`oxide deploy dir:/srv/app`
//! writes the layout — the pointer LAST):
//!
//! ```sh
//! oxide start --releases /srv/app          # boot the deployed release
//! oxide start --releases /srv/app --watch-dist   # ...and promote rebuilds
//! ```

use oxide_fw_adapters::{DeployStore, DirStore};
use oxide_fw_cli::serve::generations::{SkewState, dist_watch_next_id};
use oxide_fw_cli::serve::releases::{boot_sources_conflict, resolve_release_boot};

/// Build a synthetic deployed store the `dir:` target's shape: 64
/// immutable chunk files under `assets/` (shared across releases), a
/// release prefix with the non-hashed dist files, the pointer written
/// last.
fn seed_store(root: &std::path::Path, id: &str, body: &[u8]) {
    let rel = root.join("releases").join(id);
    std::fs::create_dir_all(rel.join("prerendered")).expect("mkdir");
    std::fs::write(rel.join("build-id"), format!("{id}\n")).expect("build-id");
    std::fs::write(
        rel.join("prerendered/index.html"),
        format!("<html>{}</html>", String::from_utf8_lossy(body)),
    )
    .expect("html");
    std::fs::create_dir_all(root.join("assets")).expect("mkdir assets");
    std::fs::write(root.join("assets/app.js"), b"export const v = 1;\n").expect("asset");
    DirStore::new(root).put_pointer(id).expect("pointer");
}

fn main() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = tmp.path();

    println!(
        "== the deploy: release A written (immutable chunks at the root, the pointer last) =="
    );
    seed_store(store, "0123456789abcdef", b"release A");
    println!(
        "store: {}/releases/0123456789abcdef + assets/ + current",
        store.display()
    );

    println!("\n== `oxide start --releases`: resolve the pointer, materialize the dist view ==");
    let boot = resolve_release_boot(store).expect("boot");
    println!("pointer names build-id {}", boot.build_id);
    println!("release prefix: {}", boot.release_root.display());
    println!("materialized view: {}", boot.dist.display());
    println!(
        "  prerendered/index.html -> {:?}",
        std::fs::read_to_string(boot.dist.join("prerendered/index.html")).expect("html")
    );
    println!(
        "  assets/app.js (a store-root immutable chunk) -> {} bytes",
        std::fs::read(boot.dist.join("assets/app.js"))
            .expect("asset")
            .len()
    );
    println!(
        "  build-id -> {:?}",
        oxide_fw_cache::read_build_id(&boot.dist).expect("build id")
    );

    println!("\n== the second deploy: release B swaps the pointer (written last) ==");
    seed_store(store, "fedcba9876543210", b"release B");
    let boot_b = resolve_release_boot(store).expect("boot B");
    println!(
        "the boot view follows the pointer: build-id {} (was {})",
        boot_b.build_id, boot.build_id
    );

    println!("\n== the boot sources: `--generation` and `--releases` both repoint dist/ ==");
    println!(
        "both named -> conflict: {}",
        boot_sources_conflict(Some("previous"), Some("/srv/app"))
    );

    println!("\n== `--watch-dist`: the tick the serve task runs every 2s ==");
    let dist = boot_b.dist.clone();
    let id_a = oxide_fw_cache::read_build_id(&dist).expect("read");
    println!(
        "last-seen {id_a}; same dist again -> {:?}",
        dist_watch_next_id(Some(&id_a), &dist)
    );
    // A rebuilt dist: a changed chunk moves the A04 build id.
    std::fs::write(
        dist.join("prerendered/index.html"),
        "<html>release C</html>",
    )
    .expect("rewrite");
    let id_c = oxide_fw_cache::stamp_build_id(&dist).expect("restamp");
    println!("rebuilt dist -> the tick names {id_c} (the promote core takes it from here)");

    println!("\n== the promote core (the SIGHUP handler and the watcher share it) ==");
    let skew = SkewState::from_boot(&id_a, "15m", 3);
    let promoted = skew.promote_from_dist(&dist);
    println!(
        "promoted {id_c:?}: the previous generation {id_a} stays accepted for its deploy.skewWindow"
    );
    let _ = promoted;
    println!("\nDone — the same arcs run on the wire through `oxide start --releases <STORE>`.");
}
