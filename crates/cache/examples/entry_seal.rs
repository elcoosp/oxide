//! The entry seal's trust boundary (§11.3 / P0-8), walked: why a forged
//! L2 payload is a MISS, never a hit.
//!
//! An L2 payload is untrusted transport: anyone with write access to a
//! shared store (Redis, S3, the plane's wire, the disk file) could
//! otherwise mint `Entry { body: "<script>…", headers: […] }` and serve
//! stored XSS or header injection. [`oxide_fw_cache::seal_entry`] prefixes
//! every entry with a 32-byte keyed BLAKE3 MAC before it leaves a trusted
//! tier; [`oxide_fw_cache::open_entry`] verifies it before the payload is
//! served — a MAC failure is a miss (never served, never an error). Both
//! wire halves ride it: the `HttpCacheHandler` client seals every PUT and
//! refuses a payload that does not open, and the plane's shell refuses a
//! forged or tampered PUT with a 400 that never stores. The key is read
//! from `OXIDE_CACHE_MAC_KEY` (hex or raw) when set — a multi-node
//! deployment MUST set it so sealed entries interoperate — and derived
//! per-process otherwise, so a single-process deployment still resists an
//! attacker who can only write.
//!
//! Run with: `cargo run -p oxide-fw-cache --example entry_seal`

use oxide_fw_cache::{MAC_LEN, open_entry, seal_entry};

fn main() {
    let payload = b"<html>trust-boundary-v1</html>";

    // 1. The seal: the envelope is MAC_LEN of keyed MAC, then the bytes.
    let sealed = seal_entry(payload);
    println!(
        "1. sealed: {} bytes = MAC_LEN ({}) + payload ({} bytes)",
        sealed.len(),
        MAC_LEN,
        payload.len()
    );

    // 2. The honest open: the round-trip the client's `get` rides on
    //    every hit.
    let opened = open_entry(&sealed);
    println!(
        "2. opened: {} — the sealed payload verifies",
        opened == Some(&payload[..])
    );

    // 3. The tampered body: one flipped byte in the payload — the MAC
    //    covers it, the comparison is constant-time, the answer is a
    //    miss (the client's `get` returns Ok(None), the plane's shell
    //    answers 400 and never stores).
    let mut tampered_body = sealed.clone();
    let last = tampered_body.len() - 1;
    tampered_body[last] ^= 0x01;
    println!(
        "3. tampered body: open_entry = {:?} — a miss, never a hit",
        open_entry(&tampered_body).is_none()
    );

    // 4. The tampered MAC: the attacker flips a byte in the MAC prefix
    //    itself — the same miss.
    let mut tampered_mac = sealed.clone();
    tampered_mac[0] ^= 0x01;
    println!(
        "4. tampered MAC:   open_entry = {:?} — the same miss",
        open_entry(&tampered_mac).is_none()
    );

    // 5. The forgery: the attacker with store-write access simply OMITS
    //    the MAC (the unsealed legacy shape) — still a miss. This is the
    //    pair the §11.2 audit found the HTTP client missing once: a
    //    forged entry was served to every reader; the seal on BOTH wire
    //    halves closed it.
    let forged: Vec<u8> = payload.to_vec();
    println!(
        "5. forged (unsealed): open_entry = {:?} — omitting the MAC mints nothing",
        open_entry(&forged).is_none()
    );

    // 6. The short input: an envelope shorter than the MAC cannot even
    //    carry one — a miss by construction.
    println!(
        "6. short input:      open_entry = {:?} — nothing shorter than MAC_LEN opens",
        open_entry(&[0u8; 8]).is_none()
    );

    // 7. The operator note: `OXIDE_CACHE_MAC_KEY` (hex or raw) makes the
    //    seal interoperable across nodes; unset, the key is minted
    //    per-process (single-process deployments keep the write-protection;
    //    multi-node MUST set the env var — see the module docs).
    println!(
        "7. multi-node rule: set OXIDE_CACHE_MAC_KEY so every node opens the \
         same sealed entries (unset here: {})",
        std::env::var("OXIDE_CACHE_MAC_KEY").is_err()
    );
}
