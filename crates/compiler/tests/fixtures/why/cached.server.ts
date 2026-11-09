// The fetch happens inside a `'use cache'` function, so it is not an
// UncachedFetch reason (V2-F16: only `fetch(` calls outside a cached
// function count).
export async function load() {
  const data = await getCached();
  return { data };
}

async function getCached() {
  'use cache';
  const res = await fetch('https://example.com/api');
  return res.json();
}
