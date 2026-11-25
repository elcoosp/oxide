//! The CLI's serve-side modules — the skew/generation state
//! (`generations`) the start pipeline and the reload path share, the
//! live-loader hub + rerun worker handle (`live`, §V2-F05) the dispatch arm
//! and the revalidation hooks share, the resumable-stream state
//! (`streams`, §V2-F07) the tee and the reattach route share, the brownout
//! state (`brownout`, §V2-F12) the sampler, the enforcement stage and the
//! readiness flip share, and the deployed-release boot seam (`releases`,
//! §V2-F11's store layout consumed by §V2-F10's serve boot) the start
//! command's `--releases` leg drives.

pub mod brownout;
pub mod generations;
pub mod live;
pub mod releases;
pub mod streams;

mod session;
pub(crate) use session::*;
mod start;
pub use start::*;
mod stream;
pub use stream::*;
