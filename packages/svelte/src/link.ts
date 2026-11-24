// V2-E02 — the Svelte adapter's typed navigation re-exports. An SFC
// cannot re-export module members, so the typed navigation entries app
// code imports live in this plain TS module (the component ships as the
// `./link` subpath; the re-exports ride the package root). The same names
// the generated overloads declare, the same shapes the React adapter's
// `link.ts` re-exports.
export { hrefFor as href, goto, parseSearch } from '@oxide-fw/core/client'
export type { HrefOpts, GotoOpts, SearchParseResult } from '@oxide-fw/core/client'
