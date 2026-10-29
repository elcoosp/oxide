---
title: Security
description: How to report a vulnerability, and how dependency advisories are tracked.
order: 10
---

# Security

## Reporting a vulnerability

Do not open a public issue. Use GitHub's private vulnerability reporting:

<https://github.com/elcoosp/oxide/security/advisories/new>

`SECURITY.md` at the repository root is the full policy — supported versions,
response targets, scope. The short version: acknowledgement within 72 hours, a
remediation decision within 14 days, coordinated disclosure.

## Dependency advisories

`.cargo/audit.toml` is the decision record. Every suppression carries a
three-part rationale: what the advisory says, why it is not reachable from
this codebase, and what would unblock the fix.

The one currently suppressed is `rkyv 0.7.46`, pulled in through
`lightningcss → parcel_sourcemap`. Neither parent has a newer published
version, so the chain is hard-pinned. Oxide's two rkyv usages — the route-tree
archive and the graph artifact envelope — are both serialized by the build and
verified before deserialization (blake3 for the graph, a MAC for the route
tree). Neither accepts an attacker-supplied archive, which is the advisory's
attack vector.

`cargo audit` runs locally and exits non-zero on any new CVE-level advisory.
It is **not** wired into CI yet — that is a deliberate deferral, so that a
green CI baseline exists before the audit gate is added and any failure can be
attributed.

## Supply chain

Dependency build scripts are denied by default (`allowBuilds` in
`pnpm-workspace.yaml`). `cargo audit` is checked before every release. npm
publishes after the first use OIDC trusted publishing with provenance.
