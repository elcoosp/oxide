export async function load({ cookies }) {
  const theme = await cookies.get('theme');
  return { theme };
}
