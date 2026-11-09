// Two distinct cookie reads on two different lines: every hit is reported.
export async function load({ cookies }) {
  const a = await cookies.get('a');
  const b = await cookies.get('b');
  return { a, b };
}
