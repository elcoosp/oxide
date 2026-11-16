//! V2-A01 module root.

mod entry;
pub(crate) use entry::*;
// V2-A01 phase 3: the stage protocol over DispatchDeps + RequestCtx.
mod deps;
pub(crate) use deps::*;
mod ctx;
pub(crate) use ctx::*;
pub(crate) mod stages;
