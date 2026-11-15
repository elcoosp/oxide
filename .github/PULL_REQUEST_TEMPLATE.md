## Summary

<!-- What changes, and why. Link the issue if there is one. -->

## Type

- [ ] Bug fix
- [ ] Feature
- [ ] Refactor (no behaviour change)
- [ ] Documentation
- [ ] CI / build / dependencies
- [ ] Breaking change (call it out in the description)

## Verification

Every box must be ticked before review. CI enforces the same gates.

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo nextest run --workspace --no-fail-fast`
- [ ] `cargo audit` (exit 0)
- [ ] `pnpm -r --filter '!./packages/oxide-website' --filter '!./examples/*' run build`
- [ ] Adapter tests (core, react, vue, svelte, solid)
- [ ] `pnpm -C conformance test`
- [ ] `./scripts/check-launch-content.sh`

## Notes for reviewers

<!-- Non-obvious decisions, trade-offs, follow-ups deferred. -->
