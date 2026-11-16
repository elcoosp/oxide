//! V2-A01 phase 3: the dispatch stage table (spec WP-A01 "Step 3").
//!
//! The closure became: build [`RequestCtx`], then run the stages in order —
//! `stage_prelude -> stage_match -> stage_route -> stage_middleware ->
//! stage_data_route -> stage_action -> stage_l0_lookup -> stage_loaders ->
//! stage_endpoint -> stage_render`. The first nine short-circuit through
//! [`oxide_fw_dispatch::Flow`]; the last (the sketch's `stage_render` +
//! `stage_finish` pair) produces the final response.

use crate::*;

mod prelude;
pub(crate) use prelude::*;
mod matcher;
pub(crate) use matcher::*;
mod route;
pub(crate) use route::*;
mod middleware;
pub(crate) use middleware::*;
mod data_route;
pub(crate) use data_route::*;
mod action;
pub(crate) use action::*;
mod l0_lookup;
pub(crate) use l0_lookup::*;
mod loaders;
pub(crate) use loaders::*;
mod endpoint;
pub(crate) use endpoint::*;
mod render;
pub(crate) use render::*;

/// The stage table, in call order (the phase-3 runner the sketch describes:
/// `for stage in STAGES { if let Flow::Respond(r) = stage(d, &mut cx).await {
/// return r } }` — spelled out so the compiler checks the order and every
/// stage signature).
pub(crate) async fn run_stages(
    d: &DispatchDeps,
    cx: &mut RequestCtx,
) -> oxide_fw_server::OxideResponse {
    if let oxide_fw_dispatch::flow::Flow::Respond(r) = stage_prelude(d, cx).await {
        return r;
    }
    if let oxide_fw_dispatch::flow::Flow::Respond(r) = stage_match(d, cx).await {
        return r;
    }
    if let oxide_fw_dispatch::flow::Flow::Respond(r) = stage_route(d, cx).await {
        return r;
    }
    if let oxide_fw_dispatch::flow::Flow::Respond(r) = stage_middleware(d, cx).await {
        return r;
    }
    if let oxide_fw_dispatch::flow::Flow::Respond(r) = stage_data_route(d, cx).await {
        return r;
    }
    if let oxide_fw_dispatch::flow::Flow::Respond(r) = stage_action(d, cx).await {
        return r;
    }
    if let oxide_fw_dispatch::flow::Flow::Respond(r) = stage_l0_lookup(d, cx).await {
        return r;
    }
    if let oxide_fw_dispatch::flow::Flow::Respond(r) = stage_loaders(d, cx).await {
        return r;
    }
    if let oxide_fw_dispatch::flow::Flow::Respond(r) = stage_endpoint(d, cx).await {
        return r;
    }
    stage_render(d, cx).await
}
