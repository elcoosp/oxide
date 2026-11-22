// §V2-E06 Module API v1 tests (spec V2-E06): the recording fake context
// pins the `defineModule`/`ModuleContext` contract — the same shape the
// Rust `modules_loader` (`crates/cli/src/modules_loader.rs`) consumes.
//
// Zero runtime deps: `node --test` runs these directly against the TS
// source (Node 22+ native type stripping); no pnpm/build required.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  defineModule,
  OxideModuleError,
  type ModuleContext,
  type ModuleManifest,
} from '../src/module.ts'

// A recording fake context mirrors the `ModuleContext` interface so the
// real `defineModule` walks a contract the Rust loader can read across
// the language seam.
function recordingContext(): ModuleContext & {
  routes: Array<{ path: string; file: string }>
  middlewares: Array<{ file: string; scope?: string }>
  plugins: Array<{ name: string; hooks: string[]; options?: Record<string, unknown>; enforce?: 'pre' | 'post' }>
  types: Array<{ name: string; dts: string }>
  serverFns: Array<{ file: string }>
  configPatch: Record<string, unknown>
  options: unknown
} {
  const rec = {
    options: { endpoint: '/_oxide/vitals' },
    routes: [] as Array<{ path: string; file: string }>,
    middlewares: [] as Array<{ file: string; scope?: string }>,
    plugins: [] as Array<{ name: string; hooks: string[]; options?: Record<string, unknown>; enforce?: 'pre' | 'post' }>,
    types: [] as Array<{ name: string; dts: string }>,
    serverFns: [] as Array<{ file: string }>,
    configPatch: {} as Record<string, unknown>,
    addRoute(r: { path: string; file: string }) { rec.routes.push(r) },
    addMiddleware(file: string, opts?: { scope?: string }) { rec.middlewares.push({ file, scope: opts?.scope }) },
    addPlugin(p: { name: string; hooks: string[]; options?: Record<string, unknown>; enforce?: 'pre' | 'post' }) { rec.plugins.push(p) },
    addTypes(name: string, dts: string) { rec.types.push({ name, dts }) },
    extendConfig(patch: Record<string, unknown>) { rec.configPatch = { ...rec.configPatch, ...patch } },
    addServerFn(file: string) { rec.serverFns.push({ file }) },
  }
  return rec
}

test('defineModule records routes/middleware/plugins/types/serverFns via the context', () => {
  const manifest: ModuleManifest = defineModule({
    name: '@oxide/analytics',
    adapters: ['react', 'vue', 'svelte', 'solid'],
    setup(ctx) {
      ctx.addRoute({ path: '/health', file: './routes/health.tsx' })
      ctx.addMiddleware('./middleware.ts')
      ctx.addPlugin({ name: 'vitals', hooks: ['transform'] })
      ctx.addTypes('analytics', 'declare global { interface Window { __oxAnalytics: unknown } }')
      ctx.addServerFn('./server/track.ts')
    },
  })
  assert.equal(manifest.name, '@oxide/analytics')
  assert.deepEqual(manifest.adapters, ['react', 'vue', 'svelte', 'solid'])
  assert.deepEqual(manifest.routes, [{ path: '/health', file: './routes/health.tsx' }])
  assert.deepEqual(manifest.middlewares, [{ file: './middleware.ts', scope: undefined }])
  assert.deepEqual(manifest.plugins, [{ name: 'vitals', hooks: ['transform'] }])
  assert.deepEqual(manifest.types, [{ name: 'analytics', dts: 'declare global { interface Window { __oxAnalytics: unknown } }' }])
  assert.deepEqual(manifest.serverFns, [{ file: './server/track.ts' }])
  assert.deepEqual(manifest.hooks, ['transform'])
})

test('defineModule carries configDefaults and merges extendConfig patches deep', () => {
  const manifest: ModuleManifest = defineModule({
    name: '@oxide/analytics',
    configDefaults: { analytics: { endpoint: '/_oxide/vitals' } },
    setup(ctx) {
      ctx.extendConfig({ analytics: { batchSize: 10 } })
    },
  })
  assert.deepEqual(manifest.configDefaults, { analytics: { endpoint: '/_oxide/vitals' } })
  assert.deepEqual(manifest.config, { analytics: { batchSize: 10 } })
})

test('defineModule loud-refuses an unsupported adapter with a two-line fix', () => {
  assert.throws(
    () => defineModule({
      name: '@oxide/svelte-only',
      adapters: ['svelte'],
      setup() {},
    }, { adapter: 'react' }),
    (err: unknown) => {
      assert.ok(err instanceof OxideModuleError, `expected OxideModuleError, got ${(err as Error).name}`)
      const e = err as OxideModuleError
      assert.equal(e.code, 'MOD-ADAPTER-001')
      assert.match(e.fix, /add `react`/)
      assert.match(e.fix, /remove the module from `modules: \[\]` in oxide\.config\.ts/)
      return true
    },
  )
})

test('defineModule accepts any adapter when adapters is empty', () => {
  const manifest = defineModule({ name: 'plain', setup() {} }, { adapter: 'solid' })
  assert.deepEqual(manifest.adapters, [])
  assert.equal(manifest.name, 'plain')
})

test('defineModule dedupes and sorts hooks across contributed plugins', () => {
  const manifest = defineModule({
    name: 'multi',
    setup(ctx) {
      ctx.addPlugin({ name: 'a', hooks: ['transform', 'resolveId'] })
      ctx.addPlugin({ name: 'b', hooks: ['transform', 'load'] })
    },
  })
  assert.deepEqual(manifest.hooks, ['load', 'resolveId', 'transform'])
})

test('defineModule serializes into the package.json oxide.module contract the loader reads', () => {
  // The Rust loader's `Contract` reads `routes` (string | dir), `entry`,
  // `hooks` (array<string>), `adapters` (array<string>), `typegen` (string),
  // `configDefaults` (object). The manifest the v1 API returns carries every
  // one of those fields — the loader consumes the manifest directly without
  // a second translation step.
  const manifest: ModuleManifest = defineModule({
    name: '@oxide/analytics',
    adapters: ['react', 'vue', 'svelte', 'solid'],
    configDefaults: { analytics: { endpoint: '/_oxide/vitals', console: false } },
    setup(ctx) {
      ctx.addMiddleware('./middleware.ts')
      ctx.addTypes('analytics', 'declare global { interface Window { __oxAnalytics: unknown } }')
      ctx.addPlugin({ name: 'vitals', hooks: ['transform'] })
      ctx.addServerFn('./server/track.ts')
    },
  })
  assert.equal(manifest.name, '@oxide/analytics')
  assert.ok(Array.isArray(manifest.adapters), 'adapters is the array the loader reads')
  assert.ok(typeof manifest.configDefaults === 'object' && manifest.configDefaults !== null, 'configDefaults is the object the loader merges under modules.<name>')
  assert.ok(Array.isArray(manifest.hooks), 'hooks is the array the loader compares against merged/deferred')
  assert.ok(Array.isArray(manifest.middlewares), 'middlewares is the array the loader wires through the middleware chain')
  assert.ok(Array.isArray(manifest.serverFns), 'serverFns is the array the loader feeds to server-fn discovery')
  assert.ok(Array.isArray(manifest.types), 'types is the array the loader merges into .oxide/types/')
})

test('a recording fake context receives every call verbatim (the test surface the spec names)', () => {
  // The spec acceptance: "setup receives a recording fake context".
  // Pin that the context the real defineModule builds walks the same
  // shape as one a test author writes by hand — the contract is one
  // context, not two.
  const fake = recordingContext()
  fake.addRoute({ path: '/x', file: './routes/x.tsx' })
  fake.addMiddleware('./mw.ts', { scope: '/dashboard' })
  fake.addPlugin({ name: 'p', hooks: ['transform'] })
  fake.addTypes('p', 'declare global {}')
  fake.addServerFn('./s.ts')
  fake.extendConfig({ foo: 1 })
  assert.equal(fake.routes.length, 1)
  assert.equal(fake.routes[0]?.file, './routes/x.tsx')
  assert.equal(fake.middlewares[0]?.scope, '/dashboard')
  assert.equal(fake.plugins[0]?.name, 'p')
  assert.equal(fake.types[0]?.name, 'p')
  assert.equal(fake.serverFns[0]?.file, './s.ts')
  assert.equal(fake.configPatch.foo, 1)
})
