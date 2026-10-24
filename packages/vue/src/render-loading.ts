// §8.4 loading boundaries: render a route's `loading.tsx`/`.vue` component
// into an overlay element while a client-side navigation's `/__data` fetch
// is in flight. Uses Vue's `createApp` (client-side render, not SSR) so
// reactivity and lifecycle hooks work. Disposer unmounts the app.
import type { Disposer } from '../../core/src/types.ts'

/** Runtime type guard for the Vue client module (see real-client.ts's
 * isVueClientLike — same shape check, same rationale). */
function isVueLikeForLoading(v: unknown): v is VueLike {
  if (v === null || typeof v !== 'object') return false
  return typeof Reflect.get(v, 'createSSRApp') === 'function'
}


interface VueLike {
  createApp(component: unknown, props?: Record<string, unknown>): {
    mount(container: Element): unknown
    unmount(): void
  }
}

let cachedVue: VueLike | undefined
async function loadVue(): Promise<VueLike> {
  if (cachedVue) return cachedVue
  const mod: unknown = await import('vue')
  if (!isVueLikeForLoading(mod)) throw new TypeError('vue: module has no createSSRApp')
  cachedVue = mod
  return cachedVue
}

export function vueRenderLoading(
  overlay: Element,
  _input: unknown,
  Component: unknown,
): Disposer {
  let app: { unmount(): void; mount(el: unknown): void } | undefined
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
