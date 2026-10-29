# Security policy

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✅        |

Oxide is pre-1.0. Security fixes land on the latest published minor release;
there are no long-term-support branches yet.

## Reporting a vulnerability

**Do not open a public issue, pull request, or discussion for a security
problem.**

Use GitHub's private vulnerability reporting:

  https://github.com/elcoosp/oxide/security/advisories/new

That channel is private between you and the maintainers, supports
coordinated disclosure, and produces a GHSA advisory that can be published
alongside the fix. If you cannot use it, open an issue asking a maintainer
to establish a private channel — without including any details of the
vulnerability in that issue.

Please include: affected version(s), a minimal reproduction, the impact you
believe it has, and any known mitigations. A proof of concept is welcome;
a weaponised exploit is not required and should not be attached.

## What to expect

| Stage | Target |
|-------|--------|
| Acknowledgement | 3 business days |
| Initial assessment (severity, affected range) | 10 business days |
| Fix or mitigation plan | 30 days for critical, 90 days for others |
| Public disclosure | Coordinated, 90 days from report by default |

We will credit you in the advisory unless you ask us not to. If a fix is not
possible within the target window we will tell you why and agree a new date
rather than let the report go silent.

## Scope

In scope: the Rust crates published as `oxide-fw-*`, the npm packages
published as `@oxide-fw/*`, and the `oxide` CLI.

Out of scope: vulnerabilities in third-party dependencies that are already
covered by an upstream advisory (report those upstream), and issues that
require the attacker to already control the build environment or the
machine running the dev server.

## Supply-chain posture

For context when assessing a report: dependency build scripts are denied by
default (`allowBuilds` in `pnpm-workspace.yaml`), `cargo audit` runs in CI,
and releases publish with npm provenance. See
[`PUBLISHING.md`](PUBLISHING.md) for the release process.
