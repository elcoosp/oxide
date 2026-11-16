//! V2-A01 module root.

mod assets;
pub(crate) use assets::*;
mod config_bundle;
pub(crate) use config_bundle::*;
mod edge_wasm;
pub(crate) use edge_wasm::*;
mod fonts;
pub(crate) use fonts::*;
mod i18n;
pub(crate) use i18n::*;
mod pwa;
pub(crate) use pwa::*;
mod search;
pub(crate) use search::*;
mod sitemap;
pub(crate) use sitemap::*;
mod snapshot;
pub(crate) use snapshot::*;
