// §3.5 test worker. Verifies that `import.meta.hot` is present inside a
// Web Worker and that a worker module can receive an HMR update (via its
// own WS to /_oxide/hmr) without dying.

const __v = 'WORKER-V0'

type Msg = { hotDefined: boolean; v: string; ts: number }
const post = (): void => {
  const m: Msg = {
    hotDefined: typeof (import.meta as unknown as { hot?: unknown }).hot !== 'undefined',
    v: __v,
    ts: Date.now(),
  }
  ;(self as unknown as { postMessage: (x: unknown) => void }).postMessage(m)
}

post()

type HotCtx = { accept?: (cb?: () => void) => void }
const hot = (import.meta as unknown as { hot?: HotCtx }).hot
if (hot?.accept) {
  hot.accept(() => post())
}
