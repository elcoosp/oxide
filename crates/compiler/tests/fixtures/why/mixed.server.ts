// Multi-reason route: a params binding, its member use, a header read and
// an uncached fetch. `infer_reasons` must report every hit in pre-order.
export async function load({ cookies, request }) {
  const theme = await cookies.get('theme');
  const ua = await request.headers.get('user-agent');
  const res = await fetch('https://example.com/api');
  return { theme, ua, res };
}
