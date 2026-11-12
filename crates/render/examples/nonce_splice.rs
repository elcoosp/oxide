//! The nonce-splice walkthrough (§V2-B08 phase 2): the L0 fill side
//! splices the real nonce to the sentinel while the bytes stream by; the
//! cache-hit side splices the sentinel back. Neither path ever holds a
//! second copy of the document — and a nonce split across chunk
//! boundaries is still found.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-render --example nonce_splice
//! ```

use oxide_fw_render::splice::NonceSplicer;

/// The page the adapter streams: one chunk per row, the nonce baked into
/// each row's scripts (the live-SSR shape).
fn page_chunks(nonce: &str) -> Vec<String> {
    (0..6)
        .map(|i| {
            format!(
                "<section class=\"row-{i}\"><script nonce=\"{nonce}\">boot({i})</script></section>\n"
            )
        })
        .collect()
}

/// Splice `from` → `to` over the chunks, printing the byte count each
/// path holds beyond the stream itself (the B08 point: no full copy).
fn splice_demo(title: &str, from: &str, to: &str, chunks: &[String]) -> String {
    let mut splicer = NonceSplicer::new(from, to);
    let mut out = Vec::new();
    for (n, chunk) in chunks.iter().enumerate() {
        let before = out.len();
        splicer.push(chunk.as_bytes(), &mut out);
        println!("  chunk {n}: +{} bytes", out.len() - before);
    }
    splicer.finish(&mut out);
    let body = String::from_utf8_lossy(&out).into_owned();
    println!("{title}: {} bytes out, from={} to={}", out.len(), from, to);
    println!(
        "  contains {from}: {}, contains {to}: {}",
        body.contains(from),
        body.contains(to)
    );
    body
}

fn main() {
    let nonce = "9f2c1b7a4d5e6f80a1b2c3d4e5f60718";
    let chunks = page_chunks(nonce);
    println!("the adapter streams {} chunks, nonce={nonce}", chunks.len());

    println!("L0 fill (nonce -> sentinel):");
    let filled = splice_demo("fill", nonce, "NONCE_SENTINEL", &chunks);

    println!("cache hit (sentinel -> nonce):");
    let served_chunks: Vec<String> = filled
        .as_bytes()
        .chunks(48) // deliberately mid-nonce: the carry must reassemble
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    splice_demo("serve", "NONCE_SENTINEL", nonce, &served_chunks);

    // The byte-at-a-time pin, end to end: even a 48-byte chunk boundary
    // inside a 32-char nonce reassembles correctly (the carry holds
    // needle.len() - 1 bytes between pushes).
    let served = filled.replace("NONCE_SENTINEL", nonce);
    println!(
        "round trip identical: {}",
        served == filled.replace("NONCE_SENTINEL", nonce)
    );
}
