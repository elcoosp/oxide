//! V2-F04: stage_experiments — the §15.4 A/B evaluation stage (the wiring
//! half of cache-safe personalization).
//!
//! Runs between the middleware chain and the data-route/action/L0 arms so
//! every downstream consumer sees the resolved state at once: the assigned
//! map rides the locals the loaders and the render input read (the
//! pipeline-owned `experiments` slot — a middleware that staged the same
//! key loses, per `RequestLocals`), the minted `Set-Cookie` headers ride
//! the per-request middleware-header list (applied on the L0-hit path AND
//! the render path, never stored in a cached entry — the store refuses a
//! response that sets a cookie, and `make_entry` strips the header as
//! belt-and-braces), the `|exp:` vary dimension joins the L0 key (only
//! when the suffix is non-empty, so routes outside every experiment keep
//! byte-identical keys to the pre-F04 baseline), and each assignment
//! counts one `oxide_experiment_assignment{experiment,variant}` exposure.
//!
//! The evaluation itself is the tested V8-free substrate
//! (`oxide_fw_dispatch::experiments` — pinned by
//! `crates/dispatch/tests/experiments_tdd.rs`); this stage is the
//! request-shaped wiring: config → engine conversion, the header/cookie/IP
//! probes, the mint (the SEC-09 CSPRNG token) and the four write-backs
//! above.

use crate::*;
use oxide_fw_dispatch::experiments::{self, Experiment, Sticky};
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
/// The config's `experiments` rows converted to the engine's shapes. The
/// config crate does not (and must not) depend on dispatch, so the mapping
/// lives here, field-by-field (the shapes are pinned by their own crates).
#[must_use]
pub(crate) fn experiments_of_config(from: &[oxide_fw_config::ExperimentConfig]) -> Vec<Experiment> {
    from.iter()
        .map(|e| Experiment {
            id: e.id.clone(),
            traffic: e.traffic,
            sticky: match &e.sticky {
                oxide_fw_config::StickyConfig::Cookie { cookie_name } => {
                    Sticky::Cookie(cookie_name.clone())
                }
                oxide_fw_config::StickyConfig::Header { header_name } => {
                    Sticky::Header(header_name.clone())
                }
                oxide_fw_config::StickyConfig::IpHash => Sticky::IpHash,
            },
            variants: e.variants.clone(),
            routes: e.routes.clone(),
        })
        .collect()
}

/// The L0 vary string with the `|exp:` dimension appended. The suffix joins
/// ONLY when non-empty (the acceptance: with no experiments configured, L0
/// keys stay byte-identical to the pre-F04 baseline — the empty arm returns
/// the input unchanged).
#[must_use]
pub(crate) fn vary_with_experiments(vary: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        return vary.to_string();
    }
    format!("{vary}|exp:{suffix}")
}

pub(crate) async fn stage_experiments(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    if d.experiments.is_empty() {
        return Flow::Continue;
    }
    // The §15.4 probes, read before the write-backs (the resolver's header
    // closure borrows the header list, so nothing re-enters `cx` while it
    // is held).
    let path = if cx.req_path.is_empty() {
        cx.raw.clone()
    } else {
        cx.req_path.clone()
    };
    let cookie_header = cx.req.header("cookie").map(str::to_string);
    let headers = std::sync::Arc::clone(&cx.req.headers);
    let header_lookup = move |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let ip = cx.req.peer.map(|p| p.to_string()).unwrap_or_default();
    // The §15.4 evaluation. Note the mint is NOT gated on assignment: a
    // cookie identity outside the traffic band is still minted (the
    // substrate's engine mints before the gate), so its `Set-Cookie` rides
    // the response and the next request resolves against the same value.
    let resolved = experiments::resolve(
        &d.experiments,
        &path,
        cookie_header.as_deref(),
        &header_lookup,
        &ip,
        &(|| oxide_fw_server::dev_token()),
    );
    if resolved.assigned.is_empty() && resolved.set_cookie.is_empty() {
        return Flow::Continue;
    }
    // The exposure metric (§V2-F04 item 4): one counter per assigned pair.
    let metrics = std::sync::Arc::clone(&d.metrics);
    for (id, variant) in &resolved.assigned {
        metrics.observe_experiment(id, variant);
    }
    // (1) The L0 vary dimension + the single-flight key it shapes (the key
    // must match the stored entry's key exactly — recompute from the
    // updated vary, the same `key_for` spelling stage_route used).
    if !resolved.vary_suffix.is_empty() {
        cx.l0_vary = vary_with_experiments(&cx.l0_vary, &resolved.vary_suffix);
        if let Some(c) = d.l0.as_ref() {
            cx.flight_key =
                l0_cache::L0Cache::key_for(&cx.l0_route, &cx.l0_params, &cx.l0_vary, c.build_id());
        }
    }
    // (2) The assigned map into the pipeline-owned locals slot — the
    // loaders (`LoaderRunInput.locals`) and the render input read it, a
    // middleware-staged `experiments` key loses at to-value time.
    if !resolved.assigned.is_empty()
        && let Some(obj) = cx.middleware_locals.as_object_mut()
    {
        obj.insert(
            "experiments".to_string(),
            serde_json::Value::Object(
                resolved
                    .assigned
                    .iter()
                    .map(|(id, v)| (id.clone(), serde_json::Value::String(v.clone())))
                    .collect(),
            ),
        );
    }
    // (3) The minted identities ride the per-request middleware-header
    // list: applied on the L0-hit and coalesced paths AND every render
    // response (P0-5 semantics), never stored — the L0 store refuses a
    // response that sets a cookie and `make_entry` strips the header.
    for cookie in &resolved.set_cookie {
        cx.middleware_headers
            .push(("set-cookie".to_string(), cookie.clone()));
    }
    cx.exp_resolved = Some(resolved);
    Flow::Continue
}
// <<< stage-wrapper

#[cfg(test)]
mod tests {
    use super::*;

    // The acceptance the engine pins only half of: the KEY dimension the
    // stage appends must not exist when no experiment assigned — the
    // pre-F04 byte shapes ride on.
    #[test]
    fn empty_suffix_keeps_the_vary_string_byte_identical() {
        assert_eq!(
            vary_with_experiments("accept-encoding", ""),
            "accept-encoding"
        );
        assert_eq!(vary_with_experiments("", ""), "");
    }

    #[test]
    fn nonempty_suffix_appends_the_exp_dimension() {
        assert_eq!(
            vary_with_experiments("accept-encoding", "hero=control"),
            "accept-encoding|exp:hero=control"
        );
        assert_eq!(
            vary_with_experiments("", "hero=control"),
            "|exp:hero=control"
        );
    }

    // The wiring conversion: the config shapes map field-by-field onto the
    // engine's (the config crate must not depend on dispatch).
    #[test]
    fn config_experiments_convert_to_engine_shapes() {
        let cfg = vec![oxide_fw_config::ExperimentConfig {
            id: "hero".to_string(),
            traffic: 60,
            sticky: oxide_fw_config::StickyConfig::Cookie {
                cookie_name: "oxide_hero".to_string(),
            },
            variants: vec!["a".to_string(), "b".to_string()],
            routes: vec!["/blog".to_string()],
        }];
        let engine = experiments_of_config(&cfg);
        assert_eq!(engine.len(), 1);
        assert_eq!(engine[0].id, "hero");
        assert_eq!(engine[0].traffic, 60);
        assert_eq!(engine[0].sticky, Sticky::Cookie("oxide_hero".to_string()));
        assert_eq!(engine[0].variants, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(engine[0].routes, vec!["/blog".to_string()]);
    }
}
