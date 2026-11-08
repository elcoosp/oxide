//! The §8.5 middleware chain runner (spec v0.2, V2-A05 step 1).
//!
//! The dispatch closure runs the route's full ancestor `middleware.ts`
//! chain, outermost first (§8.5). The ~200-line loop that does this lives
//! inline in `build_dispatch` (`crates/cli/src/lib.rs`), behind a V8 link
//! its tests pay on every iteration. This module is the substrate the
//! phase-3 `stage_middleware` wires instead: one tested, V8-free
//! [`run_chain`] call with the exact semantics the loop ships today —
//!
//! - **Order**: outer to inner, one [`MiddlewareDispatcher`] call per
//!   layer; the walk stops at the first directive that ends it.
//! - **Locals**: each layer sees the locals merged so far (`None` while
//!   nothing has accumulated — the trait doc's contract), the merge is
//!   later-wins (an inner middleware refines an outer one), and the
//!   outcome carries the merged object for the downstream stages.
//! - **Headers**: accumulated across layers, last-writer-wins per
//!   case-insensitive name (the CLI's B-06 fix — appending both produced
//!   duplicate header lines).
//! - **Short-circuit / redirect**: a `{ short: true, status, redirect }`
//!   answer ends the walk; the raw [`MiddlewareRunResult`] rides out so
//!   the caller builds the response (location header, session write-back)
//!   exactly as the loop does.
//! - **Rewrite** (§8.5, B-22): ends the walk with the target URL, the
//!   merged locals and headers, and the rewriting layer's path as the
//!   resume marker — the caller re-dispatches the whole request and
//!   passes the marker back so the layers that already ran (and the
//!   rewriting layer itself) do not run twice. A marker naming a layer
//!   that is not in this route's chain is ignored and the chain runs
//!   from the top (the re-dispatch may have matched a shallower route).
//! - **Errors**: a dispatcher failure propagates verbatim; the walk stops.
//!
//! Pinned by `crates/dispatch/tests/chain_tdd.rs`. The CLI wiring is
//! deferred to the A01 phase-3 move (see `docs/deviations.md`).

use oxide_fw_server::page_renderer::MiddlewareDispatchArgs;
use oxide_fw_server::{MiddlewareDispatcher, MiddlewareRunError, MiddlewareRunResult};
use std::path::PathBuf;

/// The request-shaped part of a middleware dispatch: constant across the
/// chain, threaded verbatim into every layer's
/// [`MiddlewareDispatchArgs`] (the full request so an auth or upload
/// middleware sees what it needs).
#[derive(Debug)]
pub struct ChainRequest<'a> {
    /// HTTP method of the in-flight request.
    pub method: &'a str,
    /// The request URL (path + query), as the worker receives it.
    pub url: &'a str,
    /// The matched route's params JSON, built once in the prelude.
    pub params: &'a serde_json::Map<String, serde_json::Value>,
    /// The request headers, in arrival order.
    pub headers: &'a [(String, String)],
    /// The request body bytes (the §9.3 form pair reads these).
    pub body: &'a [u8],
}

/// What the chain wants after its walk: keep going with the merged state,
/// answer from a layer, or rewrite.
#[derive(Debug)]
pub enum ChainOutcome {
    /// Every layer passed: carry the merged locals and the accumulated
    /// response headers for the downstream stages.
    Continue {
        /// The single locals object loaders, actions and render receive.
        locals: serde_json::Value,
        /// Headers accumulated across the chain, last-writer-wins per name.
        headers: Vec<(String, String)>,
    },
    /// A middleware answered (short-circuit or redirect): the raw result
    /// the caller maps to a response (location header, session
    /// write-back) exactly as the inline loop does.
    Respond(MiddlewareRunResult),
    /// A middleware asked for a rewrite (§8.5): the caller re-dispatches
    /// the request to `to` through the normal match → loader → render
    /// path, and passes `resume_after` back so the layers that already
    /// ran — including the rewriting layer — do not run twice.
    Rewrite {
        /// The rewrite target URL.
        to: String,
        /// The locals merged so far, handed to the re-dispatch.
        locals: serde_json::Value,
        /// Headers accumulated before the rewrite.
        headers: Vec<(String, String)>,
        /// The rewriting layer's module path (the B-22 resume marker).
        resume_after: Option<String>,
    },
}

/// Merge one middleware's `locals` payload into the accumulated map:
/// object keys later-wins, non-object payloads contribute nothing
/// (P-08's by-value merge, without the per-hop deep clone).
pub fn merge_locals(
    into: &mut serde_json::Map<String, serde_json::Value>,
    from: serde_json::Value,
) {
    if let serde_json::Value::Object(source) = from {
        for (key, value) in source {
            into.insert(key, value);
        }
    }
}

/// Accumulate response headers last-writer-wins per case-insensitive name
/// (the CLI's B-06 semantics: an inner middleware refines what an outer
/// one set; appending both produced duplicate header lines).
pub fn merge_headers(into: &mut Vec<(String, String)>, add: &[(String, String)]) {
    for (name, value) in add {
        let lower = name.to_ascii_lowercase();
        match into
            .iter_mut()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(&lower))
        {
            Some((_, existing)) => existing.clone_from(value),
            None => into.push((name.clone(), value.clone())),
        }
    }
}

/// Run the ancestor middleware chain, outermost first (§8.5).
///
/// `resume_after` is the B-22 marker from a previous rewrite (the layer
/// that issued it): layers before it already ran in the previous pass and
/// the layer itself must not run twice, so the walk resumes after it. A
/// marker naming a layer absent from `chain` is ignored and the chain
/// runs from the top.
///
/// # Errors
///
/// A dispatcher failure propagates verbatim and ends the walk; the caller
/// maps it to the `MIDDLEWARE`-coded JSON envelope.
pub async fn run_chain(
    dispatcher: &dyn MiddlewareDispatcher,
    chain: &[PathBuf],
    request: ChainRequest<'_>,
    initial_locals: serde_json::Value,
    resume_after: Option<&str>,
) -> Result<ChainOutcome, MiddlewareRunError> {
    let mut locals = match initial_locals {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut pending_resume = resume_after
        .filter(|marker| chain.iter().any(|path| path.to_string_lossy() == *marker))
        .map(str::to_owned);
    for path in chain {
        let module_path = path.to_string_lossy().to_string();
        if let Some(marker) = pending_resume.as_deref()
            && marker != module_path
        {
            // Before the marker: this layer ran in the previous pass.
            continue;
        }
        if pending_resume.is_some() {
            // The marker layer itself must not run twice.
            pending_resume = None;
            continue;
        }
        // Pass the locals accumulated so far; None while nothing has
        // accumulated (the trait doc's contract).
        let locals_arg = if locals.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(locals.clone()))
        };
        let result = dispatcher
            .dispatch(MiddlewareDispatchArgs {
                module_path: &module_path,
                method: request.method,
                url: request.url,
                params: request.params,
                headers: request.headers,
                body: request.body,
                locals: locals_arg.as_ref(),
            })
            .await?;
        if result.short || result.redirect.is_some() {
            return Ok(ChainOutcome::Respond(result));
        }
        // P-08: consume the fields by value — no per-hop deep clone.
        let MiddlewareRunResult {
            headers: layer_headers,
            locals: layer_locals,
            rewrite,
            ..
        } = result;
        merge_headers(&mut headers, &layer_headers);
        if let Some(source) = layer_locals {
            merge_locals(&mut locals, source);
        }
        if let Some(to) = rewrite {
            return Ok(ChainOutcome::Rewrite {
                to,
                locals: serde_json::Value::Object(locals),
                headers,
                resume_after: Some(module_path),
            });
        }
    }
    Ok(ChainOutcome::Continue {
        locals: serde_json::Value::Object(locals),
        headers,
    })
}
