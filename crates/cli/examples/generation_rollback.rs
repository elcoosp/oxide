//! §V2-F10 step 4: the generation-rollback walkthrough — the retained
//! generations layout the build's emit tail keeps, the `previous`
//! resolution and the dist swap `oxide rollback` performs, driven
//! in-process over synthetic dists (no server, no toolchain).
//!
//! Run with:
//!
//! ```sh
//! cargo run -p oxide-fw-cli --no-default-features --example generation_rollback
//! ```
//!
//! The real commands over a built project (`deploy.generations` dists
//! retained under `.oxide/generations/<id>/dist`):
//!
//! ```sh
//! oxide build                # the emit tail retains dist as a generation
//! oxide start --generation previous   # boot the OLD build's bytes
//! oxide rollback             # repoint dist + SIGHUP the running serve
//! ```

use oxide_fw_cli::serve::generations::{
    repoint_dist_to_generation, resolve_generation, retain_dist_generation, retained_generations,
};

/// Stamp a synthetic dist through the A04 seam (the same call the emit
/// tail makes): distinct bodies mint distinct build ids.
fn stamp_dist(root: &std::path::Path, body: &[u8]) -> String {
    let dist = root.join("dist");
    std::fs::create_dir_all(dist.join("client/assets")).expect("mkdir");
    std::fs::write(dist.join("client/assets/a.js"), body).expect("write");
    oxide_fw_cache::stamp_build_id(&dist).expect("stamp")
}

fn main() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    println!("== the emit tail: three builds, each retained (keep = deploy.generations) ==");
    for (n, body) in [
        (1usize, b"export const v = 1;\n".as_slice()),
        (2usize, b"export const v = 2;\n".as_slice()),
        (3usize, b"export const v = 3;\n".as_slice()),
    ] {
        let id = stamp_dist(root, body);
        retain_dist_generation(root, &id, 3).expect("retain");
        println!("build {n} — build-id {id} retained under .oxide/generations/{id}/dist");
    }

    let retained = retained_generations(root);
    println!("\n== retained (newest first) ==");
    for id in &retained {
        println!("{id}");
    }

    let v3 = &retained[0];
    let gen_dist = resolve_generation(root, "previous", Some(v3)).expect("previous resolves");
    let previous_id = oxide_fw_cache::read_build_id(&gen_dist).expect("build id");
    println!("\n== `previous` (current is {v3}) ==");
    println!("resolves to {previous_id}");

    println!("\n== `oxide rollback` (the swap, no rebuild) ==");
    repoint_dist_to_generation(root, &gen_dist).expect("repoint");
    let served = oxide_fw_cache::read_build_id(&root.join("dist")).expect("served id");
    let bytes = std::fs::read_to_string(root.join("dist/client/assets/a.js")).expect("bytes");
    println!("dist now serves build-id {served}");
    println!("dist now serves bytes: {}", bytes.trim());

    println!("\n== the running serve's half (a real boot writes .oxide/serve.pid) ==");
    println!("oxide rollback delivers SIGHUP — the promote re-reads the repointed dist,");
    println!("demotes the pre-rollback id into its deploy.skewWindow window, and the");
    println!("two-pool route step keeps old-tab data-plane calls on the OLD bytes.");
}
