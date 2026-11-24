// V2-A08 parity fixture: a §8.1 middleware whose object directive propagates
// request-scoped locals to the loader (§8.1/§9.1). `ctx.params` pins the
// `(request, ctx)` call convention the Node worker uses.
export function middleware(request, ctx) {
  const name = ctx.params.name ?? '';
  return { locals: { greet: 'hi ' + name } };
}
