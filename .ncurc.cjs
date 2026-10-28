// @types/node must track the runtime version (engines.node = ">=24").
// 26.x types APIs Node 24 doesn't ship. Reject from bumps only.
// Deliberately does NOT force a version via pnpm.overrides — that's
// a separate "unify Node type versions" concern, not this pass.
module.exports = {
  target: 'latest',
  reject: ['@types/node'],
}
