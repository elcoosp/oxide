// §8.4 loading boundaries: render a route's `loading.tsx`/`.vue` component
// into an overlay element while a client-side navigation's `/__data` fetch
// is in flight. Uses Vue's `createApp` (client-side render, not SSR) so
// reactivity and lifecycle hooks work. Disposer unmounts the app.
import type { Disposer } from '../../core/src/types.ts'

interface VueLike {
  createApp(component: unknown, props?: Record<string, unknown>): {
    mount(container: Element): unknown
    unmount(): void
  }
}

let cachedVue: VueLike | undefined
async function loadVue(): Promise<VueLike> {
  if (cachedVue) return cachedVue
  const mod = (await import('vue')) as unknown as VueLike & { default?: VueLike }
  cachedVue = (mod.default ?? mod) as VueLike
  return cachedVue
}

export function vueRenderLoading(
  overlay: Element,
  _input: { url: string; route: string; params: Record<string, string | string[]>; data: unknown },
  Component: unknown,
): Disposer {
  let app: { unmount(): void } | undefined
  let cancelled = false
  void loadVue()
    .then((vue) => {
      if (cancelled) return
      app = vue.createApp(Component, {})
      app.mount(overlay)
    })
    .catch((e) => {
      console.error?.('oxide: renderLoading (vue) failed', e)
    })
  return () => {
    cancelled = true
    try {
      app?.unmount()
    } catch {
      // best-effort
    }
    app = undefined
  }
}
