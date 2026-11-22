// @oxide/analytics — the module's server-fn (§V2-E06).
//
// A `track` server function the client calls to record a single vitals
// sample (the spec's §9.4 server-fn shape: `export async function track(...)`).
// The build's server-fn discovery walk picks this up via the module's
// `serverFns` declaration and mints it a stable `/_oxide/fn/<id>` URL.

export interface TrackInput {
  name: string
  value: number
  route: string
}

export async function track(input: TrackInput): Promise<{ ok: true }> {
  // The server side appends to the project's analytics sink (Prometheus
  // registry, a webhook, etc.). The default sink is a no-op — a project
  // wires its own via the `analytics.endpoint` configDefaults merge.
  void input
  return { ok: true }
}
