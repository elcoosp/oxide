//! TDD pins for the V2-A04 build-id substrate (`build_id_of_dist`,
//! spec v0.2 §V2-A04 step 1).
//!
//! The build id is the canonical identity of one emitted build: blake3 over
//! the sorted `relative/path<TAB>blake3-hex` manifest of every file under
//! `dist/client/**`, `dist/server/**` and `.oxide/routes.manifest`
//! (relative to the project root, i.e. `dist`'s parent), truncated to the
//! 16-hex convention the existing `build_id(&[&[u8]])` helper established.
//!
//! Written FIRST (red), per the spec's agent contract V2.0 item 3: the pins
//! below failed on unresolved-symbol errors before `build_id_of_dist`
//! existed. Wiring (writing `dist/build-id` / `.oxide/build-id` at the end
//! of `phase_emit`) is deferred to the A01-phase-3 CLI adoption; see
//! `docs/deviations.md`.

use std::fs;
use std::path::Path;

use oxide_fw_cache::build_id_of_dist;
use tempfile::TempDir;

/// Create `p` (creating parents) with `contents`; the LAST writer wins.
fn write(root: &Path, rel: &str, contents: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdirs");
    fs::write(p, contents).expect("write");
}

#[test]
fn id_is_16_hex() {
    let tmp = TempDir::new().expect("tmp");
    let root = tmp.path();
    write(root, "dist/client/a.js", b"console.log(1)");
    let id = build_id_of_dist(&root.join("dist")).expect("id");
    assert_eq!(id.len(), 16, "{id}");
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
}

#[test]
fn build_id_of_dist_is_stable_and_order_independent() {
    // The spec's named pin (§V2-A04 "Tests"): two temp trees with the same
    // files in different creation order must agree.
    let a = TempDir::new().expect("tmp-a");
    let ra = a.path();
    write(ra, "dist/client/z.js", b"z");
    write(ra, "dist/client/a.js", b"a");
    write(ra, "dist/server/entry.js", b"serve");
    write(ra, ".oxide/routes.manifest", b"routes");

    let b = TempDir::new().expect("tmp-b");
    let rb = b.path();
    write(rb, "dist/server/entry.js", b"serve");
    write(rb, "dist/client/a.js", b"a");
    write(rb, ".oxide/routes.manifest", b"routes");
    write(rb, "dist/client/z.js", b"z");

    let ia = build_id_of_dist(&ra.join("dist")).expect("a");
    let ib = build_id_of_dist(&rb.join("dist")).expect("b");
    assert_eq!(ia, ib, "same files, different creation order");

    // Re-computing over an unchanged tree is byte-stable.
    assert_eq!(ia, build_id_of_dist(&ra.join("dist")).expect("a2"));
}

#[test]
fn one_changed_byte_changes_the_id() {
    let tmp = TempDir::new().expect("tmp");
    let root = tmp.path();
    write(root, "dist/client/a.js", b"version-one");
    let before = build_id_of_dist(&root.join("dist")).expect("before");
    write(root, "dist/client/a.js", b"version-two");
    let after = build_id_of_dist(&root.join("dist")).expect("after");
    assert_ne!(before, after, "one source byte must move the id");
}

#[test]
fn covers_client_server_and_routes_manifest() {
    // Each of the three covered inputs moves the id on its own.
    let base = |root: &Path| build_id_of_dist(&root.join("dist")).expect("base");

    let t1 = TempDir::new().expect("t1");
    write(t1.path(), "dist/client/a.js", b"a");
    let with_client = base(t1.path());

    let t2 = TempDir::new().expect("t2");
    write(t2.path(), "dist/client/a.js", b"a");
    write(t2.path(), "dist/server/m.js", b"m");
    let with_server = base(t2.path());
    assert_ne!(with_client, with_server, "dist/server/** participates");

    let t3 = TempDir::new().expect("t3");
    write(t3.path(), "dist/client/a.js", b"a");
    write(t3.path(), "dist/server/m.js", b"m");
    write(t3.path(), ".oxide/routes.manifest", b"routes");
    let with_manifest = base(t3.path());
    assert_ne!(
        with_server, with_manifest,
        ".oxide/routes.manifest participates"
    );

    // A manifest byte flip moves the id too (the route table is contract).
    write(t3.path(), ".oxide/routes.manifest", b"routes-v2");
    assert_ne!(with_manifest, base(t3.path()), "manifest bytes participate");
}

#[test]
fn paths_outside_the_covered_trees_are_ignored() {
    // §V2-A04 covers dist/client/**, dist/server/** and
    // .oxide/routes.manifest — nothing else. Files elsewhere under dist/
    // (e.g. build logs) and under the project root must not move the id.
    let t1 = TempDir::new().expect("t1");
    write(t1.path(), "dist/client/a.js", b"a");
    write(t1.path(), "dist/server/m.js", b"m");
    let before = build_id_of_dist(&t1.path().join("dist")).expect("before");

    write(t1.path(), "dist/noise.txt", b"noise");
    write(t1.path(), "dist/assets/logo.svg", b"<svg/>");
    write(t1.path(), "src/main.rs", b"fn main() {}");
    let after = build_id_of_dist(&t1.path().join("dist")).expect("after");
    assert_eq!(before, after, "uncovered paths must not participate");
}

#[test]
fn nested_client_dirs_are_covered() {
    let t1 = TempDir::new().expect("t1");
    write(t1.path(), "dist/client/a.js", b"a");
    let before = build_id_of_dist(&t1.path().join("dist")).expect("before");
    write(t1.path(), "dist/client/chunks/deep/b.js", b"b");
    let after = build_id_of_dist(&t1.path().join("dist")).expect("after");
    assert_ne!(before, after, "files below nested dirs participate");
}

#[test]
fn missing_routes_manifest_and_missing_server_dir_are_tolerated() {
    // Wiring runs the function on whatever the emit phase produced; a
    // missing `.oxide/routes.manifest` (or a tree that only emitted the
    // client half) must not error — it only narrows the covered set.
    let t1 = TempDir::new().expect("t1");
    write(t1.path(), "dist/client/a.js", b"a");
    let id = build_id_of_dist(&t1.path().join("dist")).expect("client-only");
    assert_eq!(id.len(), 16);

    let t2 = TempDir::new().expect("t2");
    write(t2.path(), "dist/client/a.js", b"a");
    write(t2.path(), ".oxide/routes.manifest", b"routes");
    let with_manifest = build_id_of_dist(&t2.path().join("dist")).expect("with-manifest");
    assert_ne!(id, with_manifest);
}

#[test]
fn missing_dist_is_an_error() {
    let tmp = TempDir::new().expect("tmp");
    let missing = tmp.path().join("does-not-exist");
    let err = build_id_of_dist(&missing).expect_err("missing dist must error");
    assert!(err.0.code.starts_with("CAC-"), "{}", err.0.code);
}

#[test]
fn empty_trees_still_yield_a_deterministic_id() {
    let t1 = TempDir::new().expect("t1");
    fs::create_dir_all(t1.path().join("dist/client")).expect("mkdir");
    fs::create_dir_all(t1.path().join("dist/server")).expect("mkdir");
    let a = build_id_of_dist(&t1.path().join("dist")).expect("a");
    let t2 = TempDir::new().expect("t2");
    fs::create_dir_all(t2.path().join("dist/server")).expect("mkdir");
    fs::create_dir_all(t2.path().join("dist/client")).expect("mkdir");
    let b = build_id_of_dist(&t2.path().join("dist")).expect("b");
    assert_eq!(a, b, "empty trees agree regardless of dir creation order");
}

// --- M1: the stamp/read half (A04 step 1 wiring) ---------------------------

use oxide_fw_cache::{read_build_id, stamp_build_id};

#[test]
fn stamp_writes_both_files_and_read_round_trips() {
    // The spec's shape: `dist/build-id` (plain text) and `.oxide/build-id`,
    // both the canonical id `build_id_of_dist` computed over the final
    // bytes; `read_build_id` is the one read site the boot consumes.
    let tmp = TempDir::new().expect("tmp");
    let root = tmp.path();
    write(root, "dist/client/a.js", b"a");
    let dist = root.join("dist");
    let id = stamp_build_id(&dist).expect("stamp");
    assert_eq!(id.len(), 16, "{id}");
    assert_eq!(
        fs::read_to_string(dist.join("build-id"))
            .expect("dist/build-id")
            .trim(),
        id,
        "dist/build-id carries the id"
    );
    assert_eq!(
        fs::read_to_string(root.join(".oxide/build-id"))
            .expect(".oxide/build-id")
            .trim(),
        id,
        ".oxide/build-id carries the id"
    );
    assert_eq!(read_build_id(&dist).expect("read"), id, "round trip");
    assert_eq!(
        read_build_id(&root.join("dist")).as_deref(),
        Some(id.as_str())
    );
}

#[test]
fn stamp_is_idempotent_and_id_narrows_to_the_covered_set() {
    // dist/build-id lives at the dist ROOT and .oxide/build-id at the
    // project root — neither is inside the covered set (dist/client,
    // dist/server, routes.manifest), so a second stamp over the same tree
    // must yield the SAME id (a rebuild with no source change prints the
    // same dist/build-id — the spec's observable proof).
    let tmp = TempDir::new().expect("tmp");
    let root = tmp.path();
    write(root, "dist/client/a.js", b"a");
    let dist = root.join("dist");
    let first = stamp_build_id(&dist).expect("first");
    let second = stamp_build_id(&dist).expect("second");
    assert_eq!(first, second, "re-stamping an unchanged dist is stable");
    // The stamped files did not leak into the digest: the id equals the
    // bare computation over the same tree.
    assert_eq!(first, build_id_of_dist(&dist).expect("bare"));
    // One changed covered byte moves the id; a changed NON-covered file
    // (outside dist/client, dist/server and routes.manifest — e.g. the
    // stamped dist/build-id itself or a dist-root deploy note) does not.
    write(root, "dist/client/a.js", b"b");
    let third = stamp_build_id(&dist).expect("third");
    assert_ne!(first, third, "a covered byte change moves the id");
    write(root, "dist/deploy-note.txt", b"uncovered");
    let fourth = stamp_build_id(&dist).expect("fourth");
    assert_eq!(third, fourth, "non-covered files do not move the id");
}

#[test]
fn stamp_over_a_missing_dist_errors_and_read_of_a_missing_file_is_none() {
    let tmp = TempDir::new().expect("tmp");
    let missing = tmp.path().join("dist");
    let err = stamp_build_id(&missing).expect_err("missing dist must error");
    assert!(err.0.code.starts_with("CAC-"), "{}", err.0.code);
    assert!(read_build_id(&missing).is_none(), "no file, no id");
    let empty = TempDir::new().expect("empty");
    assert!(read_build_id(&empty.path().join("dist")).is_none());
}

#[test]
fn read_build_id_tolerates_surrounding_whitespace() {
    // The file is written trim-ended, but a hand-edited or CRLF file must
    // still read back as the id.
    let tmp = TempDir::new().expect("tmp");
    let dist = tmp.path().join("dist");
    fs::create_dir_all(&dist).expect("mkdir");
    fs::write(dist.join("build-id"), b"  0123456789abcdef\r\n").expect("write");
    assert_eq!(
        read_build_id(&dist).as_deref(),
        Some("0123456789abcdef"),
        "whitespace-trimmed read"
    );
}

#[test]
fn the_native_dir_joins_the_covered_set_when_present() {
    // §V2-F08 finish: `dist/native` is a covered input when the build
    // emitted one — a rebuilt library is a new build identity — and its
    // absence computes the same id as before the arm landed.
    let a = TempDir::new().expect("tmp-a");
    let ra = a.path();
    write(ra, "dist/client/a.js", b"a");
    let without = build_id_of_dist(&ra.join("dist")).expect("id without native");

    let b = TempDir::new().expect("tmp-b");
    let rb = b.path();
    write(rb, "dist/client/a.js", b"a");
    write(rb, "dist/native/lib-1.so", b"cdylib-bytes");
    let with = build_id_of_dist(&rb.join("dist")).expect("id with native");
    assert_ne!(without, with, "a native dir changes the identity");

    // The identity tracks the native BYTES: a rebuilt library is a new id.
    let c = TempDir::new().expect("tmp-c");
    let rc = c.path();
    write(rc, "dist/client/a.js", b"a");
    write(rc, "dist/native/lib-1.so", b"cdylib-bytes-v2");
    let rebuilt = build_id_of_dist(&rc.join("dist")).expect("rebuilt id");
    assert_ne!(with, rebuilt, "new native bytes, new id");

    // The manifest names the file (the covered-set contract).
    let d = TempDir::new().expect("tmp-d");
    let rd = d.path();
    write(rd, "dist/client/a.js", b"a");
    write(rd, "dist/native/types.json", b"{\"fns\":[]}");
    let with_types = build_id_of_dist(&rd.join("dist")).expect("id with types");
    assert_ne!(without, with_types, "the payload is covered too");
}
