// V2-A08 parity fixture: a §8.4 `route.ts`-shaped GET endpoint. The handler
// exercises the surface the op envelope carries: `request.url` (WHATWG URL +
// searchParams), `request.headers`, and a `Response` with its own headers.
export async function GET(request) {
  const url = new URL(request.url);
  const q = url.searchParams.get('q') ?? '';
  return new Response(JSON.stringify({ users: ['ada', 'grace'], q }), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}
