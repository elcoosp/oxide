// Oxide config helper (spec §14.1).
//
// `oxide.config.{ts,mjs,js}` files import `defineConfig` to get a typed
// wrapper around the config object. It's an identity function at runtime;
// its only job is to anchor the type so editors autocomplete the config
// surface and TS catches typos before they reach the CLI.
//
// Kept dependency-free so it can be imported from a bare config module
// without pulling the full `oxide` runtime (router, ODF decoder, client
// modules) into scope. The type is intentionally permissive: the CLI
// parses the config at build time, not at the TS level.

/** The config object shape `oxide.config.{ts,mjs,js}` exports. Kept
 *  permissive because the Rust-side schema is the source of truth; the
 *  TS type exists for editor ergonomics. Fields are optional and unknown
 *  extras are allowed. */
export interface OxideUserConfig {
  framework?: unknown
  routes?: { dir?: string; [k: string]: unknown }
  server?: Record<string, unknown>
  render?: Record<string, unknown>
  build?: Record<string, unknown>
  adapter?: string
  modules?: unknown[]
  [k: string]: unknown
}

/** Identity helper that anchors the config type in editors. */
export function defineConfig<T extends OxideUserConfig>(config: T): T {
  return config
}
