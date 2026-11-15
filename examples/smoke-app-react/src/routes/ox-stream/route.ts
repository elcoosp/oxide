// §V2-F07 fixture: a resumable streaming endpoint. The handler marks its
// streamed response through the `oxide/server` `resumable` helper (sets
// `x-oxide-resumable: 1`), so the server tees the bytes into the stream
// store and stamps `x-oxide-stream: <id>` on the head. A client that drops
// mid-stream reattaches at `GET /__oxide/stream/<id>` with
// `Range: bytes=<n>-` and receives only the bytes it missed — the
// `stream_resume_reattach_delivers_the_missed_bytes` e2e pin reads the
// full stream, drops after 8 lines, and proves the concatenation is
// byte-identical to the un-dropped stream.
//
// The producer enqueues the twenty lines in two batches with no timer
// between them: the tee is pull-driven (the substrate's documented model),
// so a producer that stalls between batches only reaches the store while
// some consumer keeps pulling — an e2e whose first client dies mid-stream
// needs the whole body pulled before the drop to stay deterministic. The
// timed-producer shape (20 lines at 50 ms) is pinned V8-free at the
// substrate level (`reattach_stream_emits_backlog_then_live_tail_until_done`
// in `crates/server/tests/stream_tdd.rs`), where a live reader drives the
// tail.
import { resumable } from '@oxide-fw/core/server'

export function GET(): Response {
  const enc = new TextEncoder()
  const lines = (from: number, to: number): Uint8Array[] => {
    const out: Uint8Array[] = []
    for (let n = from; n < to; n += 1) out.push(enc.encode(`line ${n}\n`))
    return out
  }
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const c of lines(0, 10)) controller.enqueue(c)
      for (const c of lines(10, 20)) controller.enqueue(c)
      controller.close()
    },
  })
  return resumable(
    new Response(stream, {
      status: 200,
      headers: {
        'content-type': 'text/plain; charset=utf-8',
        'cache-control': 'no-cache',
      },
    }),
  )
}
