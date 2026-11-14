# 2026-11-14

## C05 — React Compiler (optional): SPIKED-KEEP, LANDED (WP V2-C05)

**Dependency arm (§0b).** `oxc_react_compiler 0.143.0`, **crates.io
latest** (published 2026-08-03 — fresh per §0b(1); ~106k downloads).

**The oxc skew — measured, not fatal.** The crate pins the oxc 0.143
stack while the workspace transform rides oxc 0.155. The seam is STRING
IN / STRING OUT (the C01 rule): the backend parses with the crate's own
0.143 parser, compiles on the pristine AST with
`SemanticBuilder::with_build_nodes(true)` (the crate's doc contract),
prints with the 0.143 codegen, and hands the text back to the normal
0.155 transform. No 0.143 type crosses into the workspace; the
`oxc_*_0_143` package aliases carry the second stack. The spec's "skip
if the oxc skew cannot link" criterion does NOT fire — it links (the
spike test runs), because nothing shares a type boundary.

**Spike pin (`crates/compiler/tests/react_compiler_spike.rs`).** A
stateful component (`useState`, string concat, arithmetic, JSX) runs
the full seam: parse → semantic → `compile` → print. The compile
succeeds with no fatal diagnostics; the rewrite output lands in the
printed module.

**Wiring.** `crates/compiler/src/native/react_compiler.rs`
(`ReactCompilerNative`, `react-compiler-native@0.143.0`, handles
`tsx`/`jsx`) behind `native-react-compiler` (forwarded
`oxide-fw-cli/native-react-compiler`). Applied BEFORE the JSX lowering
(string in, string out, then the normal transform). React's existing
native path is unchanged; this adds only the optional memoizer. The
registry pick for React tsx/jsx only fires when the feature is compiled
in — the default `auto` build keeps the current oxc transform for
every React file.
