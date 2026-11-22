// @oxide/analytics — the module's middleware (§V2-E06).
//
// Adds an `x-oxide-analytics` response header naming the configured
// endpoint so a client (or the oxide inspector) can discover the vitals
// sink without reading the project's config. The middleware is no-op when
// the endpoint is empty.

import type { AnalyticsOptions } from '../index.ts'

interface AnalyticsMiddlewareLocals {
  endpoint?: string
}

export const config = {
  render: 'static',
}

export default function middleware(_locals: AnalyticsMiddlewareLocals, opts?: AnalyticsOptions) {
  const endpoint = opts?.endpoint ?? '/_oxide/vitals'
  return {
    headers: { 'x-oxide-analytics': endpoint },
  }
}
