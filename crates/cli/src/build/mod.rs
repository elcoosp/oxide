//! V2-A01 module root.

mod cmd;
pub(crate) use cmd::*;
mod secret;
pub(crate) use secret::*;
mod splice_nonce;
pub(crate) use splice_nonce::*;
// V2-A01 phase 2: the build pipeline as named phases over BuildCtx.
mod ctx;
pub(crate) use ctx::*;
mod load;
pub(crate) use load::*;
mod plan;
pub(crate) use plan::*;
mod bundle_client;
pub(crate) use bundle_client::*;
mod bundle_server;
pub(crate) use bundle_server::*;
mod native;
mod prerender;
pub(crate) use native::*;
pub(crate) use prerender::*;
mod seed_l0;
pub(crate) use seed_l0::*;
// §10.4 / P0-4: the build-time PPR shell seed (the manifest phase + the
// one-definition-site contract the boot's read side parses). The module
// itself is `pub` so the lib root can re-export the seed contract's public
// surface (the bench + the walkthrough example ride it).
pub mod ppr_seed;
pub(crate) use ppr_seed::*;
mod typegen;
pub(crate) use typegen::*;
mod emit;
pub(crate) use emit::*;
// V2-F02: the auto-islands scan (the interactivity call-site the build
// runs when `framework.autoIslands` is on — scan → side-car → registry
// merge; see `docs/deviations.md` for the queued dev-warn half).
mod auto_islands;
pub(crate) use auto_islands::*;
