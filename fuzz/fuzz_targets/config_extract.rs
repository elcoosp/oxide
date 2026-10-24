// §22: fuzz the config static extractor. It parses user-authored
// `oxide.config.ts` with a hand-rolled scanner; malformed input must produce
// `None` or an error, never panic or loop.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let src = String::from_utf8_lossy(data);
    let _ = oxide_fw_config::extract_public(&src);
});
