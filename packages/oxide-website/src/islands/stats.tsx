// src/islands/stats.tsx — the home-page proof strip, shipped as a real
// island: server-rendered inline (final values in the HTML), hydrated on
// the client with a count-up animation via the `visible` strategy.
//
// The props cross the wire as base64 ODF (`data-ox-props`) — the same
// byte-exact format the cache uses, decoded by the zero-dependency client
// runtime. Registration happens through `framework.islands` in
// oxide.config.ts; the build emits the __boot__ entry that hydrates us.
import { useEffect, useState } from 'react'

export interface StatItem {
  value: number
  label: string
}

export default function Stats({ items }: { items: StatItem[] }) {
  const [progress, setProgress] = useState(1)

  useEffect(() => {
    // animate once on hydration; SSR already showed the real numbers
    const start = performance.now()
    const dur = 900
    setProgress(0)
    let raf = 0
    const tick = (t: number) => {
      const p = Math.min(1, (t - start) / dur)
      setProgress(p)
      if (p < 1) raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [])

  const fmt = (v: number) => {
    const n = Math.round(v * progress)
    return n >= 1000 ? n.toLocaleString('en-US') : String(n)
  }

  return (
    <div className="grid">
      {items.map((it) => (
        <div className="stat" key={it.label}>
          <div className="num">{fmt(it.value)}</div>
          <div className="lbl">{it.label}</div>
        </div>
      ))}
    </div>
  )
}
