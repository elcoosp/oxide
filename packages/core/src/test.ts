// In-process test helper (spec §16.6): exercises the full stack with
// `app.fetch(request)` — no network, no spawned processes. Node 24 provides
// the global Request/Response pair.

export interface TestAppRequest {
  method?: string
  url: string
  headers?: Record<string, string> | Headers
  body?: string | ArrayBuffer | FormData | ReadableStream<Uint8Array> | URLSearchParams | Uint8Array<ArrayBuffer>
}

export interface TestApp {
  /** Send a request through the app handler in-process. */
  fetch(input: Request | string | TestAppRequest): Promise<Response>
}

export type TestHandler = (req: Request) => Response | Promise<Response>

/**
 * Wrap a request handler (the same handler shape the Rust pipeline dispatches
 * to) into a fetch-like test client:
 *
 *   const app = createTestApp((req) => new Response('hi'))
 *   const res = await app.fetch({ method: 'POST', url: '/?/comment', body: 'x' })
 *
 * `Request` instances pass through untouched; strings become GET requests;
 * object input is normalized into a `Request` (a body without an explicit
 * method defaults to POST).
 */
export function createTestApp(handler: TestHandler): TestApp {
  return {
    async fetch(input: Request | string | TestAppRequest): Promise<Response> {
      const req = normalize(input)
      return await handler(req)
    },
  }
}

function normalize(input: Request | string | TestAppRequest): Request {
  if (typeof input === 'string') return new Request(input)
  if (typeof (input as Request).arrayBuffer === 'function' && input instanceof Request) return input
  const spec = input as TestAppRequest
  const headers = spec.headers instanceof Headers ? spec.headers : new Headers(spec.headers)
  const method = (spec.method ?? (spec.body !== undefined ? 'POST' : 'GET')).toUpperCase()
  const init: RequestInit & { duplex?: 'half' } = {
    method,
    headers,
    body: method === 'GET' || method === 'HEAD' ? undefined : spec.body,
  }
  if (init.body instanceof ReadableStream) init.duplex = 'half'
  return new Request(spec.url, init)
}
