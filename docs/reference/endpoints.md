<!-- GENERATED FILE — `oxide docs generate` (spec v0.2 §V2-E11). DO NOT EDIT.
Regenerate: cargo run -p oxide-fw-cli -- docs generate
Drift gate: crates/cli/tests/docs_tdd.rs -->

# Endpoint map

Every HTTP surface the framework serves (Appendix V2.J), generated from the `oxide_fw_server::ENDPOINT_MAP` data the server crate ships. The plane rule is structural: `/__oxide/*` is the **data plane** (the build-header check guards it, A04), `/_oxide/*` is the **ops plane**, and the dev-only surfaces are dev-token guarded (D41).

## Endpoints

| path | method | plane | WP | purpose |
| --- | --- | --- | --- | --- |
| `/__data?route=` | GET | data | existing, A04, A05 | Route loader data as ODF; now gated by middleware and carries `x-oxide-build` |
| `/__oxide/live` | GET (SSE) | data | F05 | Live loader snapshots and patches |
| `/__oxide/stream/<id>` | GET | data | F07 | Reattach to a resumable stream with `Range` |
| `/__oxide/state` | GET | data | existing | Hydration state for tooling |
| `/_oxide/fn/<id>` | POST | ops | existing, F08 | Server functions (TypeScript and Rust) |
| `/_oxide/search` | GET | ops | F09 | Full-text query |
| `/_oxide/nav?from=` | GET | ops | F13 | Aggregate next-route probabilities |
| `/_oxide/csp-report` | POST | ops | E04 | CSP violation reports |
| `/_oxide/health, /_oxide/ready` | GET | ops | existing, F12 | Liveness and readiness (level reported) |
| `/_oxide/{vitals,static,og,metrics,image,hmr}` | various | ops | existing | Unchanged |
| `/__oxide/open, /__oxide/cost` | GET | dev | E08, F17 | Open in editor; component cost (dev-token guarded) |
| `/__oxide/traces` | GET | dev | F21 | Recent-request trace feed over the §20 context (dev-token guarded) |
| `/__oxide` | GET | dev | existing, E08 | Inspector |
