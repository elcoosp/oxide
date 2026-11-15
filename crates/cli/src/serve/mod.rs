//! The CLI's serve-side modules — the skew/generation state
//! (`generations`) the start pipeline and the reload path share, the
//! live-loader hub + rerun worker handle (`live`, §V2-F05) the dispatch arm
//! and the revalidation hooks share, the resumable-stream state
//! (`streams`, §V2-F07) the tee and the reattach route share, and the
//! brownout state (`brownout`, §V2-F12) the sampler, the enforcement stage
//! and the readiness flip share.

pub mod brownout;
pub mod generations;
pub mod live;
pub mod streams;
