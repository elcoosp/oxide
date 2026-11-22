// §V2-E06 Module API v1 — `defineModule` + `ModuleContext`.
//
// A module is a distributable bundle of routes, middleware, plugins,
// types, config defaults and server functions, declared through one
// function so the build (Rust `modules_loader`) reads exactly the same
// contract a framework adapter plugin already speaks. The TS half is
// the authoring surface; the Rust half (`Contract` / `ResolvedModule`
// / `ModuleManifest` in `crates/cli/src/modules_loader.rs` and
// `crates/plugin/src/lib.rs`) is the consumer.
//
// One contract, not two: the object `defineModule` returns is shaped so
// a package's `package.json#oxide.module` (written by a build hook or
// by the package author by hand) carries the same fields the loader
// reads today — `routes`, `entry`/`main`, `hooks`, `adapters`,
// `typegen`, `configDefaults` — plus the v1 contributions
// (`middlewares`, `serverFns`, `plugins`, `types`, `config`). The
// loader resolves those contributions through the EXISTING middleware
// chain, server-fn discovery and plugin registry rather than a second
// pipeline.
//
// Loud refusal on an unsupported adapter: `setup(ctx, options)` runs
// against a `ModuleContext` whose `adapters` declaration is checked
// against the host adapter. A mismatch raises an `OxideModuleError`
// with code `MOD-ADAPTER-001` and the two-line fix (the Rust loader
// mirrors it through `OxideError::with_fix`). The TS error surfaces at
// `defineModule` time so a module imported into a config file fails
// loudly at config load, not silently at request time.

/** A route the module contributes. `file` is package-relative. */
export interface ModuleRoute {
  /** URL path under `/modules/<name>/…`. A leading `/` is absolute. */
  path: string
  /** Package-relative file path (e.g. `./routes/page.tsx`). */
  file: string
}

/** A middleware the module contributes. */
export interface ModuleMiddleware {
  /** Package-relative file path (e.g. `./middleware.ts`). */
  file: string
  /** Optional scope; defaults to the module's own `/modules/<name>/` prefix. */
  scope?: string
}

/** A type declaration the module ships for the project's `.oxide/types/`. */
export interface ModuleTypes {
  /** Logical name (e.g. `analytics`, `tailwind`). */
  name: string
  /** The `.d.ts` body. */
  dts: string
}

/** A server-function file the module exposes for the fn discovery walk. */
export interface ModuleServerFn {
  /** Package-relative file path (e.g. `./server/track.ts`). */
  file: string
}

/** A plugin descriptor the module registers. */
export interface ModulePlugin {
  /** Plugin name (stable across builds). */
  name: string
  /** Hook labels: `transform`, `resolveId`, `load`, … (§15.2 table). */
  hooks: string[]
  /** Options passed to the plugin host. */
  options?: Record<string, unknown>
  /** `pre` | `post` | undefined (§15.1 ordering). */
  enforce?: 'pre' | 'post'
}

/**
 * The context `setup(ctx, options)` receives. Each `add*` method records
 * a contribution the build merges through an existing pipeline — the
 * `recording` fake in `test/module-api.test.ts` pins the shape so the
 * Rust loader's resolver matches it byte-for-byte.
 */
export interface ModuleContext {
  /** Contribute a route under `/modules/<name>/…`. */
  addRoute(r: ModuleRoute): void
  /** Contribute a middleware file. */
  addMiddleware(file: string, opts?: { scope?: string }): void
  /** Contribute a plugin. */
  addPlugin(p: ModulePlugin): void
  /** Contribute a `.d.ts` the build merges into `.oxide/types/`. */
  addTypes(name: string, dts: string): void
  /** Patch the project's resolved config (deep-merged under `modules.<name>`). */
  extendConfig(patch: Record<string, unknown>): void
  /** Contribute a server-function file. */
  addServerFn(file: string): void
  /** The resolved options object the module was created with. */
  readonly options: unknown
}

/** The adapter the host is running. */
export type ModuleAdapter = 'react' | 'vue' | 'svelte' | 'solid'

/** A module definition as `defineModule` accepts it. */
export interface ModuleDefinition<O = unknown> {
  /** Stable module name (the `package.json` `name` by convention). */
  name: string
  /** Adapters the module supports. Empty = any. */
  adapters?: Array<ModuleAdapter | string>
  /** Setup; runs once at config load against a fresh `ModuleContext`. */
  setup(ctx: ModuleContext, options: O): void | Promise<void>
  /** Default options merged under `modules.<name>` (configDefaults). */
  configDefaults?: Record<string, unknown>
}

/**
 * The serialized manifest `defineModule` returns — the object a build hook
 * writes to `package.json#oxide.module` so the Rust loader picks it up.
 * One contract: it extends (does not duplicate) the loader's
 * `Contract` / `ModuleManifest`.
 */
export interface ModuleManifest {
  name: string
  adapters: string[]
  routes: Array<{ path: string; file: string }>
  middlewares: Array<{ file: string; scope?: string }>
  plugins: Array<ModulePlugin>
  types: Array<{ name: string; dts: string }>
  serverFns: Array<{ file: string }>
  configDefaults: Record<string, unknown>
  config: Record<string, unknown>
  hooks: string[]
  /** The entry file the JS bridge loads (the package's `main` by convention). */
  entry?: string
  /** A relative `.d.ts` the build merges into `.oxide/types/`. */
  typegen?: string
}

/** A module-refusal error (code `MOD-ADAPTER-001` / `MOD-SETUP-001`). */
export class OxideModuleError extends Error {
  /** Stable code, mirrored in the Rust registry. */
  readonly code: string
  /** The two-line fix a user pastes into their config. */
  readonly fix: string
  constructor(code: string, message: string, fix: string) {
    super(`${code}: ${message}`)
    this.name = 'OxideModuleError'
    this.code = code
    this.fix = fix
  }
}

/** The runtime the build passes in. `adapter` is checked against `adapters`. */
export interface ModuleHost {
  /** The host adapter (`react` | `vue` | `svelte` | `solid`). */
  adapter?: string
}

/** Internal recorder the build walks after `setup` to read the manifest. */
interface RecordingContext<O> extends ModuleContext {
  options: O
  routes: ModuleRoute[]
  middlewares: ModuleMiddleware[]
  plugins: ModulePlugin[]
  types: ModuleTypes[]
  serverFns: ModuleServerFn[]
  configPatch: Record<string, unknown>
}

function newRecorder<O>(options: O): RecordingContext<O> {
  const rec: RecordingContext<O> = {
    options,
    routes: [],
    middlewares: [],
    plugins: [],
    types: [],
    serverFns: [],
    configPatch: {},
    addRoute(r) {
      rec.routes.push(r)
    },
    addMiddleware(file, opts) {
      rec.middlewares.push({ file, scope: opts?.scope })
    },
    addPlugin(p) {
      rec.plugins.push(p)
    },
    addTypes(name, dts) {
      rec.types.push({ name, dts })
    },
    extendConfig(patch) {
      rec.configPatch = deepMerge(rec.configPatch, patch)
    },
    addServerFn(file) {
      rec.serverFns.push({ file })
    },
  }
  return rec
}

function deepMerge(a: Record<string, unknown>, b: Record<string, unknown>): Record<string, unknown> {
  if (typeof a !== 'object' || a === null || Array.isArray(a)) return { ...b }
  if (typeof b !== 'object' || b === null || Array.isArray(b)) return { ...a, ...b }
  const out: Record<string, unknown> = { ...a }
  for (const [k, v] of Object.entries(b)) {
    if (v !== undefined && typeof v === 'object' && !Array.isArray(v) && typeof out[k] === 'object' && !Array.isArray(out[k])) {
      out[k] = deepMerge(out[k] as Record<string, unknown>, v as Record<string, unknown>)
    } else {
      out[k] = v
    }
  }
  return out
}

/**
 * Define a module (§V2-E06). Returns the serialized manifest the build
 * consumes — the SAME contract `crates/cli/src/modules_loader.rs` reads
 * from `package.json#oxide.module`, extended with the v1 contributions
 * (middlewares/plugins/types/serverFns/config) the loader now wires.
 *
 * The `host.adapter` is checked against `def.adapters`; a mismatch raises
 * `OxideModuleError` with code `MOD-ADAPTER-001` and a two-line fix so a
 * module imported into a project it does not support fails loudly at
 * config load. An empty `adapters` array (the default) supports every
 * adapter.
 *
 * @example
 * ```ts
 * const manifest = defineModule({
 *   name: '@oxide/analytics',
 *   adapters: ['react', 'vue', 'svelte', 'solid'],
 *   configDefaults: { analytics: { endpoint: '/_oxide/vitals' } },
 *   setup(ctx) {
 *     ctx.addMiddleware('./middleware.ts')
 *     ctx.addServerFn('./server/track.ts')
 *     ctx.addTypes('analytics', 'declare global { interface Window { __oxAnalytics: unknown } }')
 *   },
 * })
 * ```
 */
export function defineModule<O>(
  def: ModuleDefinition<O>,
  host: ModuleHost = {},
): ModuleManifest {
  if (def.adapters && def.adapters.length > 0) {
    const a = host.adapter
    if (a && !def.adapters.includes(a)) {
      throw new OxideModuleError(
        'MOD-ADAPTER-001',
        `module \`${def.name}\` supports adapters [${def.adapters.join(', ')}] but the host adapter is \`${a}\``,
        `add \`${a}\` to the module's \`adapters\` declaration, or\n  remove the module from \`modules: []\` in oxide.config.ts`,
      )
    }
  }
  const rec = newRecorder(def.configDefaults ?? ({} as O))
  // The setup contract: synchronous or async. The build awaits it; the
  // manifest is the recorded state when `setup` returns.
  void def.setup(rec, rec.options)
  const hooks = new Set<string>()
  for (const p of rec.plugins) for (const h of p.hooks) hooks.add(h)
  return {
    name: def.name,
    adapters: (def.adapters ?? []).map(String),
    routes: rec.routes.slice(),
    middlewares: rec.middlewares.slice(),
    plugins: rec.plugins.slice(),
    types: rec.types.slice(),
    serverFns: rec.serverFns.slice(),
    configDefaults: { ...(def.configDefaults ?? {}) },
    config: rec.configPatch,
    hooks: [...hooks].sort(),
  }
}
