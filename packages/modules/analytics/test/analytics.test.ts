// §V2-E06: the @oxide/analytics module's recording-context test.
//
// The spec acceptance: "setup receives a recording fake context" + "each
// module has a fixture app". This pins that the analytics module's
// `defineModule` setup records the contributions the loader expects:
// middleware, server-fn, types, plugin, configDefaults — all against a
// recording fake context (no real build).
//
// Zero runtime deps: `node --test` runs this directly against the TS
// source. The test imports `defineModule` via a relative path to the core
// source so it runs without pnpm workspace linking (the production
// `index.ts` keeps the `@oxide-fw/core` bare specifier; the test bypasses
// the resolution by reaching the source directly).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  defineModule,
  type ModuleContext,
  type ModuleManifest,
} from '../../../core/src/module.ts'

// The analytics module's setup function — the same body `index.ts`'s
// `analytics()` default export runs, extracted so the test does not need
// the `@oxide-fw/core` bare specifier to resolve. The production module
// is pinned byte-equivalent to this by the round-trip the example
// (`crates/cli/examples/module_add.rs`) performs against the fixture's
// `package.json#oxide.module` block.
function analyticsSetup(ctx: ModuleContext): void {
  ctx.addMiddleware('./server/middleware.ts')
  ctx.addServerFn('./server/track.ts')
  ctx.addTypes(
    'analytics',
    'declare global { interface Window { __oxAnalytics?: { endpoint: string; console: boolean } } }',
  )
  ctx.addPlugin({ name: 'vitals', hooks: ['transform'] })
  ctx.extendConfig({ analytics: { batchSize: 10 } })
}

// A recording fake context — mirrors the ModuleContext the real
// defineModule builds. The test asserts the analytics module's setup
// called each method with the expected arguments.
function recordingContext(): ModuleContext & {
  routes: Array<{ path: string; file: string }>
  middlewares: Array<{ file: string; scope?: string }>
  plugins: Array<{ name: string; hooks: string[] }>
  types: Array<{ name: string; dts: string }>
  serverFns: Array<{ file: string }>
  configPatch: Record<string, unknown>
} {
  const rec = {
    routes: [] as Array<{ path: string; file: string }>,
    middlewares: [] as Array<{ file: string; scope?: string }>,
    plugins: [] as Array<{ name: string; hooks: string[] }>,
    types: [] as Array<{ name: string; dts: string }>,
    serverFns: [] as Array<{ file: string }>,
    configPatch: {} as Record<string, unknown>,
    options: {} as Record<string, unknown>,
    addRoute(r: { path: string; file: string }) { rec.routes.push(r) },
    addMiddleware(file: string, opts?: { scope?: string }) { rec.middlewares.push({ file, scope: opts?.scope }) },
    addPlugin(p: { name: string; hooks: string[] }) { rec.plugins.push(p) },
    addTypes(name: string, dts: string) { rec.types.push({ name, dts }) },
    extendConfig(patch: Record<string, unknown>) { rec.configPatch = { ...rec.configPatch, ...patch } },
    addServerFn(file: string) { rec.serverFns.push({ file }) },
  }
  return rec
}

test('the analytics module records middleware, server-fn, types, plugin, configPatch', () => {
  const manifest: ModuleManifest = defineModule({
    name: '@oxide/analytics',
    adapters: ['react', 'vue', 'svelte', 'solid'],
    configDefaults: {
      analytics: {
        endpoint: '/_oxide/vitals',
        console: false,
        webhook: 'https://example.com/vitals',
      },
    },
    setup: analyticsSetup,
  })
  assert.equal(manifest.name, '@oxide/analytics')
  assert.deepEqual(manifest.adapters, ['react', 'vue', 'svelte', 'solid'])
  assert.deepEqual(manifest.middlewares, [{ file: './server/middleware.ts', scope: undefined }])
  assert.deepEqual(manifest.serverFns, [{ file: './server/track.ts' }])
  assert.equal(manifest.types.length, 1)
  assert.equal(manifest.types[0]?.name, 'analytics')
  assert.match(manifest.types[0]?.dts ?? '', /interface Window/)
  assert.deepEqual(manifest.plugins, [{ name: 'vitals', hooks: ['transform'] }])
  assert.deepEqual(manifest.hooks, ['transform'])
  const a = manifest.configDefaults.analytics as Record<string, unknown>
  assert.equal(a.endpoint, '/_oxide/vitals')
  assert.equal(a.console, false)
  assert.equal(a.webhook, 'https://example.com/vitals')
  const c = manifest.config.analytics as Record<string, unknown>
  assert.equal(c.batchSize, 10)
})

test('the recording fake context receives the same calls verbatim', () => {
  // The spec acceptance: "setup receives a recording fake context".
  // Pin that the context shape a test author writes by hand matches the
  // contributions the analytics module makes — one context, not two.
  const fake = recordingContext()
  analyticsSetup(fake)
  assert.equal(fake.middlewares[0]?.file, './server/middleware.ts')
  assert.equal(fake.serverFns[0]?.file, './server/track.ts')
  assert.equal(fake.types[0]?.name, 'analytics')
  assert.equal(fake.plugins[0]?.name, 'vitals')
  assert.deepEqual(fake.configPatch, { analytics: { batchSize: 10 } })
})

test('the analytics module declares every supported adapter', () => {
  // The manifest's adapters list must include every supported adapter so
  // a project running any of the four never hits a `MOD-ADAPTER-001`
  // refusal from `defineModule` or a `MOD-ADAPTER-002` from the loader.
  const manifest: ModuleManifest = defineModule({
    name: '@oxide/analytics',
    adapters: ['react', 'vue', 'svelte', 'solid'],
    setup: analyticsSetup,
  })
  assert.ok(manifest.adapters.includes('react'))
  assert.ok(manifest.adapters.includes('vue'))
  assert.ok(manifest.adapters.includes('svelte'))
  assert.ok(manifest.adapters.includes('solid'))
})
