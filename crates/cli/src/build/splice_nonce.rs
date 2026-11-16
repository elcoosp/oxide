//! V2-B08 integration: the build-side nonce splice over the render crate's
//! chunk-safe [`NonceSplicer`] (spec §V2-B08 "Phase 2 (streaming splice)").
//!
//! The prerender pass rewrote the CSP nonce with whole-document
//! `str::replace` calls — one full copy of every HTML document per splice,
//! three splices per document on the seed path. [`NonceSplicer`] is the
//! sliding-window splicer the L0 hit path uses; driven with the whole
//! document as ONE chunk it allocates exactly one output buffer instead of
//! the `replace` copy, and it finds a needle split across chunk boundaries
//! (the carry holds the last `from.len() - 1` bytes). The B08 dhat
//! allocation budget rides the CLI adoption.

/// Splice every `from` occurrence in `html` to `to` through the
/// [`NonceSplicer`] (one chunk = the whole document). The nonce is ASCII hex
/// and the replacement is the ASCII sentinel, so a match can never split a
/// multi-byte UTF-8 sequence and the output is always valid UTF-8 — the
/// `expect` names that invariant rather than inventing a fallback the
/// caller cannot distinguish from the spliced bytes.
pub(crate) fn splice_nonce(html: &str, from: &str, to: &str) -> String {
    let mut splicer = oxide_fw_render::splice::NonceSplicer::new(from, to);
    let mut out = Vec::with_capacity(html.len());
    splicer.push(html.as_bytes(), &mut out);
    splicer.finish(&mut out);
    String::from_utf8(out).expect("nonce splice preserves UTF-8 (ascii needle + replacement)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    /// The single-shot splice is byte-identical to `str::replace` — the same
    /// contract the L0 hit path's chunked pins already carry, pinned here for
    /// the whole-document shape the prerender pass uses.
    #[test]
    fn splice_matches_str_replace() {
        let nonce = "nonce-abc123";
        let html = "<html><script nonce=\"nonce-abc123\"></script>\
                    <p>nonce-abc123</p><script nonce=\"nonce-abc123\"/></html>";
        assert_eq!(
            splice_nonce(html, nonce, l0_cache::NONCE_SENTINEL),
            html.replace(nonce, l0_cache::NONCE_SENTINEL)
        );
    }

    /// No occurrence: the document comes back unchanged (and no allocation
    /// panic on the empty-match edge `str::replace` handles).
    #[test]
    fn splice_without_needle_is_identity() {
        let html = "<html><body>plain</body></html>";
        assert_eq!(splice_nonce(html, "nonce-xyz", "SENTINEL"), html);
    }

    /// The needle at the very start and very end of the document (the carry
    /// window's boundary cases).
    #[test]
    fn splice_at_document_boundaries() {
        let nonce = "N";
        assert_eq!(splice_nonce("N", nonce, "S"), "S");
        assert_eq!(splice_nonce("xNx", nonce, "S"), "xSx");
        assert_eq!(splice_nonce("abN", nonce, "S"), "abS");
    }
}
