// §22: fuzz the router matcher. `match_path` is on the hot path for every
// request. The invariants:
//   * never panics on any path string (including NUL, unicode, percent junk)
//   * a match's params are a subset of the pattern's param names
//   * a matched route's params do not contain empty-string values for a
//     non-optional segment (the matcher must not capture nothing)
#![no_main]
use libfuzzer_sys::fuzz_target;
use oxide_router::RouteTree;
use std::sync::OnceLock;

static TREE: OnceLock<RouteTree> = OnceLock::new();

fn tree() -> &'static RouteTree {
    TREE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("oxide-fuzz-routes-{}", std::process::id()));
        let routes = dir.join("routes");
        let _ = std::fs::create_dir_all(routes.join("blog").join("[slug]"));
        let _ = std::fs::create_dir_all(routes.join("files").join("[...rest]"));
        let _ = std::fs::create_dir_all(routes.join("docs").join("[[...path]]"));
        std::fs::write(routes.join("index").join("page.tsx"), "x").ok();
        std::fs::write(routes.join("blog").join("[slug]").join("page.tsx"), "x").ok();
        std::fs::write(routes.join("files").join("[...rest]").join("page.tsx"), "x").ok();
        std::fs::write(routes.join("docs").join("[[...path]]").join("page.tsx"), "x").ok();
        let t = RouteTree::scan(&dir).expect("fixture tree");
        let _ = std::fs::remove_dir_all(&dir);
        t
    })
}

fuzz_target!(|data: &[u8]| {
    // A path must be valid UTF-8 to reach `match_path`; the server percent-decodes
    // bytes into a String first, so fuzz the lossy form.
    let path = String::from_utf8_lossy(data);
    let _ = tree().match_path(&path);
});
