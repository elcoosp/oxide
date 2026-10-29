---
title: Wire format (ODF)
description: The Value enum, the encode/decode functions in both languages, and the hardening that makes an untrusted payload safe.
order: 3
---

# Wire format: ODF

Version-prefixed, type-tagged, little-endian. A devalue-compatible superset of
JSON. The same bytes go to the cache and to the wire, so a value that enters
the cache comes out with its types intact.

## The value model

Everything ODF can carry is a variant of one enum,
`crates/odf/src/lib.rs:37`:

```
pub enum Value {
    Null,
    Undefined,
    Bool(bool),
    /// All finite numbers plus ±Infinity/NaN via f64 bit pattern.
    Number(f64),
    BigInt(String),
    String(String),
    /// `Date` — epoch milliseconds.
    Date(i64),
    /// `URL` — serialized string form.
    Url(String),
    /// `RegExp` — source + flags.
    RegExp(String, String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
    /// Preserves non-string keys ordering semantics of a JS Map.
    Map(Vec<(Value, Value)>),
    Set(Vec<Value>),
    /// Typed array kinds (spec §9.5 "typed arrays").
    TypedArray(TypedArrayKind, Vec<u8>),
    /// `Error` — name + message (+ stack).
    Error {
        name: String,
        message: String,
        stack: Option<String>,
    },
    /// A shared reference: index into the shared-value table (cycles + reuse).
    Ref(u32),
    /// A promise placeholder: resolved later by the streaming boundary protocol (§7.1).
    Deferred {
        id: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedArrayKind {
    I8,
    U8,
    U8Clamped,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
    BigInt64,
    BigUint64,
}
```

`TypedArrayKind` (`:73`) enumerates the typed-array element types.
`Value::from_json` (`:89`) and `Value::to_json` (`:105`) are the conversion
boundary for data that never leaves JSON.

## The functions

    pub fn encode(value: &Value) -> Result<Vec<u8>>                             // :171
    pub fn decode(bytes: &[u8]) -> Result<Value>                               // :186
    pub fn encode_with_shared(value: &Value, shared: &[(u32, Value)]) -> Result<Vec<u8>>  // :196

`encode_with_shared` carries a table of already-encoded values so a cyclic or
repeated graph is written once. `decode` rejects input that exceeds the caps
below rather than attempting a partial read. The shared test fixture for the
cyclic case is `cyclic_fixture()` at `crates/odf/src/lib.rs:215`.

## Two implementations, one format

| Side | File | Role |
|---|---|---|
| Rust | `crates/odf/src/lib.rs` | encoder + decoder |
| Rust | `crates/odf/src/lib.rs:171` | `encode` |
| Rust | `crates/odf/src/lib.rs:186` | `decode` |
| TypeScript | `packages/core/src/client/odf-decode.ts` | decoder — single file, zero npm dependencies |
| TypeScript | `packages/core/src/odf-encode.ts` | re-encoder, used by the settle protocol |

**16 golden fixtures** committed as raw bytes, asserted byte-exact by both
sides. **8 encoder goldens** lock client-side `Date`, `BigInt`, `Map`, `Set`,
`URL`, `RegExp` and typed-array layout against the Rust encoder. CI fails if
either side produces different bytes; drift is a build failure, not a review
concern.


## Where the tests live

Every claim above is asserted by a test whose file you can read:

| Assertion | Test location |
|---|---|
| encode/decode round-trips every variant | `crates/odf/tests/integration.rs` |
| the 16 golden fixtures match byte-for-byte | `crates/odf/tests/integration.rs` |
| the TS decoder produces the same bytes | `packages/core/test/odf.test.ts` |
| the TS encoder matches the Rust encoder | `packages/core/test/odf-encode.test.ts` |
| the client decoder handles hostile input | `packages/core/test/odf-decode-hardening.test.ts` |
| the cyclic fixture round-trips | `crates/odf/src/lib.rs:215` (`cyclic_fixture()`) |

The two fuzz harnesses run under `cargo-fuzz`: `crates/odf/fuzz/fuzz_targets/decode.rs`
exercises `decode` on arbitrary bytes, and the corpus is regenerated from the
same golden fixtures so a real-world regression lands in the corpus.

## The TypeScript side

- `packages/core/src/client/odf-decode.ts` — the decoder. One file, no imports.
  Reads the version tag, dispatches on the type tag, and enforces the caps at
  each recursion level.
- `packages/core/src/odf-encode.ts` — the re-encoder. Used by the settle
  protocol (`packages/core/src/client/defer-glue.ts`) and by any client-side
  operation that needs to round-trip a value through the cache.
- `packages/core/src/types.ts` — the shared `Value` discriminant that both
  TypeScript files use.

## Hardening

- 256 MiB input cap, depth limit 512, 1M references — enforced by `decode`
  (`crates/odf/src/lib.rs:186`)
- `__proto__` and `constructor` keys rejected (prototype-pollution guards)
- stable `ODF-*` error codes
- `cargo-fuzz` harnesses on the Rust decoder; `proptest` suites on the router
  (`crates/router/src/lib.rs`) and graph (`crates/graph/src/`) engines

## Where you meet it

Loader data (`props.data`), `'use cache'` round trips, island props
(`data-ox-props` is base64 ODF), and the deferred-script protocol in
`packages/core/src/client/defer-glue.ts`.

Because island props are ODF, a `Date` passed as a prop arrives as a `Date`.
Same `<Island>` call in four frameworks:

```framework-tabs
---react
<Island component="Clock" chunk="Clock" props={{ at: new Date() }}>
  <Clock at={new Date()} />
</Island>
---vue
<Island component="Clock" chunk="Clock" :props="{ at: new Date() }">
  <Clock :at="new Date()" />
</Island>
---svelte
<Island component="Clock" chunk="Clock" props={{ at: new Date() }}>
  <Clock at={new Date()} />
</Island>
---solid
<Island component="Clock" chunk="Clock" props={{ at: new Date() }}>
  <Clock at={new Date()} />
</Island>
```
