# oxide-fw-odf

**Oxide Data Format — the binary encoding for hydration payloads.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-odf.svg)](https://crates.io/crates/oxide-fw-odf) [![docs.rs](https://docs.rs/oxide-fw-odf/badge.svg)](https://docs.rs/oxide-fw-odf)

> [!NOTE]
> Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

A compact, deterministic binary format used wherever the framework passes structured data across a boundary: the hydration payload (`window.__oxide`), cached response bodies, and deferred-settle envelopes.

The format exists so the payload a client reads is decode-safe without `JSON.parse`'s prototype-pollution surface. Every value carries its own tag; a decoder that never trusts an untrusted length prefix cannot be coerced into a prototype swap.

## What it provides

- **`Value`** — the format's tagged union: Null, Bool, Int, Float, String, Bytes, Array, Object
- **`encode_to_bytes` / `decode_from_bytes`** — roundtrip with strict bounds checks
- **`Value::from_json`** — convert from `serde_json::Value`
- **`refs`** — the reference-resolution helper that deduplicates repeated values across a payload
- **Deterministic output** — same input → same bytes, so cached bodies can be byte-compared

## Usage

```rust
use oxide_fw_odf::{Value, encode_to_bytes, decode_from_bytes};

let v = Value::Object(
    [("route".to_string(), Value::String("routes/index".into()))]
        .into_iter()
        .collect(),
);
let bytes = encode_to_bytes(&v)?;
let decoded = decode_from_bytes(&bytes)?;
assert_eq!(decoded, v);
```

> [!NOTE]
> ODF is deliberately not a general-purpose serialization format — it has no extensibility model, no schema, and no forward-compat story beyond a format version byte. If you need those, use `serde_json` or `rkyv`; ODF is the format that lives inside a `<script>` tag.

## How it fits in Oxide

`oxide-fw-render`'s `hydration_bootstrap` embeds an ODF-encoded payload in the `window.__oxide` script tag. The client-side boot module (in the framework packages under `packages/oxide`) decodes it. `oxide-fw-cli`'s HMR route-data update path uses the same encoder for the `route-data-update` message envelope.

## See also

- [`oxide-fw-render`](https://crates.io/crates/oxide-fw-render) — embeds the payload in HTML
- [`oxide-fw-cache`](https://crates.io/crates/oxide-fw-cache) — stores ODF bodies in cached entries
- [`serde_json`](https://crates.io/crates/serde_json) — the JSON conversion the CLI uses

---

Full API reference: [docs.rs/oxide-fw-odf](https://docs.rs/oxide-fw-odf)
