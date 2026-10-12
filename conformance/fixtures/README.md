# ODF golden fixtures (conformance consumer note)

The ODF (Oxide Data Format, spec §9.5 / D11) golden fixtures **live in the Rust
crate** and are committed there:

```
oxide/crates/odf/tests/fixtures/<name>.odf.bin      encoded bytes (ground truth)
oxide/crates/odf/tests/fixtures/<name>.expected.json  Rust Value::to_json() output
```

They are consumed by the JS decoder suite at:

```
packages/oxide/test/odf.test.ts
```

via the relative path `../../crates/odf/tests/fixtures/` (resolved from the test
file's directory), so both language halves of the wire format stay in lockstep
from one source of truth. The 16 fixtures: `bigint`, `cycle`, `deferred`,
`empty-containers`, `errors`, `kitchen-sink`, `map-mixed-keys`,
`nested-arrays`, `number-specials`, `object-keys-sorted`, `primitives`,
`set-mixed`, `shared-structure`, `strings`, `temporal`, `typed-arrays`.

## Byte layout (reverse-engineered from `crates/odf/src/{wire,encode}.rs`,
verified byte-exactly by `odf.test.ts`; `crates/odf/docs/format.md` does not
exist at the time of writing)

- Payload = magic `"ODF1"` + LEB128 varuint preamble count `N` + `N` preamble
  entries + the root value. Preamble entries never register shared slots; every
  container opened in the root section registers one slot, numbered in
  pre-order; `Ref(i)` resolves to the i-th slot (identity, so cycles close).
- Record tags: `0x00` null · `0x01` undefined · `0x02` bool+u8 · `0x03`
  f64-bits-LE · `0x04` bigint-decimal-string · `0x05` string · `0x06` date
  i64-ms-LE · `0x07` url-string · `0x08` regexp (source, flags) · `0x09` array
  · `0x0a` object (byte-sorted keys) · `0x0b` map · `0x0c` set · `0x0d` typed
  array (kind byte 0..10, LE elements) · `0x0e` error (name, message,
  stack-flag) · `0x0f` ref · `0x10` deferred (varuint id). Length-prefixed
  strings = varuint byte length + UTF-8.
- The Rust encoder stores object keys byte-sorted (BTreeMap) and rejects the
  reserved keys `__proto__`/`constructor` (§17.1); the JS decoder skips them if
  it ever sees one.

## `.expected.json` → rich JS conversion table (as asserted in odf.test.ts)

| Rust `Value`                | `.expected.json`              | JS decoder returns            |
| --------------------------- | ----------------------------- | ----------------------------- |
| `Undefined`                 | `null`                        | `undefined`                   |
| `Number` NaN/±Inf           | `null`                        | `NaN` / `±Infinity` (bit-exact) |
| `Number` −0.0               | `0`                           | `-0`                          |
| `BigInt(s)`                 | `"s"` (decimal string)        | `BigInt(s)`                   |
| `Date(ms)`                  | epoch ms number               | `new Date(ms)`¹               |
| `Url(s)`                    | `s`                           | `new URL(s)`²                 |
| `RegExp(src, flags)`        | `src` (source only)           | `new RegExp(src, flags)`      |
| `Map` non-string keys       | Rust `Debug` form (`Date(7)`, `Number(2.0)`, `Bool(true)`, `Null`) | real keys |
| `Error{name,message,stack}` | `{name, message}`             | Error instance³               |
| `Ref(i)`                    | `null`                        | resolved live reference       |
| `Deferred{id}`              | `{"__deferred": id}`          | `{__deferred: id}` placeholder |

¹ Epochs outside the ECMAScript time range (|ms| > 8.64e15, e.g. `temporal`'s
`date-max` = i64::MAX) decode to the raw epoch number — a JS `Date` there would
be `Invalid Date`.
² `new URL(s).href` percent-encodes some strings (e.g. `?q=≥`); the decoder
keeps the raw string side-channel so `decodeOdfToJson` reproduces the fixture.
³ `stack` decodes to `undefined` when the record has none.

`shared-structure.expected.json` was generated from the pre-encode Value (refs
already folded), so its JSON check is replaced by an identity assertion
(`decoded[0] === decoded[1]`).

Do not regenerate fixture bytes without regenerating both the `.expected.json`
files and re-running `packages/oxide` tests — the TS suite pins the byte
format. The Rust stability test (`crates/odf/tests/integration.rs`) guards the
same files from the other side.
