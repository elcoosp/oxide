//! The F08 fixture user crate (spec: "Build a fixture crate in the
//! test, built with `cargo build` at test time"): the shape a user's
//! `src/*.server.rs` cdylib compiles to — `#[server_fn]` exports over
//! the C ABI, the `export_table!` crate exports, and the types side
//! export. Its own workspace root (never a workspace member), so the
//! quick profile pays nothing for it.

use oxide_rustfn::prelude::*;

/// The spec's example shape (`thumb.server.rs`): typed in, typed out,
/// no JS involved.
#[derive(serde::Deserialize, TsType)]
pub struct FixtureIn {
    pub url: String,
    pub width: u32,
}

#[derive(serde::Serialize, TsType)]
pub struct FixtureOut {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// The happy path: echo the width as `width` zero bytes.
#[server_fn]
pub fn thumbnail(input: FixtureIn) -> Result<FixtureOut, ServerError> {
    Ok(FixtureOut {
        bytes: vec![0u8; input.width as usize],
        mime: "image/png".into(),
    })
}

/// The error path: a `ServerError` rides the boundary as status 1.
#[server_fn]
pub fn boom(_input: FixtureIn) -> Result<FixtureOut, ServerError> {
    Err(ServerError::new("boom: the fixture refuses"))
}

oxide_rustfn::export_table!(thumbnail, boom);
