# Security review — v0.2 (R05 draft)

Status: **draft for sign-off** (the R05 deliverable; the sign-off block at
the end is empty until the release review passes). Scope: the surfaces the
v0.2 release wave newly wired — live (SSE), stream (resumable), the Rust
function host, search, record/replay, the image endpoint and the deploy
target — reviewed against the spec's R05 control table
(`docs/spec-v0.2-draft.md`, §V2-R05). Method: every control below was read
out of the tree at HEAD `379e138b` and is linked to the file that implements
it and the test that pins it; anything the tree does not implement is
recorded as an explicit gap in the last section, never claimed as done.

Plane rule (Appendix V2.J): `/__oxide/*` is the data plane (the A04
build-header stage gates it); `/_oxide/*` is the ops plane. Dev-only
surfaces carry the dev-token guard (D41).

## 1. `/_oxide/live` — live SSE fan-out (F05)

| Threat | Control | Where | Pinned by |
| --- | --- | --- | --- |
| Cross-origin read of loader snapshots | Same-origin gate: `Sec-Fetch-Site` must be `same-origin` when present; otherwise `Origin`'s authority is compared against `Host` (host case-insensitive, port exact); a request with **no** origin evidence is denied | `crates/server/src/live.rs::same_origin` | the `same_origin` unit pins in the same file |
| Subscriber exhaustion (FD/memory) | Global **and** per-peer subscriber capacity enforced at `subscribe`; over capacity answers `503` with `retry-after` (short — capacity recovers as connections close) | `live.rs` (`Hub::subscribe`, `live_capacity_response`) | hub capacity pins |
| Cookie replay via the connect snapshot | The per-subscriber header snapshot (cookies included) lives only as long as the connection, is never logged (the `is_secret_key` class guards the Debug/trace path), and is dropped with the subscriber | `live.rs` `Sub` (documented on the field) | the crate's Debug-redaction pin (`cookie`/`authorization`/`set-cookie`/`x-oxide-fw-dev-token` skipped in `{:?}`) |
| Rerun amplification (one revalidation → N loader runs) | Per-subscriber rerun rate cap in the `Guard` before the injected `rerun` closure runs | `live.rs` (`Guard`) | the guard pins |

Residual: SSE behind buffering proxies gets `x-accel-buffering: no` +
keepalive pings + the poll-transport fallback (the §V2 risks table) — a
latency concern, not an access-control one.

## 2. `/__oxide/stream/:id` — resumable streams (F07)

| Threat | Control | Where | Pinned by |
| --- | --- | --- | --- |
| Guessing ids / reading another user's stream | The stream id is `blake3(request_id ‖ counter)` — a full 256-bit digest, minted per stream and disclosed **only** in the response head (`x-oxide-stream`) of the response that created it; a reattach with an unknown, expired or overflowed id answers `410` (empty body) | `crates/server/src/stream_store.rs` (`stream_id`, `Reattach::Gone`), wired at `crates/cli/src/dispatch/stages/endpoint.rs` (the tee-into-store + head step) | the stream-store TDD pins (reattach-at-end byte-identical, `410` shapes) |
| Unbounded replay buffer (memory DoS) | The store is a bounded buffer; reattach past the buffer's length or past an overflowed buffer is `410` | `stream_store.rs` | the overflow pins |
| Cross-plane access | The endpoint rides the data plane: the A04 build-header stage gates it like `/__data` | the dispatch stage table | the A04 stage pins |

**Gap (recorded, not shipped):** the spec's R05 control "bind the id to the
original request's session-cookie hash; mismatch gives 410" is **not**
implemented — the compensating controls are the 256-bit id entropy and the
fact that the id is never disclosed outside the creating response. A
multi-user deployment that treats stream bytes as per-session secret must
not enable resumable streams until the binding lands (v0.3 candidate).

## 3. `/_oxide/fn/*` — server functions incl. the Rust host (F08)

| Threat | Control | Where | Pinned by |
| --- | --- | --- | --- |
| Arbitrary export call | Calls dispatch through the registered server-fn registry by stable id; the Rust host speaks the C ABI over bytes with an **ABI version gate** — a foreign `abi_version` refuses `RFN-001` naming both sides | `crates/rustfn/src/host.rs` (the `ABI_VERSION` check), `crates/rustfn/src/abi.rs` | the host TDD pins (version mismatch refuses) |
| Panics crossing FFI | `catch_unwind` at the shim maps a guest panic to `STATUS_PANIC` instead of unwinding into the host process | `crates/rustfn/src/shim.rs` | the shim pins |
| Resource exhaustion | Per-function timeout (default 30 s) and a bounded semaphore around invocation; buffers are the bounded `RawBuf` shape | `crates/rustfn/src/host.rs` (module contract) | the host pins |
| SSRF via outbound fetch (the shared guard also protects the image endpoint) | `ssrf_allowed`: literal IPs in loopback/private/link-local ranges refused; `localhost` (any case), `*.localhost`, `*.local` and `metadata.google.internal` refused; explicit allow-list override; re-validation after redirect/DNS is the fetch op's documented caller duty | `crates/server/src/lib.rs::ssrf_allowed` + `is_blocked_ip` | the SSRF unit pins |

**Gap:** the fuzz targets the spec names for this surface (`rustfn_shim`)
do not exist in `fuzz/` yet — the shim is property-pinned by unit tests
only. Recorded below.

## 4. `/_oxide/search` — search (F09)

| Threat | Control | Where | Pinned by |
| --- | --- | --- | --- |
| Query DoS | Per-peer rate limit — `20` requests / `10 s` (`SEARCH_RATE_MAX`/`SEARCH_RATE_WINDOW_SECS`); result cap `MAX_RESULT_LIMIT = 100` (default 10) so one query cannot serialize the whole corpus; non-GET is `405` + `allow: GET` | `crates/server/src/search_endpoint.rs` | the endpoint pins (rate-limit + caps) |
| Engine error leakage | Engine codes map to stable responses: the known `SRC-104` shape is a `400`; any other engine code is a `500` naming the code — a serialization failure is a loud `500` text body, never a silent `200 {null}` | `search_endpoint.rs` (`search_response`) | the error-shape pins |
| Regex injection | The client static matcher (`packages/core/src/client/searchEngine.ts`) does substring/postings arithmetic only — no regex evaluation of user input; the server engine is fuzzy-substring over the tantivy schema, also not regex | `searchEngine.ts`, `crates/search` | the F09 conformance assertion + engine pins |
| Client-side posture | Search ships as a plain client module riding the static index — **no WASM**, so the attack surface is the same origin's own JS | `packages/core/src/client/searchEngine.ts` | the `search:` conformance assertion (§22) |

**Gap:** the spec's explicit "query length cap (200)" is not a named
constant — the rate limit and result cap are the implemented DoS controls;
a query-length cap is a one-line follow-up recorded below.

## 5. Record and replay (F15) — secret leakage

| Threat | Control | Where | Pinned by |
| --- | --- | --- | --- |
| Secret headers captured verbatim | The header redactor replaces the secret trio (`authorization`, `cookie`, `set-cookie`) with a `sha256:<12-hex>` digest (the D30 cross-implementation digest), order preserved; `include_secrets` keeps values verbatim and is dev-policy only | `crates/dispatch/src/record.rs` (`HeaderRedactor::redact`, `RecordPolicy`) | the record TDD pins (trio redacted, others verbatim) |
| Records leaking into VCS | Records land under `.oxide/records/`; the repo gitignore covers `.oxide/` at every root — the scaffold template carries the same entry (verify per-app on adoption; recorded as a checklist item below) | `.gitignore`, the create-oxide template | — |
| Replay diffing leaking values | The replay diff compares digests/normalized shapes; the 32-hex nonce is blanked and the re-derived header trio (`content-security-policy`, `date`, `set-cookie`, `server-timing`) is skipped | `crates/dispatch/src/replay.rs` | the replay pins |
| Diagnostics leaking cookies | Structured log events carry whitelisted fields only — no cookies, no raw bodies | `crates/server/src/lib.rs` (the structured-event path) | the redaction pins |

## 6. `/_oxide/image` — the image endpoint (C09, SEC-08)

| Threat | Control | Where | Pinned by |
| --- | --- | --- | --- |
| URL forgery / open-transform abuse | Optional request signing: `s = blake3(url ‖ secret)` — the **full 64-hex digest** (SEC-08: the old 16-hex prefix was forgeable by birthday search and was upgraded), compared **constant-time** | `crates/assets/src/lib.rs` (`sign_image_url`, `verify_image_signature`, `constant_time_eq_hex`) | the assets pins (sign/verify/forge-refusal) |
| Signing misconfiguration | A configured secret is signing ENABLED; an **empty** secret (the CLI's `OXIDE_IMAGE_UNSIGNED=1` operator disable maps to `Some("")`) is signing DISABLED — the endpoint answers unsigned instead of failing closed on a misconfigured empty value; the disable is explicit operator intent, not a default | `crates/server/src/lib.rs` (the empty-secret arm), `crates/cli/src/endpoints/image_og.rs`, `crates/cli/src/tests.rs` (the `OXIDE_IMAGE_UNSIGNED` mapping pin) | the server empty-secret pin + the CLI mapping pin |
| SSRF through remote sources | Remote `http(s)` sources are validated against `ImagesConfig.remotePatterns` and the shared `ssrf_allowed` guard (Section 3) | `crates/assets/src/lib.rs` (`ImageEndpointRequest::url` contract) + `ssrf_allowed` | the SSRF pins |
| Encode-job amplification | The job model bounds concurrency (CPU lane + AVIF lane semaphores) and single-flights per output key — a burst cannot fan out into unbounded encodes (the C09 acceptance: 64 concurrent, one encode) | `crates/assets/src/image_jobs.rs` | `tests/image_acceptance.sh` (CI, jshost-e2e) |

## 7. `oxide deploy` — delta deploy / object store (F11)

| Threat | Control | Where | Pinned by |
| --- | --- | --- | --- |
| Credential handling | The engine never sees, stores or logs credentials — `s3://`/`r2://` targets ride the `object_store` crate's standard provider chain (env/profile per backend); the `dir:`/`file://`/`mem://` targets are network-free | `crates/adapters/src/objstore.rs` (the seam doc), `crates/adapters/src/delta.rs` (`DeployStore`) | the objstore TDD pins |
| Pointer clobbering (lost deploy race) | The `current` pointer swap is a conditional put (`PutMode::Update(etag)`, `Create` on fresh); a write against a moved-or-taken version refuses `DEP-POINTER` and never lands | `objstore.rs` (the pointer-swap doc) | `tests/object_store_tdd.rs` |
| Path traversal on object keys | `DirStore` object writes go through the traversal guard (no `..`/absolute escapes past the target root) | `crates/adapters/src/delta.rs` (the guard behind `DirStore`) | the traversal refusal pins |
| Deploying an unstamped dist | The plan refuses with `DEP-BUILDID` before any upload — a dist without the build stamp cannot become `current` | `delta.rs` / the CLI arm | `tests/deploy_acceptance.sh` (CI: the unstamped refusal arc) |
| Corrupt ledger/pointer state | `DEP-MANIFEST` / `DEP-URL` / `DEP-IO` coded refusals; content addressing is blake3 over the scanned `FileEntry` map (the §V2-F11 order of operations) | `delta.rs` | the delta TDD pins |

## 8. Gaps and follow-ups (honest list)

1. **Stream↔session binding** (Section 2): the id is 256-bit and disclosed
   only to the creating response, but the spec's session-cookie-hash
   binding is not implemented. Multi-user per-session-secret deployments
   should keep resumable streams off until it lands.
2. **Fuzz targets**: `fuzz/` carries `odf_decode`, `path_split`,
   `router_match`, `config_extract`. The spec's `odf_patch` (patch-op
   depth/count limits), `rustfn_shim` (decoder bounds) and `stream_range`
   (reattach arithmetic) targets do not exist yet; the corresponding unit
   pins cover the happy paths and the named refusals (`MAX_DEPTH`,
   `MAX_REFS`, `RawBuf` bounds).
3. **Search query-length cap** (Section 4): not a named constant; the rate
   limit + result cap are the live controls.
4. **`.oxide/records` gitignore**: covered at this repo's roots by the
   `/.oxide/` entries; the create-oxide scaffold's generated gitignore
   must carry the same line — a release-checklist verification, not a code
   change.
5. **`odf_patch` prototype pollution**: the decoder rejects `__proto__` /
   `constructor` keys and enforces the depth/op-count limits
   (`MAX_DEPTH = 512`, `MAX_REFS`) — the unit pins hold the named
   rejections; the dedicated fuzz target (item 2) is the remaining
   hardening step.

## Sign-off

| Surface | Reviewer | Verdict | Date |
| --- | --- | --- | --- |
| live / stream / rust-fn / search / replay / image / deploy | — | draft | 2026-11-16 |

Draft prepared by the v0.2 release wave (R05). Every "pinned by" cell names
a test that exists at HEAD; every gap is actionable and scoped. The review
is complete for sign-off when each gap is either fixed or explicitly
accepted in `docs/deviations.md`.
