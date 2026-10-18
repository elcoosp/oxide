// §9.6: a WebSocket route. `open` greets, `message` echoes back, `close`
// records the disconnect. The Rust layer owns the upgrade/framing; this is
// the per-phase JS handler.
import { defineSocket } from 'oxide/server'

export const socket = defineSocket({
  open({ send }: { send: (d: string) => void }) {
    send(JSON.stringify({ type: 'welcome' }))
  },
  message({ data, send }: { data?: unknown; send: (d: string) => void }) {
    // Echo the inbound text back, wrapped.
    send(JSON.stringify({ type: 'echo', payload: data }))
  },
  close() {
    // Best-effort cleanup; state lives in the worker's per-socket record.
  },
})
