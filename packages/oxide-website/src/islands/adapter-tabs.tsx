// src/islands/adapter-tabs.tsx — one engine, four frameworks.
//
// Switching tabs is a same-document navigation in miniature. Where the View
// Transitions API exists (Baseline since Oct 2025) the panel crossfades from
// its old geometry to its new one; elsewhere the switch is instant, which is
// the correct fallback — a missing transition is invisible, a broken one is
// not.
//
// Reduced motion collapses both paths to the instant switch, per W3C C39:
// the information (which adapter is selected) still arrives, only the trip is
// skipped.

import { useCallback, useMemo, useState } from 'react'
import { supportsViewTransitions, useReducedMotion } from '../lib/motion.ts'

export interface Adapter {
  id: string
  label: string
  snippet: string
  note: string
}

interface Props {
  adapters: Adapter[]
}

export default function AdapterTabs({ adapters }: Props) {
  const firstId = adapters[0]?.id ?? ''
  const [active, setActive] = useState<string>(firstId)
  const reduced = useReducedMotion()

  const current = useMemo<Adapter | undefined>(
    () => adapters.find((a) => a.id === active) ?? adapters[0],
    [adapters, active],
  )

  const select = useCallback((id: string) => {
    if (id === active) return
    if (!reduced && supportsViewTransitions()) {
      const doc = document as Document & {
        startViewTransition?: (cb: () => void) => unknown
      }
      if (typeof doc.startViewTransition === 'function') {
        doc.startViewTransition(() => setActive(id))
        return
      }
    }
    setActive(id)
  }, [active, reduced])

  const onKeyDown = useCallback((e: React.KeyboardEvent<HTMLButtonElement>) => {
    if (adapters.length === 0) return
    const i = adapters.findIndex((x) => x.id === active)
    const at = i < 0 ? 0 : i
    if (e.key === 'ArrowRight') {
      e.preventDefault()
      const next = adapters[(at + 1) % adapters.length]
      if (next) select(next.id)
    } else if (e.key === 'ArrowLeft') {
      e.preventDefault()
      const prev = adapters[(at - 1 + adapters.length) % adapters.length]
      if (prev) select(prev.id)
    }
  }, [adapters, active, select])

  if (!current) return null

  return (
    <div className="adapter-tabs">
      <div className="adapter-tablist" role="tablist" aria-label="Framework adapter">
        {adapters.map((a) => (
          <button
            key={a.id}
            type="button"
            role="tab"
            id={`tab-${a.id}`}
            aria-selected={a.id === active}
            aria-controls={`panel-${a.id}`}
            tabIndex={a.id === active ? 0 : -1}
            className="adapter-tab"
            onClick={() => select(a.id)}
            onKeyDown={onKeyDown}
          >
            {a.label}
          </button>
        ))}
      </div>
      <div
        className="adapter-panel"
        role="tabpanel"
        id={`panel-${current.id}`}
        aria-labelledby={`tab-${current.id}`}
        style={{ viewTransitionName: 'adapter-panel' }}
      >
        <pre className="adapter-snippet"><code>{current.snippet}</code></pre>
        <p className="adapter-note">{current.note}</p>
      </div>
    </div>
  )
}
