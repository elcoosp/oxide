// §9.6: an SSE endpoint. Returns a Response whose body is a ReadableStream;
// the pipeline streams it (no buffering) so events reach the client as they
// are produced.
export function GET(): Response {
  const enc = new TextEncoder()
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      let n = 0
      const send = (): void => {
        if (n >= 3) {
          controller.close()
          return
        }
        controller.enqueue(enc.encode(`data: tick ${n}\n\n`))
        n += 1
        setTimeout(send, 30)
      }
      send()
    },
  })
  return new Response(stream, {
    status: 200,
    headers: {
      'content-type': 'text/event-stream',
      'cache-control': 'no-cache',
    },
  })
}
