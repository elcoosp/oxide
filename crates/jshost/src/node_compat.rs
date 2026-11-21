//! The Node-backend selection classifier, V8-free (`node_compat`).
//!
//! SPEC-02 / §4.6.3's `requires_node_backend` was born inside `v8_loader`
//! (born with the V8 loader it fences) but it is a PURE specifier
//! classifier — no V8 API, no loader state — and the CLIENT bundle path
//! (`cli`'s `bundle_client`) consults it in a `--no-default-features`
//! build, where `v8_loader` does not compile. The pure half lives here;
//! `v8_loader` re-exports it so the v8-gated callers and unit pins keep
//! their path.

/// SPEC-02 / §4.6.3: does this module specifier require the Node backend?
///
/// - a `.node` native addon — V8 cannot load it at all;
/// - a `node:` builtin outside the shim set (`fs`, `crypto`, `child_process`,
///   `net`, `worker_threads`, …).
///
/// When true and `server.backend` is `auto`/`v8`, the build switches to Node
/// and prints why.
#[must_use]
pub fn requires_node_backend(specifier: &str) -> Option<String> {
    if std::path::Path::new(specifier)
        .extension()
        .is_some_and(|e| e == "node")
    {
        return Some(format!("native addon `{specifier}`"));
    }
    if let Some(name) = specifier.strip_prefix("node:") {
        let shimmed = matches!(
            name,
            "module"
                | "path"
                | "url"
                | "buffer"
                | "events"
                | "util"
                | "stream"
                | "crypto"
                | "fs"
                | "fs/promises"
        );
        if !shimmed {
            return Some(format!("node:{name} builtin"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::requires_node_backend;

    /// The classifier is reachable V8-free: every shimmed builtin stays
    /// silent, an unshimmed builtin and a native addon name themselves.
    #[test]
    fn requires_node_backend_classifies_v8_free() {
        for name in [
            "module",
            "path",
            "url",
            "buffer",
            "events",
            "util",
            "stream",
            "crypto",
            "fs",
            "fs/promises",
        ] {
            assert!(
                requires_node_backend(&format!("node:{name}")).is_none(),
                "shimmed builtin must not require the Node backend: {name}"
            );
        }
        assert_eq!(
            requires_node_backend("node:child_process"),
            Some("node:child_process builtin".to_string())
        );
        assert_eq!(
            requires_node_backend("better_sqlite3.node"),
            Some("native addon `better_sqlite3.node`".to_string())
        );
        assert!(requires_node_backend("./local.js").is_none());
    }
}
