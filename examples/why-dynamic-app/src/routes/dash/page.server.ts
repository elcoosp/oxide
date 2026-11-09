// V2-F16 fixture route: reads `cookies`, so `oxide routes` reports the
// render mode as `dynamic` and `oxide why --route routes/dash` attaches
// two OX-DYN reasons (the destructured bind on line 4 and the
// `cookies.get` read on line 5).
export async function load({ cookies }) {
  const theme = await cookies.get('theme')
  return { theme }
}
