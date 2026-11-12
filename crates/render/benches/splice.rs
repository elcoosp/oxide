//! Criterion bench for the §V2-B08 phase-2 streaming `NonceSplicer` —
//! the splicer's chunked cost against the whole-document `str::replace`
//! the CLI still runs (the B08 evidence rows).

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_render::splice::NonceSplicer;

/// A 1 KiB HTML body with two nonces baked in (a realistic page slice).
fn page_1k() -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(1024);
    for i in 0..14 {
        let _ = writeln!(
            s,
            "<div class=\"row-{i}\"><script nonce=\"NNNN\">x{i}</script><b nonce=\"NNNN\">t{i}</b></div>"
        );
    }
    s
}

fn bench_splice_vs_replace(c: &mut Criterion) {
    let page = page_1k();
    let mut group = c.benchmark_group("splice");
    // Body sizes in KiB: the B08 acceptance shape (200 KB page) plus
    // the small-shell and the 2 MB long-tail ends.
    for target in [20usize, 200, 2048] {
        let doc: String = page.repeat(target * 1024 / page.len());
        let bytes = doc.len();
        group.throughput(criterion::Throughput::Bytes(bytes as u64));

        // The B08 baseline: one whole-document copy per replace.
        group.bench_with_input(
            BenchmarkId::new("replace_full_copy", bytes),
            &doc,
            |b, doc| {
                b.iter(|| black_box(doc.replace("NNNN", "abcdefgh")));
            },
        );

        // The splicer: the same document fed in 64 KiB chunks (the live
        // tee's chunk shape), no second copy of the document.
        group.bench_with_input(
            BenchmarkId::new("splicer_chunked_64k", bytes),
            &doc,
            |b, doc| {
                b.iter(|| {
                    let mut s = NonceSplicer::new("NNNN", "abcdefgh");
                    let mut out = Vec::with_capacity(doc.len());
                    for chunk in doc.as_bytes().chunks(64 * 1024) {
                        s.push(black_box(chunk), &mut out);
                    }
                    s.finish(&mut out);
                    out
                });
            },
        );
    }
    group.finish();
}

criterion_group!(splice_benches, bench_splice_vs_replace);
criterion_main!(splice_benches);
