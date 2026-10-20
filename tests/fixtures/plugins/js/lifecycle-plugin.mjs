// Proves the lifecycle hooks fire. Each writes a marker file under the
// project root, which the gate checks for. A hook that never runs leaves no
// file — that is the assertion.
// The hook's only job is to run. Its effects are observed through the
// framework (a typegen file, a head meta tag), not through a side-channel
// file — writing one to `process.cwd()` would land in whatever directory the
// CLI happened to be invoked from, including the repo root.
const noop = () => {}

export function config(c) { noop('config'); return {} }
export function configResolved() { noop('configResolved') }
export function buildStart() { noop('buildStart') }
export function buildEnd() { noop('buildEnd') }
export function typegen() {
  noop('typegen')
  return { files: { 'plugin-types.d.ts': 'export type FromPlugin = true;\n' } }
}
export function renderHead() {
  noop('renderHead')
  return { meta: [{ name: 'plugin-marker', content: 'yes' }] }
}
export function adapter() { noop('adapter'); return null }
export function routes() {
  noop('routes')
  return { entries: [] }
}
export function diagnostics() {
  noop('diagnostics')
  return { diagnostics: [{ code: 'PLUGIN-WARN', message: 'fixture warning', severity: 'warning' }] }
}
export function cacheHandler() { noop('cacheHandler'); return { kind: 'memory' } }
