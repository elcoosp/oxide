//! The CLI's serve-side modules — the skew/generation state
//! (`generations`) the start pipeline and the reload path share, and the
//! live-loader hub + rerun worker handle (`live`, §V2-F05) the dispatch arm
//! and the revalidation hooks share.

pub mod generations;
pub mod live;
