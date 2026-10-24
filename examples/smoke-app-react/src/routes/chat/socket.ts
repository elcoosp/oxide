// §9.6: a WebSocket route. `open` greets, `message` echoes back, `close`
// records the disconnect. The Rust layer owns the upgrade/framing; this is
// the per-phase JS handler.
import { defineSocket } from '@oxide-fw/core/server'

export const socket = defineSocket({
  open({ send, id }: { send: (d: string) => void; id: string }) {
    // The socket's own id rides in the welcome so a client can be
    // targeted by a peer's sendTo without a presence round-trip.
    send(JSON.stringify({ type: 'welcome', id }))
  },
  message({ data, send, broadcast, sendTo, clients }: {
    data?: unknown
    send: (d: string) => void
    broadcast: (d: string) => void
    sendTo: (id: string, d: string) => void
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
    // {"type":"sendto","to":"<id>","text":"..."} targets one socket.
    if (parsed?.type === 'sendto' && typeof (parsed as { to?: unknown }).to === 'string') {
      const target = (parsed as { to: string }).to
      const text = (parsed as { text?: string }).text
      sendTo(target, JSON.stringify({ type: 'direct', to: target, text }))
      return
    }
    // Echo the inbound text back, wrapped.
    send(JSON.stringify({ type: 'echo', payload: data }))
  },
  close() {
    // Best-effort cleanup; state lives in the worker's per-socket record.
  },
})
