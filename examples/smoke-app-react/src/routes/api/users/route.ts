// §8.1 route.ts: a per-method Request → Response handler. GET returns JSON;
// POST echoes the request body back with a 201. The pipeline dispatches this
// through the worker's `endpoint:run` op before the render phase, so a
// `route.ts` bypasses loaders, layouts and the page component entirely.
export function GET(request: Request): Response {
  const url = new URL(request.url)
  const name = url.searchParams.get('name') ?? 'world'
  return new Response(JSON.stringify({ hello: name, method: 'GET' }), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  })
}

export async function POST(request: Request): Promise<Response> {
  const text = await request.text()
  return new Response(JSON.stringify({ echo: text, method: 'POST' }), {
    status: 201,
    headers: { 'content-type': 'application/json' },
  })
}
