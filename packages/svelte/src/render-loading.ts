// §8.4 loading boundaries: render a route's `loading.svelte` component into
// an overlay element while a client-side navigation's `/__data` fetch is in
// flight. Uses Svelte 5's client `mount` (no hydrate — the overlay is
// empty). Disposer unmounts.
import type { Disposer } from '@oxide-fw/core/types'

interface SvelteLike {
  mount(
    component: unknown,
    opts: { target: Element; props?: Record<string, unknown>; hydrate?: boolean },
  ): unknown
  unmount(component: unknown): void
}

let cachedSvelte: SvelteLike | undefined
async function loadSvelte(): Promise<SvelteLike> {
  if (cachedSvelte) return cachedSvelte
  const mod = await import('svelte')
  cachedSvelte = mod
  return mod
}

export function svelteRenderLoading(
  overlay: Element,
  _input: unknown,
  Component: unknown,
): Disposer {
  let mounted: unknown
  let cancelled = false
  void loadSvelte()
    .then((svelte) => {
      if (cancelled) return
      mounted = svelte.mount(Component, { target: overlay, props: {}, hydrate: false })
    })
    .catch((e) => {
      console.error?.('oxide: renderLoading (svelte) failed', e)
    })
  return () => {
    cancelled = true
    if (mounted === undefined) return
    void loadSvelte()
      .then((svelte) => svelte.unmount(mounted))
      .catch(() => {
        // best-effort
      })
    mounted = undefined
  }
}
