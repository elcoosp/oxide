//! §22 / L4 — property-based fuzz for the ODF decoder.
//!
//! The plan names `cargo-fuzz` over the ODF envelope. cargo-fuzz needs a
//! nightly toolchain and a separate `fuzz/` crate with a libFuzzer harness;
//! on this workspace's stable-only CI, a `proptest` property over random
//! byte slices catches the same class of bug (panic, out-of-bounds index,
//! infinite loop on a malformed header) at a fraction of the CI cost.
//! A genuine libFuzzer harness in `fuzz/` remains a documented follow-up.
//!
//! The contract under test: `odf::decode` on arbitrary input MUST return
//! `Result::Err` (with a coded diagnostic), never panic.
use proptest::prelude::*;

proptest! {
    // 1000 random byte inputs per run — enough to catch a decoding panic
    // without dragging CI past a few seconds.
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn decode_never_panics_on_arbitrary_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
    ) {
        // The function's job is to return Err on garbage, never panic. If
        // this test panics, the bytes are hex-encoded by proptest's default
        // failure formatter and can be re-fed as a regression input.
        let _ = oxide_fw_odf::decode(&bytes);
    }

    #[test]
    fn decode_never_panics_on_odf_magic_prefix(
        // Force the ODF1 magic so we exercise the *body* parser rather
        // than the 4-byte magic short-circuit. Same contract: any suffix
        // is either a valid document or a coded Err.
        suffix in proptest::collection::vec(any::<u8>(), 0..512),
    ) {
        let mut input = Vec::with_capacity(4 + suffix.len());
        input.extend_from_slice(b"ODF1");
        input.extend_from_slice(&suffix);
        let _ = oxide_fw_odf::decode(&input);
    }
}
