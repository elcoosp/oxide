<!-- GENERATED FILE — `oxide docs generate` (spec v0.2 §V2-E11). DO NOT EDIT.
Regenerate: cargo run -p oxide-fw-cli -- docs generate
Drift gate: crates/cli/tests/docs_tdd.rs -->

# Error code registry

Every stable diagnostic code the tool can emit (Appendix V2.K). Generated from `docs/errors.toml` — the registry file, not the call sites; `crates/cli/tests/errors_registry_tdd.rs` is the gate that fails when a production call site raises a code this page does not carry.

## Codes

| code | message | fix |
| --- | --- | --- |
| `ADP-CAP` | target `{}` does not support `{feature}`; remove the requirement or pick another target (§18 refusal rule) | — |
| `ADP-IO` | cannot create {}: {e} | — |
| `ADP-JSON` | serialize {name}: {e} | — |
| `ADP-STATIC-DYN` | route `{route}` is `dynamic` and has no prerendered output; static export requires prerendering or `static.fallback` (§10.5) | Prerender the route or set `static.fallback` |
| `AST-AVIF` | ravif encode failed: {e} | — |
| `AST-CSS` | lightningcss parse: {e} | — |
| `AST-DIR` | collection `{}` directory not found: {} | — |
| `AST-ENTRY` | failed to read {}: {e} | — |
| `AST-FNT-001` | `fonts.subset: 'used'` requires every route to be `static` (glyph-usage subsetting needs all text known at build), but the project has non-static routes | Use `latin` or `unicode-range`, or make all routes static |
| `AST-FNT-002` | `fonts.subset: 'used'` requires the `subset` feature, which is not compiled into this build of oxide-fw-assets | rebuild with `--features subset`, or drop `used` from `fonts.subset` |
| `AST-FONT` | skrifa parse failed: {e} | — |
| `AST-FRONT` | unclosed frontmatter in {} (missing closing `---`) | — |
| `AST-I18N` | MessageFormat parse failed: {} | — |
| `AST-I18N-ARG` | MessageFormat: a value is missing for a placeholder argument | Pass a `values` entry for every placeholder in the message template |
| `AST-I18N-TYPE` | MessageFormat: a placeholder value has the wrong type | Pass the value type the template expects (string, number or date) |
| `AST-IDX` | index serialization failed: {e} | — |
| `AST-IMG` | failed to decode source image: {e} | — |
| `AST-JOBS` | the image encoder panicked: {join} | inspect the encoder diagnostic; the next request retries the encode |
| `AST-IMG-REQ` | `url` query parameter is empty | — |
| `AST-LIV-001` | a route opts into `live: true` but target `{}` does not support live loaders (§18 capability matrix) | Use `live.transport: 'poll'` or change target |
| `AST-OG` | usvg parse failed: {e} | — |
| `AST-QUERY` | asset specifier must be a non-empty path optionally followed by `?query` | — |
| `AST-REF` | collection `{}` entry `{}` links to `{target}` but no entry with slug `{stem}` exists | — |
| `AST-SCH` | collection `{}` entry `{}` has no frontmatter; external schema requires content | — |
| `AST-SRC-001` | `search` is configured but target `{}` does not support the built-in search capability (§18 capability matrix) | Use static JSON search or a Binary or Docker target |
| `AST-SVG-001` | the optimized SVG output violated a preservation rule: {rule}; the source is kept | Narrow the optimizer preset (the bundled backend already runs the correctness preset) or file an upstream issue; the build degrades to the unoptimized source |
| `AST-SVG-002` | `svg.optimize: true` requires the `svg-oxvg` feature, which is not compiled into this build of oxide-fw-assets | Rebuild with `--features svg-oxvg`, or set `svg.optimize: false` |
| `AST-SVG-003` | the SVG optimizer failed on the document: {e} | Check the document is well-formed XML; the build keeps the unoptimized source |
| `AST-WEBP` | webp encode unavailable in this image build; emitted PNG instead: {e} | — |
| `ATT-CANON` | the provenance file is not canonical JSON (whitespace tamper) | Rewrite the provenance in canonical form; `oxide verify --rebuild` re-checks |
| `ATT-DIST` | the dist is missing, `dist/build-id` is absent, or a subject digest does not match the dist on disk | Run `oxide build --attest` first |
| `ATT-IO` | filesystem I/O failed while writing or verifying the attestation | — |
| `ATT-KEY` | no signing key on disk or in OXIDE_SIGNING_KEY, or the key file could not be read | Pass `--key <file>` or set `OXIDE_SIGNING_KEY` |
| `ATT-KEYFMT` | the signing key is not an ed25519 PKCS#8 private key | Pass an unencrypted ed25519 PKCS#8 key (`openssl pkey -keyform DER`) |
| `ATT-LOCK` | `{file}` is not a parseable cargo/pnpm lockfile | Regenerate the lockfile with the pinned toolchain |
| `ATT-PUBKEY` | the public key is not ed25519 (SPKI PEM, 64-char hex or 32-byte base64) | Pass the ed25519 public key that pairs with the signing key |
| `ATT-SIG` | the provenance signature does not verify over the statement | Re-attest with the matching signing key |
| `AUDIT-OFFLINE` | {msg} | — |
| `BLD-CSS` | lightningcss failed for {path}: {} | — |
| `BLD-CYCLE` | import cycle detected: {chain} | break the cycle by extracting the shared code into its own module |
| `BLD-GUARD` | server-only module leaked into client graph | — |
| `BLD-IO` | mkdir {}: {e} | — |
| `BLD-JSON` | {e} | — |
| `BLD-PLUGIN` | plugin load({:?}) failed: {} {} | — |
| `BLD-RESOLVE` | entry module `{}` for route `{route}` not found | — |
| `BLD-ROLLDOWN` | tokio runtime: {e} | — |
| `BLD-SFC` | check the tooling-pool logs for the {ext} compile failure of {bare}; the build must not bundle raw framework SFC sources | — |
| `BLD-SMAP` | plugin map refused: {e} | Fix the plugin's `map` field to serialize a valid V3 sourcemap (version 3, base64-VLQ `mappings`), or omit the field — the build continues with the code and no map |
| `BLD-TAILWIND` | the @oxide-fw/tailwind plugin pass failed | Check the @oxide-fw/tailwind plugin config and the CSS entry |
| `BND-SVG` | `{path_for_errors}`: no <svg> element found | — |
| `CAC-BUILDID` | dist missing: {} | — |
| `CAC-DB` | redb: {e} | check disk space/permissions for the cache root |
| `CAC-DISK` | entry {raw_len} exceeds per-entry cap {} | — |
| `CAC-IO` | disk tier: {e} | — |
| `CAC-JSON` | index codec: {e} | — |
| `CAC-ZSTD` | compress: {e} | — |
| `CACHE-CODEC` | zstd: {e} | — |
| `CACHE-HTTP` | entry decode: {e} | — |
| `CACHE-REDIS` | open {redacted}: {e} | — |
| `CACHE-S3` | s3 client: {e} | — |
| `CAS-CORRUPT` | index entry {} has no blob file on disk | — |
| `CAS-DB` | open index: {e} | — |
| `CAS-IO` | create {}: {e} | — |
| `CAS-NO-ENTRY` | touch: key {} is not in the index | — |
| `CAS-REMOTE` | remote cache URL must be http:// ({url}) | — |
| `CFG-001` | unknown config key `framwork` | did you mean `framework`? |
| `CFG-AVIF` | `images.avifSpeed` must be a whole number in 1..=10 (ravif's encoder speed) — got `{got}` | set `images.avifSpeed` to a whole number in 1..=10 (the default is 6) |
| `CFG-BACKEND` | `framework.compilerBackend` must be one of `auto`, `native`, `node` — got `{got}` | set `framework.compilerBackend` to `auto` (the default), `native` or `node` |
| `CFG-DEPLOY` | the `deploy` section carries a bad knob: {msg} | spell the section `deploy: { skewWindow: '15m', generations: 3 }` — the window grammar is `<n>s|m|h|d`, plain integers are seconds |
| `CFG-EXTENDS` | extends chain too deep at `{spec}` (cycle?) | — |
| `CFG-IO` | read {}: {e} | — |
| `CFG-LLMS` | the `llms` section carries a bad knob: {msg} | fix the `llms` section — `enabled` is the unquoted boolean master switch and `include` an array of absolute site-path patterns like `/docs/**` |
| `CFG-REC` | the `record` key is `false`, `'all'`, `'errors'` or `{ sample, onError }` | spell the key `record: false`, `record: 'all'`, `record: 'errors'` or `record: { sample: 0.001, onError: true }` |
| `CFG-SCHEMA` | serialize defaults: {e} | — |
| `CFG-SEC` | `security.{key}` must be one of {valid} — got `{got}` | Set `security.{key}` to one of the valid spellings |
| `CFG-SIZE` | invalid size `{s}`: empty string | — |
| `CFG-UNKNOWN-KEY` | config root must be an object | — |
| `CHECK-LINT` | `{bin}` reported errors | — |
| `CHECK-TS` | the typecheck binary reported errors | Fix the reported type errors, or narrow the `check` config |
| `CMP-CACHE` | 'use cache' function has no binding name | — |
| `CMP-GUARD` | server-only `{ident}` from `{}` shipped into client chunk `{chunk}` | — |
| `CMP-NAT-001` | framework.compilerBackend is 'native' but no healthy native compiler is available for {fw} | build oxide-cli with --features native-{fw} or set framework.compilerBackend to 'auto' or 'node' |
| `CMP-PARSE` | fix the syntax error reported by the Oxc parser | Fix the syntax error the parser names |
| `CMP-SECRET` | `process.env.{name}` is read in client module `{}`;                          this would ship the secret to the browser. Move the                          read to a server module or rename to `PUBLIC_{name}`. | — |
| `CONTENT-TYPEGEN` | mkdir {}: {e} | — |
| `DEP-BUILDID` | the deploy build id `{}` must be non-empty lowercase hex (the A04 shape) | — |
| `DEP-IO` | deploy store operation failed on `{}`: {e} | — |
| `DEP-MANIFEST` | the remote deploy manifest (manifest.json) is corrupt or from a future engine version: {e} | Re-run the deploy - a fresh deploy rewrites the ledger; inspect the store's manifest.json if this repeats |
| `DEP-POINTER` | the deploy pointer (current) is corrupt: {detail} | — |
| `DEP-ROOT` | dist root {} is not a directory | Pass the build's dist directory (the `oxide build` output) |
| `DEP-ROLLBACK` | cannot roll back: no previous build id is recorded | Deploy first: every deploy records the pointer's previous value |
| `DEP-URL` | `{}` is not a deploy target URL: {} | Use s3://bucket/prefix, r2://bucket/prefix?endpoint=... (the endpoint query parameter is required for r2), file:///path or mem://label |
| `DEV-BUNDLE` | rebuild failed for {pstr}: {err_msg} | — |
| `DEV-HMR-001` | HMR fell back to a full reload: {reason} | — |
| `DEV-HMR-002` | {} is not imported by any module | — |
| `DEV-HMR-003` | HMR frame speaks schema v{found}; this build speaks v{} | — |
| `DEV-NOTIFY` | create watcher: {e} | — |
| `DEV-QR` | QR generation failed: {e} | — |
| `DEV-TLS` | certificate params failed: {e} | — |
| `DOC-001` | config keys carry no schema description; the page refuses to render (E11 acceptance) | add a `description` in `crates/config/src/schema.rs`, then regenerate the pages |
| `DOC-002` | docs/errors.toml carries an entry the strict registry format rejects (the diagnostic names the line) | fix the entry the diagnostic names (the format is `[CODE]` + `message`/`fix`/`spec_only`) |
| `DOC-003` | a docs/reference page is missing or stale against the running binary | regenerate with `cargo run -p oxide-fw-cli -- docs generate` |
| `DSP-101` | a middleware staged a reserved locals key (`session`, `locale` or `experiments`) | Rename the key; reserved keys are owned by the core |
| `DSP-110` | invalid experiment definition (traffic outside 0-100, or fewer than two variants) | Fix the `experiments` entry |
| `DSP-111` | invalid `live` section (keepaliveSeconds/maxSubscribers/maxRerunsPerSecond below 1, or perIpMax above maxSubscribers) | Fix the `live` section |
| `DSP-112` | invalid `stream.resume` section (ttlSecs below 1, maxBytesEach below 1 byte, or maxBytesTotal below maxBytesEach) | Fix the `stream.resume` section |
| `DSP-113` | invalid overload priority spelling (rules[].priority accepts only `low`, `normal`, `critical`) | Use `low`, `normal` or `critical` |
| `DSP-114` | invalid `interactionReplay` spelling (a non-boolean value) | Set `interactionReplay` to the unquoted boolean `true` or `false` |
| `DSP-115` | invalid `prefetch` section (a non-boolean `learned`, or a `minProbability` outside 0.0-1.0) | Fix the `prefetch` section |
| `EDGE-101` | routes manifest must be an array of route rows or {"routes": [...]} (router typegen §8.3) | — |
| `FN-BAD-REQUEST` | the server-function envelope is malformed (the 400 dispatch class) | Send the envelope shape `/_oxide/fn/<id>` documents |
| `FN-INTERNAL` | the server-function dispatch failed internally (the 500 dispatch class) | Inspect the server logs; the dispatch failed inside the host |
| `FN-NOT-FOUND` | no server function with that id is registered in this build | Rebuild, or check the id the client posts to `/_oxide/fn/<id>` |
| `FN-THREW` | the server function threw; the envelope carries the function's own error message | — |
| `GRAPH-ARTIFACT` | rkyv serialize: {e} | — |
| `GRAPH-CANCEL` | computation cancelled: a newer change invalidated in-flight work | — |
| `GRAPH-CORRUPT` | artifact: truncated header | — |
| `GRAPH-CYCLE` | dependency cycle detected at node `{}` | — |
| `GRAPH-FS-IO` | read {}: {e} | — |
| `GRAPH-NO-COMPUTE` | node `{}` is not memoized and has no registered Compute | — |
| `GRAPH-S3` | s3 client (bucket {bucket}): {e} | — |
| `I18N-LOAD` | the locale catalog could not be read or parsed ({path}: {e}) | Create the locales dir or fix the locale file's frontmatter |
| `IF-NONE-MATCH` | \"a\" | — |
| `JSH-ABORT` | call aborted before completion | — |
| `JSH-ACTION` | ACT | — |
| `JSH-B64` | invalid base64 length/padding ({}) chars | — |
| `JSH-BOOM` | worker simulated failure for kind `{}` | — |
| `JSH-COMPILE` | plugin transform failed for {path}: {} | — |
| `JSH-CRASH` | worker simulated crash (kind `die`) | — |
| `JSH-DEADLINE` | chunked stream did not finish within 30s | — |
| `JSH-DRAIN` | pool is draining; no new calls are accepted (§12.2) | — |
| `JSH-ENDPOINT` | ENDPOINT | — |
| `JSH-FETCH` | the SSRF-guarded fetch the framework adapter issued failed | — |
| `JSH-FN` | server-fn:run returned invalid JSON: {e} | — |
| `JSH-FN-THREW` | the server function threw; the envelope carries the function's own error message | Fix the thrown error in the server function |
| `JSH-FRAME` | frame of {} bytes exceeds the 64 MiB protocol limit (§4.2) | — |
| `JSH-JSON` | failed to serialize worker message: {e} | — |
| `JSH-LOADER` | loader:run response missing numeric `status` | — |
| `JSH-NODE` | failed to spawn node worker `{node_bin}`: {e} | — |
| `JSH-NODESHIM` | crypto shim missing loud-throw for unsupported algorithms | — |
| `JSH-NOSCRIPT` | ScriptedHost has no handler registered; construct it via ScriptedHost::with_handler(..) | — |
| `JSH-OP` | cache_get requires a string `key` | — |
| `JSH-OVERFLOW` | chunked stream exceeded the {} MiB collect cap | — |
| `JSH-PARAMS` | generate-params:run returned invalid JSON: {e} | — |
| `JSH-POOL` | V8 pool has no workers | — |
| `JSH-SNAPVER` | snapshot at {} is not a versioned envelope (stale raw blob?); expected format {} / oxide {} | — |
| `JSH-SOCKET` | SOCKET | — |
| `JSH-TIMEOUT` | no response from worker within 10s for call id {id} (kind `{}`) | — |
| `JSH-TOOLING` | task join failed: {e} | — |
| `JSH-V8` | tokio rt: {e} | — |
| `JSH-V8KIND` | V8 backend incompatible with the server code (JSH-V8KIND) | — |
| `NOT-FOUND` | JSH-ACTION | — |
| `ODF-CYCLE` | reference expansion exceeds the {MAX_DEPTH}-level limit | — |
| `ODF-DEPTH` | value is nested deeper than the {MAX_DEPTH}-level limit | — |
| `ODF-KEY` | reserved object key `{k}` (§17.1: use a Map for data-level keys) | — |
| `ODF-MAGIC` | payload does not start with the b\"ODF1\" magic + version | — |
| `ODF-REF` | shared table has {n} entries, limit is {MAX_REFS} | — |
| `ODF-SIZE` | encoded payload is {} bytes, limit is {MAX_BYTES} | — |
| `ODF-TAG` | invalid BigInt decimal string `{s}` (expected `-?(0\|[0-9]+)`) | — |
| `ODF-TRUNC` | payload ends prematurely: needed {n} more byte(s) at offset {} | — |
| `ODP-BASE` | the patch's base digest does not match the target document | — |
| `ODP-DEL` | del target is not an object key | — |
| `ODP-KEY` | reserved key {k:?} cannot appear in a patch path | — |
| `ODP-MAGIC` | patch payload does not start with the b\"ODP1\" magic | — |
| `ODP-OP` | unknown op code {other} | — |
| `ODP-PATH` | del path is empty | — |
| `ODP-SHAPE` | patch root is not an object | — |
| `ODP-SPLICE` | splice target is not an array | — |
| `OX-DYN` | route is dynamic; the attached reasons name the reads that force it (F16) | Run `oxide why <route> --fix` |
| `OX-PLUGIN` | plugins[{i}] (kind:\"native\") is missing a `name` field | — |
| `PERF-BUILD` | the candidate `oxide build` failed inside the bisect worktree | — |
| `PERF-GIT` | git is unavailable on PATH, or a git plumbing invocation (rev-list, worktree, rev-parse) failed | — |
| `PERF-METRIC` | PERF-THRESHOLD | — |
| `PERF-RANGE` | the bisect range is empty or `good` is not a first-parent ancestor of `bad` | Pass a range where `good` precedes `bad` on the first-parent chain |
| `PERF-RUNS` | `--runs` must be at least 1 | Pass `--runs 1` or higher |
| `PLG-BRIDGE` | bridge runtime: {e} | — |
| `PLG-CALL` | plugin transform failed: {} | — |
| `PLG-CAP` | WASM plugin `{}` requests capability `{cap}` — allowed: fs, network, env, clock (§17.2 sandboxing) | — |
| `PLG-HOOK` | plugin `{}` declares hook `{}`, which the JS bridge does not support — see the compatibility table (§15.2/§15.3) | — |
| `PLG-HOOKS` | WASM plugin `{}` declares no hooks — nothing to dispatch (§15.2) | — |
| `PLG-LOAD` | plugin:load requires a `path` | — |
| `PLG-WASM` | wasmtime engine init failed: {e} | — |
| `PLG-WIT` | WASM plugin `{}` declares no `wit_world` — a stable WIT interface is required (§15.1 kind 2) | — |
| `RFN-002` | `*.server.rs` functions are present but target `{}` cannot load native functions (V2-F08) | Use Binary or Docker, or remove the Rust functions |
| `RND-INJECT` | html injection failed: {e} | — |
| `RND-PPR` | unterminated boundary tag | — |
| `RND-SCRIPT` | <Script> requires either a `src` or inline `content` | — |
| `RND-UTF8` | render stream was not UTF-8: {e} | — |
| `RPL-001` | cannot write or load the record {}: {e} | check the project root's writability; a record id must be a single path-safe segment |
| `RPL-002` | Replay saw an outbound call that is not in the record | Re-record, or the code path changed |
| `RPL-003` | Record was made on an older build id | Replay continues; results may differ |
| `RPL-004` | Replay exhausted the recorded non-deterministic sequence | Re-record; the replay observed more than the record captured |
| `RPL-005` | cannot load the record: schema version {} is not one this build reads (version 1) | re-record the request with the current build; the record schema changed |
| `RTR-001` | routes directory not found: {} | — |
| `RTR-002` | invalid route directory `{name}`: {msg} | — |
| `RTR-100` | failed to serialize route archive: {e} | — |
| `RTR-101` | cannot open route archive {}: {e} | — |
| `RTR-102` | route archive is corrupt: {e} | — |
| `RTR-110` | rule #{n}: source `{}` must start with `/` | — |
| `RTR-200` | cannot create typegen directory {}: {e} | — |
| `SCHEMA-001` | SCHEMA-002 | — |
| `SCHEMA-002` | the config module's value is not JSON (the schema crate's parse class) | Serve the module's value as JSON (the schema crate parses values, not TS) |
| `SEC-SESSION` | a session codec needs at least one key (security.sessionKeys) | — |
| `SNAP-002` | top-level side effect | create connections lazily |
| `SNP-FORMAT` | snapshot format {} is not supported (expected {}) | — |
| `SNP-PANIC` | snapshot builder thread panicked | — |
| `SRC-101` | failed to open the search index directory (missing, stale or corrupt meta.json) | run `oxide build` (the `search.collections` emitter writes dist/search/index), or clear search.collections |
| `SRC-102` | failed to build or commit the search index (writer, commit or merge failure) | check the dist directory is writable and retry the build |
| `SRC-103` | search document ingest refused: the entry has an empty `id` or `url` | every indexed entry needs the collection's stable key and a page URL |
| `SRC-104` | the search query is empty or failed to execute | send `GET /_oxide/search?q=<text>`; a punctuation-only query answers an empty response |
| `SRC-105` | the static search index failed to serialize or parse (dist/search/static.json) | rebuild (the export is written atomically); the shape is `{docs, postings}` |
| `SRC-106` | search.collections names a collection content.collections does not have | add the collection to content.collections, or fix the search.collections spelling |
| `SRV-408` | request body read timed out | — |
| `SRV-413` | request body exceeds the limit of {} bytes | — |
| `SRV-ASSET` | asset root {} is not a directory | — |
| `SRV-BIND` | cannot bind {addr}: {e} | — |
| `SRV-COMP` | gzip failed: {e} | — |
| `SRV-CSRF` | cross-origin request rejected by Origin/Sec-Fetch-Site verification | — |
| `SRV-NOENC` | unsupported content encoding `{other}` | — |
| `SRV-PROXY` | build request failed: {e} | — |
| `SRV-TLS` | cert parse failed: {e} | — |
| `SRV-WS` | the websocket handshake or a frame decode failed | — |
| `VRF-INTEGRITY` | client manifest integrity verification failed: {detail} | Restore dist/client from a trusted copy or rebuild with `oxide build` |

## Spec-only codes

Defined by the spec for work packages that have not landed; no call site raises them yet.

| code | message | fix |
| --- | --- | --- |
| `BLD-409` | Build header unknown or expired (surfaced client-side as a reload) | Reload the page |
| `OX-SRC-001` | Search params failed `searchSchema` validation | Fix the URL or add defaults to the schema |
| `PRM-001` | A route used a capability outside its declared permissions (F19) | Add it to `config.permissions` or remove the call |
| `RFN-001` | Rust function library ABI version mismatch | Rebuild the user crate with the matching `oxide-rustfn` version |
