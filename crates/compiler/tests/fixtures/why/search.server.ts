export async function load({ searchParams }) {
  const q = await searchParams.get('q');
  return { q };
}
