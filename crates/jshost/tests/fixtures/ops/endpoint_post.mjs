// V2-A08 parity fixture: a §8.4 POST endpoint. Reads the request body as
// JSON (`request.json()`), echoes it back with a 201 — pins the body decode
// arm (`body_b64` → bytes) and the can-have-body rule (GET/HEAD carry none).
export async function POST(request) {
  const body = await request.json();
  return new Response(JSON.stringify({ created: body.name }), {
    status: 201,
    headers: { 'content-type': 'application/json' },
  });
}
