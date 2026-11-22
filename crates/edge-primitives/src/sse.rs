//! The SSE event frame (§V2-F05 step 3), extracted to its V8-free home.
//!
//! Moved verbatim from `oxide-fw-server::live::sse_frame`: the server crate
//! re-exports [`sse_frame`] from here so its behavior is byte-identical, and
//! the wasm32 edge build can frame an event without linking `tokio`/`hyper`.

use std::fmt::Write as _;

/// Frame one SSE event (§V2-F05 step 3). Multi-line data becomes one
/// `data:` line per input line; the event terminator is a blank line.
///
/// This is the exact function `oxide-fw-server::live` calls per data/ping
/// frame — moved here so `oxide-fw-core-wasm` can re-export it for the edge
/// Worker without pulling the server's V8 graph into the wasm build.
#[must_use]
pub fn sse_frame(event: &str, id: Option<usize>, data: &str) -> String {
    let mut s = String::new();
    // `write!`/`writeln!` into a `String` is infallible; the `Result` is
    // discarded.
    let _ = writeln!(s, "event: {event}");
    if let Some(i) = id {
        let _ = writeln!(s, "id: {i}");
    }
    for line in data.split('\n') {
        let _ = writeln!(s, "data: {line}");
    }
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::sse_frame;

    /// The spec's own pin (§V2-F05 tests: `sse_frame_multiline_data`):
    /// multi-line data becomes one `data:` line per input line.
    #[test]
    fn sse_frame_multiline_data() {
        assert_eq!(
            sse_frame("data", Some(7), "line1\nline2"),
            "event: data\nid: 7\ndata: line1\ndata: line2\n\n"
        );
    }

    /// A ping frame with no id and single-line data.
    #[test]
    fn sse_frame_ping() {
        assert_eq!(sse_frame("ping", None, "1"), "event: ping\ndata: 1\n\n");
    }

    /// A stale frame carrying a hex build id.
    #[test]
    fn sse_frame_stale() {
        assert_eq!(
            sse_frame("stale", None, "deadbeef00000001"),
            "event: stale\ndata: deadbeef00000001\n\n"
        );
    }
}
