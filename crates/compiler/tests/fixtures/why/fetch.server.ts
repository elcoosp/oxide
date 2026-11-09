export async function load() {
  const res = await fetch('https://example.com/api');
  const data = await res.json();
  return { data };
}
