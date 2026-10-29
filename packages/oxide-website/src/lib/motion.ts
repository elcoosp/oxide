// src/lib/motion.ts — motion primitives shared by every island.
//
// Two rules, both from W3C technique C39/SCR40:
//
//   1. Motion is opt-in. `useReducedMotion()` returns true when the user has
//      asked the OS to reduce motion; callers must render the static version
//      rather than a shortened animation.
//   2. Nothing here assumes a browser. `matchMedia` is guarded, and the
//      subscription uses `addEventListener` with an `addListener` fallback for
//      older Safari.
//
// A "micro-interaction" on this site is <= 200ms of transform/opacity. Longer
// motion (reveals) is scroll-triggered, not timed.

import { useEffect, useState } from 'react'

const QUERY = '(prefers-reduced-motion: reduce)'

function getInitial(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') {
    return false
  }
  return window.matchMedia(QUERY).matches
}

export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState<boolean>(getInitial)

  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
    const mql = window.matchMedia(QUERY)
    const onChange = () => setReduced(mql.matches)
    onChange()
    if (typeof mql.addEventListener === 'function') {
      mql.addEventListener('change', onChange)
      return () => mql.removeEventListener('change', onChange)
    }
    // Safari < 14
    mql.addListener(onChange)
    return () => mql.removeListener(onChange)
  }, [])

  return reduced
}

/// True when the browser implements the same-document View Transitions API.
/// Baseline since Oct 2025 (Chrome 111+, Safari 18+, Firefox 144+), but a
/// runtime check keeps the site working on anything older.
export function supportsViewTransitions(): boolean {
  if (typeof document === 'undefined') return false
  const doc = document as Document & {
    startViewTransition?: (cb: () => void) => unknown
  }
  return typeof doc.startViewTransition === 'function'
}
