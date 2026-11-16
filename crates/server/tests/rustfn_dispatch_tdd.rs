//! TDD pins for the §V2-F08 dispatcher consult — the
//! `RustFirstDispatcher` the server-fn route drives: the native
//! registry owns its ids (the `*.server.rs` stub ids) and the JS pool
//! answers everything else.
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - A native hit answers from the native source and the JS dispatcher
//!   is never consulted (the "no JS hop" guarantee, decision D33).
//! - A native error on a native-owned id surfaces (the fn's own
//!   failure is not a reason to re-run it in JS).
//! - A native miss falls through to the JS pool (mixed projects:
//!   `.server.ts` and `.server.rs` fns on one build).
//! - No dispatcher at all answers the pipeline's `FN-NO-DISPATCHER`
//!   shape: 404 `FN-NOT-FOUND` (the route's pre-existing refusal).
//! - `NativeFnSource` is object-safe (`Arc<dyn NativeFnSource>`), the
//!   shape the CLI wires the loaded `RustFnHost` into.

use std::sync::Arc;

use oxide_fw_server::{DispatchError, NativeFnSource, RustFirstDispatcher, ServerFnDispatcher};

/// A fake native source: owns exactly the ids in its set.
struct FakeNative {
    ids: &'static [&'static str],
    /// The reply a hit produces (or the error to surface).
    reply: Result<Vec<u8>, DispatchError>,
    /// Records that `call` ran (the consult-order assertion).
    called: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl NativeFnSource for FakeNative {
    fn has(&self, id: &str) -> bool {
        self.ids.contains(&id)
    }
    async fn call(&self, _id: &str, _args: &[u8]) -> Result<Vec<u8>, DispatchError> {
        self.called.store(true, std::sync::atomic::Ordering::SeqCst);
        self.reply.clone()
    }
}

/// A fake JS pool dispatcher: panics the pin when consulted on a
/// native-owned id, answers `js:<id>` otherwise.
struct FakeJs {
    fail_on_call: bool,
}

#[async_trait::async_trait]
impl ServerFnDispatcher for FakeJs {
    async fn dispatch(&self, id: &str, _args: &[u8]) -> Result<Vec<u8>, DispatchError> {
        if self.fail_on_call {
            panic!("the JS pool must not be consulted for a native-owned id");
        }
        Ok(format!("js:{id}").into_bytes())
    }
}

fn native(ids: &'static [&'static str], reply: Result<Vec<u8>, DispatchError>) -> FakeNative {
    FakeNative {
        ids,
        reply,
        called: std::sync::atomic::AtomicBool::new(false),
    }
}

#[tokio::test]
async fn native_hit_answers_without_the_js_pool() {
    let n = native(&["native-id"], Ok(b"native-out".to_vec()));
    let js = FakeJs { fail_on_call: true };
    let d = RustFirstDispatcher::new(Some(Arc::new(n)), Some(Arc::new(js)));
    let out = d
        .dispatch("native-id", b"{}")
        .await
        .expect("the native source owns the id");
    assert_eq!(out, b"native-out");
}

#[tokio::test]
async fn native_error_on_a_native_owned_id_surfaces() {
    let n = native(
        &["native-id"],
        Err(DispatchError {
            status: 400,
            code: "FN-THREW".into(),
            message: "the width must be positive".into(),
        }),
    );
    let js = FakeJs { fail_on_call: true };
    let d = RustFirstDispatcher::new(Some(Arc::new(n)), Some(Arc::new(js)));
    let err = d
        .dispatch("native-id", b"{}")
        .await
        .expect_err("the fn error surfaces");
    assert_eq!(err.code, "FN-THREW");
    assert_eq!(err.status, 400);
}

#[tokio::test]
async fn native_miss_falls_through_to_the_js_pool() {
    let n = native(&["native-id"], Ok(Vec::new()));
    let js = FakeJs {
        fail_on_call: false,
    };
    let d = RustFirstDispatcher::new(Some(Arc::new(n)), Some(Arc::new(js)));
    let out = d
        .dispatch("js-id", b"{}")
        .await
        .expect("the JS pool owns this id");
    assert_eq!(out, b"js:js-id");
}

#[tokio::test]
async fn no_dispatcher_answers_not_found() {
    let d = RustFirstDispatcher::new(None, None);
    let err = d
        .dispatch("any", b"{}")
        .await
        .expect_err("nothing is registered");
    assert_eq!(err.status, 404);
    assert_eq!(err.code, "FN-NOT-FOUND");
}

#[tokio::test]
async fn js_only_wiring_still_answers() {
    let d = RustFirstDispatcher::new(
        None,
        Some(Arc::new(FakeJs {
            fail_on_call: false,
        })),
    );
    let out = d
        .dispatch("js-id", b"{}")
        .await
        .expect("the js pool answers");
    assert_eq!(out, b"js:js-id");
}

#[tokio::test]
async fn native_source_is_object_safe_behind_an_arc() {
    // The compile-time pin: `Arc<dyn NativeFnSource>` builds, so the
    // CLI can wire the loaded `RustFnHost` in without generics.
    let source: std::sync::Arc<dyn NativeFnSource> =
        Arc::new(native(&["native-id"], Ok(Vec::new())));
    assert!(source.has("native-id"));
}
