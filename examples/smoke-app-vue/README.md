# smoke-app-vue

Vue counterpart of `examples/smoke-app`. Same shape, different framework:
this fixture exists to prove the CLI build pipeline (`oxide build` →
`dist/` → `oxide start`) is framework-neutral, not React-specific. The
conformance suite already runs the same assertion set through all four
adapters in-process; this fixture runs one of them through the *real* CLI.
