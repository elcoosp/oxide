//! §V2-C01 "Equivalence harness" pin: `scripts/compiler-equiv.sh
//! <framework>` is the acceptance gate for every landed native compiler
//! (C03 vue, C04 svelte; C02 solid is killed upstream, C05 is the
//! opt-in React Compiler). The test shells out to the script and
//! asserts exit 0 + the green diff line.
//!
//! #[ignore]d: it needs a RELEASE `oxide` binary with the native
//! features compiled in and runs real builds — the CI equivalence job
//! and the per-WP acceptance runs execute it:
//!
//! ```text
//! cargo build --release -p oxide-fw-cli --features native-vue,native-svelte,native-react-compiler
//! cargo test -p oxide-fw-compiler --test compiler_equiv_tdd -- --ignored
//! scripts/compiler-equiv.sh vue
//! scripts/compiler-equiv.sh svelte
//! ```

#[test]
#[ignore = "invokes scripts/compiler-equiv.sh <fw>; needs a release binary with the native features"]
fn compiler_equiv_vue() {
    run_equiv("vue");
}

#[test]
#[ignore = "invokes scripts/compiler-equiv.sh <fw>; needs a release binary with the native features"]
fn compiler_equiv_svelte() {
    run_equiv("svelte");
}

fn run_equiv(fw: &str) {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let root = std::path::Path::new(manifest)
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf();
    let script = root.join("scripts/compiler-equiv.sh");
    assert!(script.is_file(), "the harness script must exist");
    let out = std::process::Command::new(&script)
        .arg(fw)
        .current_dir(&root)
        .output()
        .expect("the harness runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let text = format!("{stdout}{stderr}");
    // Exit 2 = no smoke app for this framework (the harness's n/a), a
    // legitimate skip for a killed WP's framework.
    assert_ne!(
        out.status.code(),
        Some(2),
        "no smoke app for {fw} — the framework needs an app to be equivalent over"
    );
    if out.status.success() {
        assert!(
            text.contains(&format!("EQUIV {fw}: green")),
            "the green line must name the framework, got: {text}"
        );
    } else {
        panic!("equiv harness RED for {fw}:\n{text}");
    }
}
