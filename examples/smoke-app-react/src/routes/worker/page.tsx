import { useEffect, useState } from 'react'

// §3.5 fixture. Spawns the counter worker on hydration via `useEffect` —
// NOT via an inline <script>. An inline `<script>` inside
// `dangerouslySetInnerHTML` is blocked by the dev CSP (`script-src 'self'
// 'nonce-...' 'strict-dynamic'`): the render pipeline adds a nonce only to
// scripts IT emits (like `/@boot/boot.mjs`), not to user-emitted inline
// scripts. The idiomatic React pattern is `useEffect`, which runs on the
// client after hydration, has the nonce-free module context, and cleans up
// on unmount.
//
// The status is rendered as a `data-worker-status` attribute (matching the
// Playwright test's selector), updated from a `useState` that the
// worker's `message` event fires.
export default function WorkerPage() {
  const [status, setStatus] = useState('not-spawned')

  useEffect(() => {
    const w = new Worker('/src/workers/counter.worker.ts', { type: 'module' })
    const onMessage = (e: MessageEvent): void => {
      setStatus(JSON.stringify(e.data))
    }
    const onError = (e: ErrorEvent): void => {
      setStatus(JSON.stringify({ error: String(e.message || e) }))
    }
    w.addEventListener('message', onMessage)
    w.addEventListener('error', onError)
    return () => {
      w.removeEventListener('message', onMessage)
      w.removeEventListener('error', onError)
      w.terminate()
    }
  }, [])

  return (
    <main data-worker-page="">
      <h1>Worker</h1>
      <div data-worker-status={status}>{status}</div>
    </main>
  )
}
