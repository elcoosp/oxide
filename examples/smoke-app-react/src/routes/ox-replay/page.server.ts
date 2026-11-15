// §V2-F15 fixture: a loader whose outputs ride every non-deterministic
// source the record-and-replay contract pins — a wall-clock read, a
// random draw, a UUID mint, and an outbound `fetch` to the app's own
// health endpoint (through the SSRF-guarded op path; the fixture's config
// allow-lists 127.0.0.1). Recording captures the fetch response and the
// sequences; a replay plays them back, so the page re-renders
// byte-identically WITHOUT the network. The origin rides an env so the
// record and the replay can use the same absolute URL even when the two
// servers run on different ports (the worker's replay matcher matches by
// URL — a mismatch is an RPL-002 divergence, which is exactly what a
// real re-execution under different origins should report).
export const load = async () => {
  const now = Date.now()
  const rand = Math.random()
  const uuid = crypto.randomUUID()
  const origin = process.env.OXIDE_REPLAY_SELF_ORIGIN ?? ''
  let fetchStatus = 0
  let fetchBody = ''
  if (origin) {
    try {
      const r = await fetch(`${origin}/_oxide/health`)
      fetchStatus = r.status
      fetchBody = await r.text()
    } catch {
      fetchBody = 'fetch failed'
    }
  }
  return { now, rand, uuid, fetchStatus, fetchBody }
}
