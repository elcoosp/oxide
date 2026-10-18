// §22: fuzz the ODF decoder. The decoder is the primary untrusted-input surface
// — every `/__data` response, every island prop payload, and every cached L1
// entry crosses it. The invariants:
//   * never panics
//   * never allocates unboundedly (MAX_BYTES/MAX_DEPTH/MAX_REFS enforced)
//   * a well-formed payload round-trips (re-encode → decode → equal)
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Any input must decode-or-error, never panic.
    let _ = oxide_odf::decode(data);

    // A successful decode must survive a re-encode round-trip.
    if let Ok(v) = oxide_odf::decode(data) {
        if let Ok(bytes) = oxide_odf::encode(&v) {
            let back = oxide_odf::decode(&bytes).expect("re-encoded payload must decode");
            assert_eq!(v, back, "ODF round-trip must be lossless");
        }
    }
});
