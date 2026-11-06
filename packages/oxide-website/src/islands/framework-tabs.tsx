import { useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import AdapterTabs from './adapter-tabs.tsx'

const META = {
  react: { label: 'React', logo: '/frameworks/react.png', note: 'React 19 adapter — streaming SSR + concurrent features.' },
  vue: { label: 'Vue', logo: '/frameworks/vue.png', note: 'Vue 3 adapter — SFC support + Vapor runtime.' },
  svelte: { label: 'Svelte', logo: '/frameworks/svelte.png', note: 'Svelte 5 adapter — runes + server components.' },
  solid: { label: 'Solid', logo: '/frameworks/solid.png', note: 'Solid adapter — fine-grained reactivity.' }
} as const

type Lang = keyof typeof META
const LANGS = ['react', 'vue', 'svelte', 'solid'] as const

function parseSnippets(el: HTMLElement): Record<Lang, string> | null {
  const raw = el.getAttribute('data-fw-snippets')
  if (!raw) return null
  try {
    const parsed = JSON.parse(raw) as Partial<Record<Lang, string>>
    if (!parsed || LANGS.some((l) => typeof parsed[l] !== 'string')) return null
    return parsed as Record<Lang, string>
  } catch { return null }
}

export default function FrameworkTabsBootstrap() {
  const [targets, setTargets] = useState<{ el: HTMLElement, snippets: Record<Lang, string> }[]>([])

  useEffect(() => {
    const process = () => {
      const els = Array.from(document.querySelectorAll<HTMLElement>('[data-fw-snippets]'))
      const found = els.map(el => {
        if (el.classList.contains('fw-code-ready')) return null
        const snippets = parseSnippets(el)
        if (snippets) {
          el.classList.add('fw-code-ready')
          // Drop the SSR placeholder children: createPortal APPENDS into the
          // target, so without this the static fallback stays stacked above
          // the interactive tabs.
          el.textContent = ''
          return { el, snippets }
        }
        return null
      }).filter(Boolean) as { el: HTMLElement, snippets: Record<Lang, string> }[]

      if (found.length > 0) {
        setTargets(prev => [...prev, ...found])
      }
    }

    process()

    const obs = new MutationObserver(process)
    obs.observe(document.body, { childList: true, subtree: true })
    return () => obs.disconnect()
  }, [])

  return (
    <>
      {targets.map(({ el, snippets }, i) => {
        const adapters = LANGS.map(id => ({
          id,
          label: META[id].label,
          logo: META[id].logo,
          note: META[id].note,
          snippet: snippets[id]
        }))
        return createPortal(<AdapterTabs key={i} adapters={adapters} />, el)
      })}
    </>
  )
}
