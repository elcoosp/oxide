//! The dispatch stage protocol (spec v0.2, V2-A02 / V2-A01 phase 3).
//!
//! Phase 3 turns the 1,400-line `build_dispatch` closure into stage
//! functions `async fn stage_x(d: &DispatchDeps, cx: &mut RequestCtx) ->
//! Flow`. This module owns the `Flow` return type so the stages can be
//! unit-tested against fakes inside this V8-free crate; the deps/ctx structs
//! land with the phase-3 move (they reference CLI-private state today).

use oxide_fw_server::OxideResponse;

/// What a dispatch stage wants after it runs: keep going down the stage
/// list, or short-circuit with a ready response.
pub enum Flow {
    /// Hand control to the next stage.
    Continue,
    /// Short-circuit: the response is final (redirect, cached L0 hit,
    /// proxy answer, error, …) and later stages must not run.
    Respond(OxideResponse),
}

#[cfg(test)]
mod tests {
    use super::Flow;
    use oxide_fw_server::OxideResponse;

    /// A minimal stage-shaped function: proves the protocol signature the
    /// phase-3 stages will use is expressible V8-free.
    fn fake_stage(respond: bool) -> Flow {
        if respond {
            Flow::Respond(OxideResponse::new(302, String::new()))
        } else {
            Flow::Continue
        }
    }

    #[test]
    fn respond_carries_the_final_response() {
        let flow = fake_stage(true);
        match flow {
            Flow::Respond(resp) => {
                assert_eq!(resp.status, 302);
                assert!(resp.body.is_empty());
                assert!(resp.stream.is_none());
            }
            Flow::Continue => panic!("stage should have short-circuited"),
        }
    }

    #[test]
    fn continue_falls_through_to_the_next_stage() {
        let flow = fake_stage(false);
        match flow {
            Flow::Continue => {}
            Flow::Respond(_) => panic!("stage should have continued"),
        }
    }

    #[test]
    fn stages_compose_in_sequence_until_one_responds() {
        // The closure's future shape: run stages in order, take the first
        // `Respond`, else fall through with `Continue`.
        let stages: [fn() -> Flow; 3] = [
            || fake_stage(false),
            || Flow::Continue,
            || Flow::Respond(OxideResponse::new(200, "ssr")),
        ];
        let mut final_status = 0;
        for stage in stages {
            if let Flow::Respond(resp) = stage() {
                final_status = resp.status;
                break;
            }
        }
        assert_eq!(final_status, 200);
    }
}
