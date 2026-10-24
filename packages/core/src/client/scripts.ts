// `<Script>` loading strategies (spec §16.6).
//
// The server renders a `<script data-ox-script data-ox-strategy="...">` tag
// (or a placeholder for a src script) and this client module schedules it:
//
//   - `defer`   — load after the document is parsed (the default). A `src`
//                 script is injected with `defer`; inline code runs at
//                 `DOMContentLoaded`.
//   - `idle`    — load in an idle callback (`requestIdleCallback`, falling
//                 back to `setTimeout`), so it never competes with hydration.
//   - `worker`  — run the script body inside a Web Worker (a `src` script is
//                 `importScripts`ed). For a measurement/analytics script that
//                 should not block the main thread. The worker's globals are
//                 isolated; a script that needs `document` cannot use it.
//   - `afterInteractive` — load once the first hydration pass completes.
//   - `lazy`    — load when the placeholder scrolls into view (IntersectionObserver).
//
// This mirrors the islands scheduler (`islands.ts`): server markup carries the
// intent, the client decides *when*. No framework JS is needed for a script,
// so this is part of the core, not an adapter.
//
// Import-safe in the SSR / config-loader environment: every function is a
// no-op when `document` is absent, because `@oxide/react` re-exports this and
// the config loader evaluates it in the Node worker.

/** The strategies §16.6 defines, plus two common conveniences. */
export type ScriptStrategy =
  | 'defer'
  | 'idle'
  | 'worker'
  | 'afterInteractive'
  | 'lazy'

export interface ScriptOptions {
  /** The document to operate on. Defaults to `globalThis.document`. */
  doc?: Document
  /** A script's source URL (a `src` script). Omit for inline code. */
  src?: string
  /** A script's inline body. Omit for a `src` script. */
  content?: string
  /** Module scripts get `type="module"`. Default `false` (classic). */
  module?: boolean
  /** Arbitrary attributes copied onto the injected `<script>` element. */
  attrs?: Record<string, string>
  /** Called after the script loads (a `src` script's `load` event). */
  onLoad?: () => void
  /** Called if the script fails to load. */
  onError?: (e: unknown) => void
  /** Overridable scheduling primitives (for tests). */
  requestIdleCallback?: (cb: () => void) => unknown
  IntersectionObserver?: new (
    cb: (entries: Array<{ isIntersecting: boolean }>) => void,
  ) => { observe(el: Element): void; disconnect(): void }
  Worker?: new (url: string) => { postMessage(msg: unknown): void; terminate(): void }
}

function docOf(opts: ScriptOptions): Document | undefined {
  return opts.doc ?? (globalThis as { document?: Document }).document
}

function idle(cb: () => void, opts: ScriptOptions): void {
  const ric =
    opts.requestIdleCallback ??
    (globalThis as { requestIdleCallback?: (cb: () => void) => unknown }).requestIdleCallback
  if (typeof ric === 'function') {
    ric(cb)
  } else {
    setTimeout(cb, 1)
  }
}

/** Inject a `<script>` element and resolve when it loads (or run inline). */
function inject(
  doc: Document,
  opts: ScriptOptions,
): void {
  const el = doc.createElement('script')
  if (opts.module) el.type = 'module'
  if (opts.attrs) for (const [k, v] of Object.entries(opts.attrs)) el.setAttribute(k, v)
  if (opts.src) {
    el.src = opts.src
    if (opts.onLoad) el.addEventListener('load', opts.onLoad, { once: true })
    if (opts.onError) el.addEventListener('error', (e) => opts.onError?.(e), { once: true })
    doc.head.appendChild(el)
  } else {
    el.textContent = opts.content ?? ''
    doc.head.appendChild(el)
    opts.onLoad?.()
  }
}

/** Run a script body in a Web Worker; a `src` is `importScripts`ed there. */
function runInWorker(opts: ScriptOptions): void {
  const W =
    opts.Worker ??
    (globalThis as { Worker?: new (url: string) => { postMessage(m: unknown): void; terminate(): void } })
      .Worker
  if (typeof W !== 'function') {
    // No Worker (SSR, an old browser): degrade to the main thread rather than
    // drop the script. The strategy is an optimization, not a correctness
    // guarantee.
    const doc = docOf(opts)
    if (doc) inject(doc, opts)
    return
  }
  const body = opts.src ? `importScripts(${JSON.stringify(opts.src)});` : (opts.content ?? '')
  try {
    const blob = new Blob([body], { type: 'application/javascript' })
    const url = URL.createObjectURL(blob)
    const worker = new W(url)
    // Terminate shortly after; these scripts are fire-and-forget measurement.
    setTimeout(() => {
      worker.terminate()
      URL.revokeObjectURL(url)
    }, 5000)
    opts.onLoad?.()
  } catch (e) {
    opts.onError?.(e)
  }
}

/**
 * Schedule one script by strategy. Returns a disposer (a no-op for most
 * strategies; it disconnects the IntersectionObserver for `lazy`). No-op when
 * no document is present.
 */
export function scheduleScript(strategy: ScriptStrategy, opts: ScriptOptions = {}): () => void {
  const doc = docOf(opts)
  if (!doc) return () => {}

  switch (strategy) {
    case 'defer': {
      // After parsing: inline runs at DOMContentLoaded, a src script gets
      // `defer` semantics by being appended after parse.
      if (doc.readyState === 'loading') {
        doc.addEventListener('DOMContentLoaded', () => inject(doc, opts), { once: true })
      } else {
        inject(doc, opts)
      }
      return () => {}
    }
    case 'idle':
      idle(() => inject(doc, opts), opts)
      return () => {}
    case 'afterInteractive': {
      // The boot runtime runs hydration in the same tick; a microtask hop is
      // enough to land after it for the common case. A project with island
      // hydration can call `scheduleScript` itself after awaiting hydration.
      Promise.resolve().then(() => inject(doc, opts))
      return () => {}
    }
    case 'worker':
      idle(() => runInWorker(opts), opts)
      return () => {}
    case 'lazy': {
      const IO =
        opts.IntersectionObserver ??
        (globalThis as { IntersectionObserver?: ScriptOptions['IntersectionObserver'] })
          .IntersectionObserver
      const placeholder = doc.currentScript ?? doc.querySelector('[data-ox-script-placeholder]')
      if (typeof IO !== 'function' || !placeholder) {
        // No observer or no placeholder: fall back to idle so the script is
        // not dropped.
        idle(() => inject(doc, opts), opts)
        return () => {}
      }
      const obs = new IO((entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          obs.disconnect()
          inject(doc, opts)
        }
      })
      obs.observe(placeholder)
      return () => obs.disconnect()
    }
  }
}

/**
 * Scan the document for server-rendered `<script data-ox-script>` tags and
 * schedule each by its `data-ox-strategy`. The generated boot entry calls
 * this after `boot()`.
 *
 * A tag is a *placeholder*: it carries `data-ox-src` (a URL) or
 * `data-ox-inline` (base64 body) plus the strategy. The real script is
 * injected by `scheduleScript`; the placeholder stays inert.
 */
export function loadScripts(opts: ScriptOptions = {}): void {
  const doc = docOf(opts)
  if (!doc) return
  const tags = doc.querySelectorAll('script[data-ox-script]')
  for (let i = 0; i < tags.length; i++) {
    const el = tags[i]
    const strategy = (el.getAttribute('data-ox-strategy') ?? 'defer') as ScriptStrategy
    const src = el.getAttribute('data-ox-src') ?? undefined
    const inlineB64 = el.getAttribute('data-ox-inline')
    const content = inlineB64
      ? new TextDecoder().decode(
          Uint8Array.from(atob(inlineB64), (c) => c.charCodeAt(0)),
        )
      : undefined
    const isModule = el.getAttribute('data-ox-module') !== null
    scheduleScript(strategy, { ...opts, doc, src, content, module: isModule })
  }
}
