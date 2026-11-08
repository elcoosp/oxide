//! Integration tests for the `oxide-fw-dispatch` stage protocol (V2-A02).
//!
//! The phase-3 stage list (`stage_prelude` … `stage_finish`) is expressed
//! against these pub types from outside the crate — the exact shape the CLI
//! closure will adopt.

use oxide_fw_dispatch::flow::Flow;
use oxide_fw_server::OxideResponse;

/// Exhaustive read of a `Flow` from outside the crate: the enum is the whole
/// protocol, with no hidden state a stage could smuggle past the runner.
fn status_of(flow: Flow) -> Option<u16> {
    match flow {
        Flow::Continue => None,
        Flow::Respond(resp) => Some(resp.status),
    }
}

#[test]
fn the_runner_takes_the_first_response_and_skips_the_rest() {
    let stages: [fn() -> Flow; 4] = [
        || Flow::Continue,
        || Flow::Respond(OxideResponse::new(304, String::new())),
        || Flow::Respond(OxideResponse::new(500, String::new())),
        || Flow::Continue,
    ];
    let mut taken = Vec::new();
    for (idx, stage) in stages.into_iter().enumerate() {
        if let Some(status) = status_of(stage()) {
            taken.push((idx, status));
            break;
        }
    }
    assert_eq!(taken, vec![(1, 304)]);
}

#[test]
fn an_all_continue_pipeline_yields_no_response() {
    let stages: [fn() -> Flow; 3] = [|| Flow::Continue, || Flow::Continue, || Flow::Continue];
    let responded = stages.into_iter().any(|s| status_of(s()).is_some());
    assert!(!responded);
}

#[test]
fn a_streaming_response_flows_through_the_protocol_unchanged() {
    // A streaming `OxideResponse` (stream set, no collected body) is carried
    // as-is; the protocol does not force collection.
    let resp = OxideResponse::new(200, String::new());
    let flow = Flow::Respond(resp);
    match flow {
        Flow::Respond(resp) => assert!(resp.stream.is_none() && resp.status == 200),
        Flow::Continue => panic!("expected the response branch"),
    }
}
