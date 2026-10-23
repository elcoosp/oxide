// §9.6: a WebSocket route. `open` greets, `message` echoes back, `close`
// records the disconnect. The Rust layer owns the upgrade/framing; this is
// the per-phase JS handler.
import { defineSocket } from 'oxide/server'

export const socket = defineSocket({
  open({ send }: { send: (d: string) => void }) {
    send(JSON.stringify({ type: 'welcome' }))
  },
  message({ data, send, broadcast, clients }: {
    data?: unknown
    send: (d: string) => void
    broadcast: (d: string) => void
    clients: string[]
  }) {
    // `{"type":"broadcast"}` fans out to every other socket on this route;
    // the caller gets the live presence list back.
    let parsed: { type?: string } | undefined
    try { parsed = typeof data === 'string' ? JSON.parse(data) : undefined } catch { /* echo */ }
    if (parsed?.type === 'broadcast') {
      broadcast(JSON.stringify({ type: 'broadcast', payload: data }))
      send(JSON.stringify({ type: 'clients', clients }))
      return
    }
    // Echo the inbound text back, wrapped.
    send(JSON.stringify({ type: 'echo', payload: data }))
  },
  close() {
    // Best-effort cleanup; state lives in the worker's per-socket record.
  },
})
