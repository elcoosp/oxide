// A comment and a string literal that mention the request-scoped names.
// Neither is a member access on a request object, so neither is a reason.
// cookies.get('theme') in a comment is still just prose.
const note = "locals can be mentioned in a string safely";
const other = 'searchParams also appear here';

export async function load() {
  return { note, other };
}
