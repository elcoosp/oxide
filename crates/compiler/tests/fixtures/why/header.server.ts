export async function load({ request }) {
  const ua = await request.headers.get('user-agent');
  return { ua };
}
