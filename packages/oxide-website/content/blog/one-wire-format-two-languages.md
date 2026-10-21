---
title: One wire format, two languages — how ODF locks bytes across Rust and TypeScript
description: Oxide's cache and wire speak the same binary format. Here's the design, the golden-fixture method that makes drift impossible, and the hardening that treats every input as hostile.
date: 2026-10-29
tags: [oxide, rust, typescript, serialization, engineering]
---

Serialization is where polyglot systems quietly rot. Your server writes a `Date` into the cache;
your client reads a string. A `Map` becomes a JSON object with a prototype chain you didn't ask
for. `BigInt` becomes an exception. Each border between languages shaves types off your data, and
every team pays for it in guard-code.

Oxide is a Rust core with a TypeScript client runtime, so it has that border everywhere. ODF —
the Oxide Data Format — is how we made the border disappear, and this post is the design story.

## The rule: cache and wire are the same bytes

Most stacks treat the cache and the HTTP response as different consumers with different formats.
In Oxide there is one format for both: `'use cache'` values are stored **ODF-encoded**, and the
same encoding crosses the wire to the browser. The consequences fall out of the rule:

- A `Date` that enters the cache comes out a `Date` — on the Node and V8 backends alike
- Cyclic and shared references survive; your object graph is not flattened by accident
- Island props cross the wire in the same format (`data-ox-props` is base64 ODF), so a component
  hydrated on the client receives exactly what the server serialized
- Swapping the JS backend — `deno_core` V8 isolate pool vs Node worker pool — changes nothing
  about the bytes

## The format

ODF is a **version-prefixed, type-tagged, little-endian binary format** — a devalue-compatible
superset. Devalue (Svelte's serializer) already proved the model: no eval, no schema, types as
tags. ODF keeps that and extends it with what a server runtime needs: `Error` values and deferred
promise slots, so a value that resolves later can ride the same stream protocol as everything
else.

The value coverage, in full: JSON primitives, `Date`, `Map`, `Set`, `BigInt`, `URL`, `RegExp`,
typed arrays, `undefined`, cyclic and shared references, `Error`, and deferred slots.

## Two implementations, zero drift

The format has exactly two implementations: the encoder in Rust (`oxide-odf`) and a
single-file, zero-dependency TypeScript module that decodes — and re-encodes. Zero dependencies
is a hard constraint, not a vibe: the decoder runs in the browser, so it cannot import anything.

Methods don't create parity — **fixtures do**. Two sets live in the repo:

1. **16 golden fixtures**, committed as raw bytes. Both the Rust encoder and the TypeScript
   decoder must consume and produce them **byte-exactly**. Not "semantically equal" — identical
   bytes, asserted by CI on both sides.
2. **8 encoder goldens** covering the client-side layout of `Date`, `BigInt`, `Map`, `Set`,
   `URL`, `RegExp` and typed arrays, each locked against the Rust encoder byte for byte.

The property these enforce is stronger than "compatible". The format *cannot drift* — a change on
either side that produces different bytes fails the build before it can fail in production. When
we added `Error` / `Ref` / `Deferred` encoders recently, both sides gained the capability in the
same commit window, locked by new goldens.

## Treating input as hostile

A wire format that runs in browsers is attack surface, so the decoder ships with hardening — and
hardening **parity**, because the Rust and TS sides must reject the same things:

- 256 MiB input cap, depth limit 512, 1M references
- `__proto__` and `constructor` keys rejected (prototype-pollution guards)
- stable `ODF-*` error codes, so tooling can branch on failures without parsing messages

Beyond the goldens, the ODF decoder has `cargo-fuzz` harnesses in CI, and the broader engines
(router matcher, config extractor, path splitter) run property suites under `proptest`. Fuzzing a
decoder is table stakes; fuzzing it against the *same* hardening rules the TS side enforces is
the part that keeps the two implementations honest.

## What it buys in practice

Three concrete examples from this site, which runs on Oxide:

- The blog loader returns plain objects; the L0 response cache stores them ODF-encoded and the
  client runtime decodes them with the same module your bundle ships — no JSON.parse edge cases
- The home page's stats island receives its props as base64 ODF on the `ox-island` host element;
  if the numbers ever included dates or BigInts, they would survive too
- The deferred-value protocol (streaming late settles to the browser) rides JSON, not ODF — the settle is a short tag injected mid-stream, and JSON's zero-dependency decode on the client is the right tool for a one-shot value that never enters the cache. ODF is for the values that must survive a *cache round trip* (Date, Map, Set, BigInt, Error), which is the problem it was built for.
  its slot payloads — one parser in the browser, for everything

Serialization is not a feature users see, which is exactly why it should be boring, byte-exact
and impossible to drift. If that's your kind of boring, [the repo is
here](https://github.com/elcoosp/oxide) — and the docs cover [the full
format](/docs/wire-format-odf).
