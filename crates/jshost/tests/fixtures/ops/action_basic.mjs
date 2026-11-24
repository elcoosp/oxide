// V2-A08 parity fixture: a §9.3 action (form-first). Mirrors the BUG-04
// action envelope pins: cookies set through `event.cookies`, the form body
// decoded from `form_json`, params through `event.params`.
export const actions = {
  greet: async (event) => {
    event.cookies.set('sess', 'abc', { path: '/' });
    const form = Object.fromEntries(event.form.entries());
    return { hello: form.name ?? 'nobody', p: event.params.slug ?? null };
  },
};
