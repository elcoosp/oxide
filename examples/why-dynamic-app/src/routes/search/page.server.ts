// V2-F16 fixture route: a `searchParams` read makes the route dynamic
// until a `searchSchema` (E02) declares the keys as cache dimensions.
export async function load({ searchParams }) {
  const q = await searchParams.get('q')
  return { q }
}
