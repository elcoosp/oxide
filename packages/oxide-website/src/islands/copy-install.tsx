// src/islands/copy-install.tsx — the install command, copyable in one click.
//
// The interior.dev rule this enforces: reserve the width of every state before
// it arrives. The button renders "Copy", "Copied" and "Select" stacked in the
// same grid cell, so the row never shifts when the label changes. All three
// spans are always present; only opacity flips.
//
// Failure is a real state, not an afterthought. navigator.clipboard.writeText
// rejects on insecure origins, on denied permission, and inside some embedded
// webviews. When it does, the component selects the text instead so the user
// can copy manually — the interaction completes either way.

import { useCallback, useEffect, useRef, useState } from 'react'

type Status = 'idle' | 'copied' | 'failed'

interface Props {
  command: string
}

export default function CopyInstall({ command }: Props) {
  const [status, setStatus] = useState<Status>('idle')
  const timer = useRef<number | null>(null)
  const code = useRef<HTMLElement | null>(null)

  useEffect(() => () => {
    if (timer.current !== null) window.clearTimeout(timer.current)
  }, [])

  const copy = useCallback(async () => {
    if (timer.current !== null) window.clearTimeout(timer.current)
    try {
      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(command)
        setStatus('copied')
      } else {
        throw new Error('clipboard unavailable')
      }
    } catch {
      setStatus('failed')
      const node = code.current
      if (node) {
        const range = document.createRange()
        range.selectNodeContents(node)
        const sel = window.getSelection()
        if (sel) {
          sel.removeAllRanges()
          sel.addRange(range)
        }
      }
    }
    timer.current = window.setTimeout(() => setStatus('idle'), 2000)
  }, [command])

  return (
    <div className="copy-install" data-status={status}>
      <code className="copy-install-cmd" ref={code}>{command}</code>
      <button
        type="button"
        className="copy-install-btn"
        onClick={copy}
        aria-label={status === 'copied' ? 'Copied to clipboard' : 'Copy install command'}
      >
        <span className="copy-install-label">
          <span data-state="idle" hidden={status !== 'idle'}>Copy</span>
          <span data-state="copied" hidden={status !== 'copied'}>Copied</span>
          <span data-state="failed" hidden={status !== 'failed'}>Select</span>
        </span>
      </button>
      <span className="sr-only" role="status" aria-live="polite">
        {status === 'copied' ? 'Install command copied to clipboard' : ''}
        {status === 'failed' ? 'Clipboard unavailable — command selected, copy manually' : ''}
      </span>
    </div>
  )
}
