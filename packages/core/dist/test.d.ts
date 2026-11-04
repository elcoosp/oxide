export interface TestAppRequest {
  method?: string;
  url: string;
  headers?: Record<string, string> | Headers;
  body?: string | ArrayBuffer | FormData | ReadableStream<Uint8Array> | URLSearchParams | Uint8Array<ArrayBuffer>;
}
export interface TestApp {
  /** Send a request through the app handler in-process. */
  fetch(input: Request | string | TestAppRequest): Promise<Response>;
}
export type TestHandler = (req: Request) => Response | Promise<Response>;
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
export declare function createTestApp(handler: TestHandler): TestApp;
//# sourceMappingURL=test.d.ts.map