# Oxide v0.1 Spec-Compliance Scorecard — r8

**HEAD:** `fbfc794` · **Range audited:** `ad8f77d..fbfc794` (**72 commits**)
**Overall:** ~92% (r7 ~91% → Δ+1)
**Method:** 4 parallel read-only verification agents; TS suites + conformance executed (362 tests / 351 pass / 11 fail at time of audit); Rust tests statically counted (1,133, 50 ignored); Playwright claim-verified only; git-tracked LOC ≈109k.

**Post-audit corrections (landed across the r8 push):** the 11 red TS
tests are green (r8 finding 1), the create-oxide scaffolder emits the
current config shape (finding 2), PPR refuses to cache cookie-setting
responses (P0.3), the schema carries `server.h3Advertise` +
`framework.islands` (finding 9), P0-8's L2 sealing is closed, the
Playwright CI job exists (finding 4), and the bench report's p99 framing
was corrected (finding 5, half — the retracted paragraph itself is
fixed in r9's WP-6).

**Still open at r8:** P0-1 (`edge_bundle.js` — unchanged), P0-2
(`/__data` unbounded `collect` — unchanged), P0-4 (PPR's runtime-shell
cache; the cookie guard landed, the build-time shell and rebuild
invalidation did not), P0-10's residues (`oxide render` attaches no
ComponentSource, `adapter_entry` is unread, the `ssr:'real'` default is
undecided).

The full r8 audit was not committed as a separate file; this summary is
the citation anchor. The r9 audit (in `docs/scorecard-r9.md`) is the
current live document.
