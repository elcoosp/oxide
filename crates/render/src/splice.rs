//! The §V2-B08 phase-2 streaming nonce splicer: replace `from` with `to`
//! across arbitrarily chunked bytes without ever holding a second copy of
//! the document.
//!
//! # The seam
//!
//! The CLI's nonce handling still does `html.replace(&nonce, SENTINEL)` —
//! one whole-document copy per prerendered page and again per L0 fill
//! (spec §V2-B08's evidence rows). [`NonceSplicer`] is the chunk-safe
//! replacement the live path and the L0 fill adopt: `push` feeds one
//! chunk, splices every complete `from` occurrence, and holds back the
//! last `from.len() - 1` bytes so a needle split across two chunks is
//! still found; `finish` flushes the carry. The fill side runs
//! `NonceSplicer::new(&nonce, NONCE_SENTINEL)`, the cache-hit side the
//! inverse — the CLI adoption (deleting the `.replace` calls, the dhat
//! acceptance) stays the A01-phase-3 slice (see `docs/deviations.md`).
//!
//! The struct owns its needle (`Finder<'static>` + `from` bytes), so a
//! splicer is `Send` and can ride the tee task.

pub struct NonceSplicer {
    from: Vec<u8>,
    to: Vec<u8>,
    finder: memchr::memmem::Finder<'static>,
    carry: Vec<u8>,
}

impl NonceSplicer {
    /// A splicer that rewrites `from` to `to` across the chunks pushed
    /// into it.
    #[must_use]
    pub fn new(from: &str, to: &str) -> Self {
        let f = from.as_bytes().to_vec();
        Self {
            finder: memchr::memmem::Finder::new(&f).into_owned(),
            from: f,
            to: to.as_bytes().to_vec(),
            carry: Vec::new(),
        }
    }

    /// Append the spliced form of `chunk` to `out`. Holds back the last
    /// `from.len() - 1` bytes so a match split across two chunks is still
    /// found.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<u8>) {
        self.carry.extend_from_slice(chunk);
        let n = self.from.len();
        let mut i = 0;
        while let Some(p) = self.finder.find(&self.carry[i..]) {
            out.extend_from_slice(&self.carry[i..i + p]);
            out.extend_from_slice(&self.to);
            i += p + n;
        }
        let keep_from = self.carry.len().saturating_sub(n.saturating_sub(1)).max(i);
        out.extend_from_slice(&self.carry[i..keep_from]);
        let tail = self.carry[keep_from..].to_vec();
        self.carry = tail;
    }

    /// Flush the held-back bytes: the caller has finished the document.
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.carry);
        self.carry.clear();
    }
}

// The spec-verbatim tests (§V2-B08's test set), plus the chunk-boundary
// pins the live tee needs.

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> Vec<u8> {
        let mut s = NonceSplicer::new("NNNN", "abcdefgh");
        let mut out = Vec::new();
        for c in chunks {
            s.push(c, &mut out);
        }
        s.finish(&mut out);
        out
    }

    #[test]
    fn splits_at_every_position() {
        let doc = b"<script nonce='NNNN'>x</script><b nonce='NNNN'>";
        let want = String::from_utf8_lossy(doc).replace("NNNN", "abcdefgh");
        for cut in 0..doc.len() {
            assert_eq!(
                String::from_utf8(run(&[&doc[..cut], &doc[cut..]])).unwrap(),
                want,
                "cut {cut}"
            );
        }
    }

    #[test]
    fn no_match_passthrough() {
        assert_eq!(run(&[b"hello ", b"world"]), b"hello world");
    }

    #[test]
    fn byte_at_a_time() {
        let doc = b"aNNNNbNNNNc";
        let parts: Vec<&[u8]> = doc.chunks(1).collect();
        assert_eq!(run(&parts), b"aabcdefghbabcdefghc");
    }

    #[test]
    fn empty_input_passes_through_and_finish_flushes_the_carry() {
        let mut s = NonceSplicer::new("NNNN", "abcdefgh");
        let mut out = Vec::new();
        s.push(b"", &mut out);
        assert!(out.is_empty(), "an empty chunk emits nothing early");
        s.push(b"tail no needle", &mut out);
        s.finish(&mut out);
        assert_eq!(out, b"tail no needle");
    }

    #[test]
    fn finish_is_idempotent_and_reusable_after_finish() {
        let mut s = NonceSplicer::new("NNNN", "abcdefgh");
        let mut out = Vec::new();
        s.push(b"abNNNNcd", &mut out);
        s.finish(&mut out);
        s.finish(&mut out);
        assert_eq!(out, b"ababcdefghcd");
    }

    #[test]
    fn adjacent_matches_splice_without_gaps() {
        // Two needles back to back: the splice must not eat the second
        // one's prefix (the `i += p + n` walk).
        assert_eq!(run(&[b"NNNNNNNN"]), b"abcdefghabcdefgh");
        assert_eq!(run(&[b"NN", b"NN", b"NNN", b"N"]), b"abcdefghabcdefgh");
    }

    #[test]
    fn longer_needle_than_chunk_still_splices() {
        // The needle is longer than the chunks around the boundary — the
        // carry holds as many bytes as the needle needs, not one chunk.
        let mut s = NonceSplicer::new("0123456789", "SPICED");
        let mut out = Vec::new();
        for b in b"start 0123456789 end" {
            s.push(&[*b], &mut out);
        }
        s.finish(&mut out);
        assert_eq!(&out, b"start SPICED end");
        // `start ` and ` end` pass through around the spliced needle.
    }
}
