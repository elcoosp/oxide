// V2-A08 parity fixture: a §8.1 middleware that short-circuits with a
// redirect directive — the `short: true` envelope shape (status, location
// header, redirect field).
export function middleware() {
  return { redirect: '/login', status: 302 };
}
