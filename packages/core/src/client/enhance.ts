// Form enhance helper (spec §9.3): with JavaScript, the adapter's `enhance`
// intercepts the submit, calls the action over fetch, updates data in place,
// preserves focus/scroll (the adapter's job), and supports optimistic updates.
// Without JavaScript the same form POSTs natively — progressive enhancement.

import { decodeOdf } from './odf-decode.ts'

/** Structural form surface (real element or test fake). */
export interface EnhanceForm {
  addEventListener(type: string, fn: (e: unknown) => void): void
  removeEventListener(type: string, fn: (e: unknown) => void): void
  getAttribute(name: string): string | null
  hasAttribute(name: string): boolean
  dispatchEvent?(ev: { type: string; detail?: unknown }): boolean
}

export interface EnhanceOptions {
  fetch?: typeof fetch
  /** Extract the form body (default: `new FormData(form)`). */
  getFormData?: (form: EnhanceForm) => FormData
  /**
   * Optimistic hook (§9.3): runs BEFORE the request with the form data, so
   * the adapter can apply an optimistic update (rollback via onError).
   */
  onSubmit?(formData: FormData, form: EnhanceForm): void | Promise<void>
  /** Receive the parsed action result (form result data, §9.3). */
  applyResult?(result: unknown, form: EnhanceForm): void
  /** Failure path — rollback optimistic state, surface the error. */
  onError?(error: unknown, form: EnhanceForm): void
  /** Where results are dispatched when no applyResult is given. */
  baseLocation?: string
}

export interface EnhanceSubmitEvent {
  preventDefault(): void
}

/**
 * Wire progressive enhancement onto a form. Returns a disposer that removes
 * the submit listener. Actions are POST-only (§17.1); the body is FormData
 * for multipart forms and URLSearchParams otherwise.
 */
export function enhance(form: EnhanceForm, opts: EnhanceOptions = {}): () => void {
  const doFetch = opts.fetch ?? ((...a: Parameters<typeof fetch>) => globalThis.fetch(...a))
  const getFormData = opts.getFormData ?? ((f: EnhanceForm) => new FormData(f as HTMLFormElement))

  const onSubmit = async (e: EnhanceSubmitEvent): Promise<void> => {
    e.preventDefault()
    const formData = getFormData(form)
    const target = form.getAttribute('action') ?? (opts.baseLocation ?? '/')
    const multipart =
      (form.getAttribute('enctype') ?? '').includes('multipart/form-data') ||
      [...formData.values()].some((v) => typeof v === 'object' && v !== null)
    const body: FormData | URLSearchParams = multipart
      ? formData
      : new URLSearchParams([...formData.entries()].map(([k, v]) => [k, String(v)]))
    try {
      await opts.onSubmit?.(formData, form)
      const res = await doFetch(target, {
        method: 'POST',
        body,
        headers: { accept: 'application/oxide+odf, application/json, text/plain' },
      })
      if (!res.ok) throw new Error(`action failed: ${res.status}`)
      const result = await parseResult(res)
      if (opts.applyResult) opts.applyResult(result, form)
      else form.dispatchEvent?.({ type: 'ox:form-result', detail: result })
    } catch (err) {
      if (opts.onError) opts.onError(err, form)
      else form.dispatchEvent?.({ type: 'ox:form-error', detail: err })
    }
  }

  form.addEventListener('submit', onSubmit as (e: unknown) => void)
  return () => form.removeEventListener('submit', onSubmit as (e: unknown) => void)
}

async function parseResult(res: Response): Promise<unknown> {
  const header = res.headers.get('content-type') ?? ''
  const type = (header.split(';').at(0) ?? '').trim()
  if (type === 'application/oxide+odf' || type === 'application/octet-stream') {
    return decodeOdf(new Uint8Array(await res.arrayBuffer()))
  }
  if (type === 'application/json') return res.json()
  return res.text()
}
