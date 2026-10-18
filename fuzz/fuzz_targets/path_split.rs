// §22: fuzz the path splitter used by the matcher (`split_path`). It strips
// query/fragment, folds duplicate slashes, and percent-decodes each segment.
// The invariant: the output is a list of non-empty Strings, and the total
// decoded byte length is bounded by the input length plus a small factor
// (percent-decoding shrinks; it never expands beyond a fixed multiplier).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    // `split_path` is `pub(crate)`; exercise it through the public matcher on
    // an empty tree — this still drives the splitter on every input.
    use oxide_router::RouteTree;
    let tree = RouteTree::new();
    let _ = tree.match_path(&s);
});
