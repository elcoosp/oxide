// Form enhance helper (spec §9.3): with JavaScript, the adapter's `enhance`
// intercepts the submit, calls the action over fetch, updates data in place,
// preserves focus/scroll (the adapter's job), and supports optimistic updates.
// Without JavaScript the same form POSTs natively — progressive enhancement.

import { decodeOdf } from './odf-decode.ts'
import { handleStale, withBuild } from './build.ts'
import type { OutboxQueue } from './outbox.ts'

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
  /**
   * V2-F14: the action outbox the offline submit rides. When the browser
   * reports it is offline, or the POST fails with the network-failure
   * shape (fetch throws a `TypeError`), the action is enqueued with an
   * `x-oxide-idem` idempotency key instead of erroring — the server's
   * idempotency stage answers the eventual replay, so the retried submit
   * inserts exactly once. The form is told with an `ox:form-queued`
   * event (`detail: { id, url }`). Only a urlencoded body can ride the
   * string-item outbox; a multipart form's parts have no string shape,
   * so it keeps today's failure path.
   */
  outbox?: OutboxQueue
  /**
   * The online probe the offline wire consults before spending the
   * fetch. Default: `navigator.onLine` when the platform reports it
   * explicitly (`false` means offline); every other environment — Node
   * tests, no `navigator` — counts as online. Injectable for tests.
   */
  isOnline?: () => boolean
}

export interface EnhanceSubmitEvent {
  preventDefault(): void
}

/** The accept header every action POST (live or queued) carries. */
const ACCEPT = 'application/oxide+odf, application/json, text/plain'

/**
 * Wire progressive enhancement onto a form. Returns a disposer that removes
 * the submit listener. Actions are POST-only (§17.1); the body is FormData
 * for multipart forms and URLSearchParams otherwise.
 */
export function enhance(form: EnhanceForm, opts: EnhanceOptions = {}): () => void {
  const doFetch = opts.fetch ?? ((...a: Parameters<typeof fetch>) => globalThis.fetch(...a))
  const getFormData = opts.getFormData ?? ((f: EnhanceForm) => new FormData(f as HTMLFormElement))

  const isOnline = (): boolean => {
    if (opts.isOnline) return opts.isOnline()
    // Only the platform's explicit offline answer counts: Node's
    // `navigator` has no `onLine` (undefined means "unknown", i.e. try
    // the network).
    return !(typeof navigator !== 'undefined' && navigator.onLine === false)
  }

  // The V2-F14 offline wire: enqueue the submit when an outbox is armed
  // and the body can ride it (a urlencoded string — the multipart parts
  // have no string shape to store). Returns true when the submit was
  // handled: either queued (the form heard `ox:form-queued`) or the
  // queue itself refused and the error was surfaced.
  const tryEnqueue = async (target: string, body: FormData | URLSearchParams): Promise<boolean> => {
    const outbox = opts.outbox
    if (!outbox || !(body instanceof URLSearchParams)) return false
    const headers = new Headers(withBuild({ headers: { accept: ACCEPT } }).headers)
    headers.set('content-type', 'application/x-www-form-urlencoded;charset=UTF-8')
    const record: Record<string, string> = {}
    headers.forEach((value, key) => {
      record[key] = value
    })
    try {
      const id = await outbox.enqueue({ url: target, method: 'POST', body: body.toString(), headers: record })
      form.dispatchEvent?.({ type: 'ox:form-queued', detail: { id, url: target } })
      return true
    } catch (queueErr) {
      surfaceError(queueErr)
      return true
    }
  }

  const surfaceError = (err: unknown): void => {
    if (opts.onError) opts.onError(err, form)
    else form.dispatchEvent?.({ type: 'ox:form-error', detail: err })
  }

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
    // The offline probe: a submit the browser already knows cannot reach
    // the network skips the doomed fetch and rides the outbox (the
    // optimistic hook never ran, so there is nothing to roll back).
    if (opts.outbox && !isOnline() && (await tryEnqueue(target, body))) return
    try {
      await opts.onSubmit?.(formData, form)
      // §V2-A04: the action POST is a data-plane request — advertise the
      // build id and hand a stale-generation 409 to the one-shot reload.
      const res = await doFetch(target, withBuild({
        method: 'POST',
        body,
        headers: { accept: ACCEPT },
      }))
      if (handleStale(res)) return // reload in flight; the action result is moot
      if (!res.ok) throw new Error(`action failed: ${res.status}`)
      const result = await parseResult(res)
      if (opts.applyResult) opts.applyResult(result, form)
      else form.dispatchEvent?.({ type: 'ox:form-result', detail: result })
    } catch (err) {
      // The network-failure shape (fetch throws a TypeError when the
      // request cannot leave the page): the action rides the outbox when
      // one is armed. A reached-but-failed response (4xx/5xx) is a live
      // error — the delivery rule never queues those.
      if (opts.outbox && err instanceof TypeError && (await tryEnqueue(target, body))) return
      surfaceError(err)
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
