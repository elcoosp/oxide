# 2026-11-14

## C10 — MDX native evaluation: SPIKED-KILLED (WP V2-C10)

**Decision.** Keep the current MDX pipeline (the `tooling:module`
`@mdx-js/mdx` pool compile in `crates/cli/src/precompile.rs`; comrak
stays for markdown content collections) and close the WP.

**The decisive question — adapter neutrality — fails.** The candidate
`snapfire_fsr_mdx 0.2.3` (crates.io 2026-11-09 — fresh; pins oxc 0.155,
matching the workspace, so the skew cost is zero) emits a **JSX/TSX
module** (`Converted::tsx`). The probe
(`crates/assets/tests/mdx_native_spike.rs`, green) pins exactly that:
the output is a jsx-runtime-shaped module with `Target::{Lowered,
Browser}` and `ClassAttr::{ClassName, Class}` knobs. JSX is the adapter
boundary itself — only React (and Solid, via `babel-preset-solid`) can
consume it. The Vue and Svelte adapters compile SFCs through their own
runtime helpers and have **no JSX runtime at all** — the spec's
condition (a) ("works for all four adapters") cannot be satisfied by
this candidate or by any MDX-to-JSX design.

**The other conditions (moot but recorded).**
- (b) A second JS parser: the report named `swc`; the actual candidate
  uses oxc 0.155 (the workspace's own stack) — the binary-size budget
  impact would have been ~zero. Not the deciding factor.
- (c) remark/rehype plugin story: the candidate has no remark/rehype
  plugin surface (its `Options` carry `gfm`, `heading_ids`,
  `frontmatter`, `highlight`, `mapped`, `provider`); the @mdx-js path
  keeps the JS plugin ecosystem. Documented as a gap, not the kill.

**Consequence.** No runtime dependency lands (the `snapfire_fsr_mdx`
dev-dependency exists solely so the spike test can re-run after a dep
bump — `cargo test -p oxide-fw-assets --test mdx_native_spike -- --ignored --nocapture`).
No feature flag, no `crates/assets/src/mdx.rs`, no code change.
