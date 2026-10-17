// §8.1 middleware.ts / §8.5: scoped by directory. Runs before the loader and
// the render. Two behaviours exercised here:
//   * inject `locals.user` — the loader for a route under this directory sees it;
//   * block an unauthenticated access with a redirect when `?gated=1` is set.
export function middleware(request: Request, ctx: { locals: Record<string, unknown> }) {
  const url = new URL(request.url)
  // A redirect short-circuits the pipeline.
  if (url.searchParams.get('gated') === '1') {
    return { redirect: '/login', status: 307 }
  }
  // Inject a locals value every loader under this directory sees.
  return { locals: { ...ctx.locals, user: 'ada', role: 'admin' } }
}
