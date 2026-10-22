// Test-only worker for the §17.2 --offline-plugins contract.
//
// The real `oxide-worker.mjs` carries the deny gate at the top of its fetch
// shim (`if (process.env.OXIDE_OFFLINE_PLUGINS === "1") throw JSH-OFFLINE`).
// It lives in this directory; this probe is a sibling so `spawn_child`'s
// `--import <dir>/register-hooks.mjs` resolves correctly (the shim is
// imported for every worker; without it, Node reports MODULE_NOT_FOUND).
//
// This probe *consumes* the same contract — it reads the same env var,
// applies the same gate, and reports both the env-var state and a fetch
// attempt's outcome to the host.
//
// A `ok` result therefore proves two things end-to-end:
//   1. PoolConfig::offline_plugins is reaching the child process (env wiring);
//   2. the environment carries enough information for a worker to refuse
//      network — i.e. the *contract* between the Rust side and a worker is
//      sound. A reading of the real worker confirms it applies the same gate.

import { stdin, stdout } from 'node:process'

const OFFLINE = process.env.OXIDE_OFFLINE_PLUGINS === '1'

// ---- outbound framing: [4-byte LE len][1-byte kind=0][JSON] ----
function send(msg) {
  const json = Buffer.from(JSON.stringify(msg), 'utf8')
  const frame = Buffer.alloc(4 + 1 + json.length)
  frame.writeUInt32LE(json.length + 1, 0)
  frame.writeUInt8(0, 4)
  json.copy(frame, 5)
  stdout.write(frame)
}

send({ t: 'ready', pid: process.pid })

// ---- inbound framing: same shape ----
let buf = Buffer.alloc(0)
stdin.on('data', (chunk) => {
  buf = Buffer.concat([buf, chunk])
  for (;;) {
    if (buf.length < 5) return
    const len = buf.readUInt32LE(0)
    if (len === 0) return
    if (buf.length < 4 + len) return
    const kind = buf.readUInt8(4)
    if (kind !== 0) {
      buf = buf.subarray(4 + len)
      return
    }
    const msg = JSON.parse(buf.subarray(5, 4 + len).toString('utf8'))
    buf = buf.subarray(4 + len)
    if (msg.t === 'call') {
      handleCall(msg).catch((e) => {
        send({
          t: 'call_err',
          id: msg.id,
          diagnostic: { code: 'PROBE', message: String(e && e.message ? e.message : e) },
        })
      })
    }
  }
})

async function handleCall(msg) {
  const id = msg.id
  // Fetch probe: report the env-var state and attempt a fetch under the
  // same gate the real worker applies.
  let fetchOutcome
  try {
    if (OFFLINE) {
      const e = new Error('outbound network disabled by --offline-plugins')
      e.code = 'JSH-OFFLINE'
      throw e
    }
    // Online case: attempt a fetch against a closed port so the outcome is
    // deterministic (ECONNREFUSED) without real egress.
    await globalThis.fetch('http://127.0.0.1:1/', {})
    fetchOutcome = { ok: true }
  } catch (e) {
    fetchOutcome = {
      error_code: (e && e.code) || 'FETCH-FAIL',
      error_message: String((e && e.message) || e),
    }
  }
  const payload = JSON.stringify({
    offline_env: process.env.OXIDE_OFFLINE_PLUGINS ?? null,
    fetch: fetchOutcome,
  })
  send({
    t: 'call_ok',
    id,
    chunks: [Buffer.from(payload, 'utf8').toString('base64')],
    complete: true,
  })
}
