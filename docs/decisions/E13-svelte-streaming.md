# 2026-11-25

## E13 — Svelte streaming SSR: SPIKED-KILLED (WP V2-E13)

**Decision.** Keep `capabilities.suspense: 'core'` in
`packages/svelte/src/real-adapter.ts` and close the WP as killed. No
`renderStream` lands in `packages/svelte/src/real-adapter.ts`; the Svelte
adapter keeps the §7.1 core deferred-boundary protocol (the `<Defer>`
component + the core swapper) for streaming and deferred data.

**The version checked.** `svelte@5.57.3` — the pnpm-lock resolution of
`packages/svelte/package.json`'s `svelte: ^5.57.3` dev-dependency (the
adapter's peer range is `svelte@^5.0.0`, so a consumer on any 5.x gets the
same verdict below until Svelte 6).

**What the spike checked.** The spec asks whether the pinned Svelte 5
release "expose[s] async or streaming server rendering (check the release
notes and `svelte/server` exports)". Checked, in order:

1. `svelte/server`'s exports at the pin — `render(Component, options)`
   returning `{ head, body, ids }` synchronously (the shape
   `packages/svelte/src/real-ssr.ts` already consumes). No streaming export
   exists: there is no `pipe`/chunked-write surface, no
   `renderStream`, nothing that yields partial HTML.
2. Async SSR — Svelte 5.36 introduced `await` in components and async
   server rendering through the SAME `render(...)` API: "Svelte supports
   asynchronous server-side rendering (SSR) with the render(...) API. To
   use it, simply await the return value"
   ([svelte.dev/docs/svelte/await-expressions](https://svelte.dev/docs/svelte/await-expressions)).
   The experimental SSR path shipped in v5.39.3 behind the
   `experimental.async` compile option
   ([svelte.dev/blog/whats-new-in-svelte-october-2025](https://svelte.dev/blog/whats-new-in-svelte-october-2025)).
3. The kill criterion — the spec's own: "the API is flagged experimental
   and unstable for the pinned version". The docs state it verbatim: "This
   feature is currently experimental, and you must opt in by adding the
   `experimental.async` option", with the flag "removed in Svelte 6". At
   the pinned 5.57.3 the flag is still required.
4. Streaming specifically — the docs name it as future work, not present:
   "In the future, we plan to add a streaming implementation that renders
   the content in the background." Even with the experimental flag, the
   whole response is awaited before it starts moving — there is nothing to
   stream the §7.1 boundary protocol's
   `<template data-ox-c>` swaps through.

**Why the kill and not an `experimental.async` opt-in.** Flipping
`capabilities.suspense` to `'native'` would (a) adopt an API the upstream
docs themselves call experimental and unstable at the pin — the spec's
kill criterion, verbatim — and (b) not actually stream: the adapter's
defer contract needs progressive chunks (`defer()` slots resolving while
the shell is already in flight), and an awaited-whole-body `render` cannot
carry them, so the "native" claim would be false in the capability matrix.
The conformance `defer-swap` assertion would pass only by keeping the core
swapper under the hood — a "native" label over a core implementation.

**Consequence.** No dependency, no compile-option change, no code change;
the adapter's real-SSR path (`packages/svelte/src/real-ssr.ts`, the
`svelte/server` `render` consumer) is untouched and
`capabilities.suspense` stays `'core'`. The re-run recipe for a future
session: when a Svelte release ships streaming SSR without an
experimental flag (the docs' own "streaming implementation" milestone, or
Svelte 6 dropping the flag), re-check `svelte/server`'s exports at the new
pin; if a chunked render surface exists, implement `renderStream` in
`packages/svelte/src/real-adapter.ts`, flip `capabilities.suspense`, and
run the `defer-swap` conformance assertion (§22) before claiming the cell.
