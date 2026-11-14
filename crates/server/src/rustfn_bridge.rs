//! The §V2-F08 dispatcher consult — the seam between the HTTP
//! pipeline's server-fn route and the native `*.server.rs` host:
//!
//! > The server function dispatcher (`build_server_fn_dispatcher`)
//! > consults the Rust host registry before the JS pool.
//!
//! The consult is a first-match on the §9.4 id: a native-owned id
//! answers from the loaded `RustFnHost` (decision D33's "no JS hop" —
//! the fn's own failure is not a reason to re-run it in JS), a native
//! miss falls through to the JS pool (mixed projects: `.server.ts`
//! and `.server.rs` fns on one build), and no dispatcher at all keeps
//! the pipeline's pre-existing `FN-NO-DISPATCHER` 404. The CLI wires
//! the loaded host in behind [`NativeFnSource`]; tests drive fakes.

use std::sync::Arc;

use crate::{DispatchError, ServerFnDispatcher};

/// The native-function source the consult asks before the JS pool —
/// the shape the loaded `RustFnHost` adapter implements (the CLI
/// bridges its id → entry map onto `has`/`call`).
#[async_trait::async_trait]
pub trait NativeFnSource: Send + Sync {
    /// Whether the id is native-owned (a stub id the `.server.rs`
    /// build generated).
    fn has(&self, id: &str) -> bool;
    /// Run the native fn: the §9.4 route's args JSON in, the fn's
    /// encoded output out.
    ///
    /// # Errors
    ///
    /// The coded failure the pipeline maps into the 4xx/5xx envelope
    /// (the same [`DispatchError`] shapes the JS pool produces).
    async fn call(&self, id: &str, args_json: &[u8]) -> Result<Vec<u8>, DispatchError>;
}

/// The consult order the spec pins: the native registry first, the JS
/// pool second, and the not-found refusal the route already carries
/// when neither owns the id. Implements [`ServerFnDispatcher`], so
/// the pipeline keeps one dispatcher seam.
pub struct RustFirstDispatcher {
    native: Option<Arc<dyn NativeFnSource>>,
    js: Option<Arc<dyn ServerFnDispatcher>>,
}

impl RustFirstDispatcher {
    /// Wire the consult: both halves optional (a JS-only build passes
    /// `native: None`; a native-only one passes `js: None`).
    #[must_use]
    pub fn new(
        native: Option<Arc<dyn NativeFnSource>>,
        js: Option<Arc<dyn ServerFnDispatcher>>,
    ) -> Self {
        Self { native, js }
    }
}

#[async_trait::async_trait]
impl ServerFnDispatcher for RustFirstDispatcher {
    async fn dispatch(&self, id: &str, args_json: &[u8]) -> Result<Vec<u8>, DispatchError> {
        if let Some(native) = &self.native
            && native.has(id)
        {
            return native.call(id, args_json).await;
        }
        if let Some(js) = &self.js {
            return js.dispatch(id, args_json).await;
        }
        Err(DispatchError::not_found(id))
    }
}
