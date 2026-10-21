# Oxide v0.1 Spec-Compliance Scorecard — r8

**HEAD:** `fbfc794` · **Range audited:** `ad8f77d..fbfc794` (**72 commits**)
**Overall:** ~92% (r7 ~91% → Δ+1)
**Method:** 4 parallel read-only verification agents; TS suites + conformance executed (362 tests / 351 pass / 11 fail at time of audit); Rust tests statically counted (1,133, 50 ignored); Playwright claim-verified only; git-tracked LOC ≈109k.

**Post-audit corrections (this push):** the 11 red TS tests are green (r8
finding 1), the create-oxide scaffolder emits the current config shape
(finding 2), PPR refuses to cache cookie-setting responses (P0.3), and
the schema carries `server.h3Advertise` + `framework.islands` (finding 9).
Remaining open findings — P0-1's `edge_bundle.js`, P0-8's L2 sealing,
P0-2's `/__data` unbounded collect, the Playwright CI gap, and the
bench-report p99 framing — are tracked in the r8 audit output and fixed
in follow-up commits on the same push.

The full r8 audit lives in the repository's PR history; this file is the
canonical citation anchor.
