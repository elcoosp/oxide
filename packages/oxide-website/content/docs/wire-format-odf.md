---
title: Wire format (ODF)
description: The Oxide Data Format — one binary wire format, byte-exact across Rust and TypeScript.
order: 3
---

# Wire format: ODF

ODF, the Oxide Data Format, is a version-prefixed, type-tagged, little-endian binary format — a
**devalue-compatible superset** covering JSON primitives plus the types JSON loses:

- `Date`, `Map`, `Set`, `BigInt`, `URL`, `RegExp`, typed arrays
- `undefined`, cyclic and shared references
- `Error` and deferred promise slots

## Why it matters

In most stacks the cache and the wire speak different dialects, and something is lost at every
border. In Oxide the same bytes go to the cache **and** to the wire: `'use cache'` values are
stored ODF-encoded, so a `Date` that enters the cache comes out a `Date` — on the Node and V8
backends alike.

## Byte parity by construction

- Encoded natively in Rust (`oxide-odf`)
- Decoded — and re-encoded — by a single-file, zero-dependency TypeScript module
- **16 golden fixtures** are committed as raw bytes and asserted byte-exact by both sides
- **8 encoder goldens** lock the client-side `Date` / `BigInt` / `Map` / `Set` / `URL` / `RegExp`
  / typed-array layout against the Rust encoder byte for byte

The format cannot drift between languages: CI fails if either side produces different bytes.

## Decode hardening

Untrusted input is a given, so the decoder ships with hardening parity on both sides:

- 256 MiB input cap, depth limit 512, 1M references
- `__proto__` / `constructor` rejection (prototype-pollution guards)
- stable `ODF-*` error codes

The ODF decoder additionally has `cargo-fuzz` harnesses in the repo, and the property suites on
the router and graph engines run under `proptest`.

## Where you meet it

- Loader data crossing the Rust → JS boundary (`props.data`)
- `'use cache'` round trips (L1 data cache)
- Island props (`data-ox-props` is base64 ODF)
- The deferred-script protocol that streams late-resolving values to the client
