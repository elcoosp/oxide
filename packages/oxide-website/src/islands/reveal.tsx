// src/islands/reveal.tsx — one observer, one pass, no per-section island.
//
// Adds .is-revealed to every [data-reveal] element as it enters the viewport.
// The transition itself lives in tailwind.css and is wrapped in
// @media (prefers-reduced-motion: no-preference), so this island only does
// something when the user has not asked the OS to reduce motion.
//
// threshold 0.1 rather than 0: revealing at 10% feels like the content was
// already there; waiting for full visibility feels late on tall sections.
// rootMargin is negative on the bottom edge so a section reveals when it has
// been read into, not when it first peeks over the fold.

import { useEffect } from 'react'
import { useReducedMotion } from '../lib/motion.ts'

export default function RevealBootstrap(): null {
  const reduced = useReducedMotion()

  useEffect(() => {
    const els = Array.from(document.querySelectorAll<HTMLElement>('[data-reveal]'))
    if (els.length === 0) return

    if (reduced || typeof IntersectionObserver !== 'function') {
      for (const el of els) el.classList.add('is-revealed')
      return
    }

    const io = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue
        entry.target.classList.add('is-revealed')
        io.unobserve(entry.target)
      }
    }, { threshold: 0.1, rootMargin: '0px 0px -8% 0px' })

    for (const el of els) io.observe(el)
    return () => io.disconnect()
  }, [reduced])

  return null
}
