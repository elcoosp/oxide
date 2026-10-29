// src/islands/framework-tabs.tsx — upgrades every framework-tabs block on the
// page from a static code sample into a synchronized tab group.
//
// Why a bootstrap and not a per-block island: markdown-authored blocks carry
// their snippets in a data attribute, not through <Island props> — island
// props travel as base64 ODF, which is encoded on the Rust side and only
// decoded on the client. One bootstrap for the page also means one
// localStorage read and one set of listeners, rather than one per code block.
//
// The persistence contract follows Starlight's syncKey pattern: a single key,
// one choice reflected in every group, surviving navigation. Cross-instance
// sync uses a custom event because the `storage` event only fires in *other*
// documents, not the one that wrote.
//
// Accessibility: real ARIA tabs with roving tabindex and arrow keys. The
// indicator is one element translated to the active tab's offsetLeft — not a
// background per tab cross-fading — so it reads as one object moving rather
// than two appearing. Under prefers-reduced-motion it jumps with no transition.

import { useEffect } from 'react'
import { useReducedMotion } from '../lib/motion.ts'

const STORAGE_KEY = 'oxide:framework'
const SYNC_EVENT = 'oxide:framework-change'
const LANGS = ['react', 'vue', 'svelte', 'solid'] as const
type Lang = (typeof LANGS)[number]

const LABEL: Record<Lang, string> = {
  react: 'React',
  vue: 'Vue',
  svelte: 'Svelte',
  solid: 'Solid',
}

function readStored(): Lang | null {
  try {
    const v = localStorage.getItem(STORAGE_KEY)
    return v && (LANGS as readonly string[]).includes(v) ? (v as Lang) : null
  } catch {
    return null
  }
}

function parseSnippets(el: HTMLElement): Record<Lang, string> | null {
  const raw = el.getAttribute('data-fw-snippets')
  if (!raw) return null
  try {
    const parsed = JSON.parse(raw) as Partial<Record<Lang, string>>
    if (!parsed || LANGS.some((l) => typeof parsed[l] !== 'string')) return null
    return parsed as Record<Lang, string>
  } catch {
    return null
  }
}

function build(el: HTMLElement, snippets: Record<Lang, string>, active: Lang, reduced: boolean) {
  el.textContent = ''
  el.classList.add('fw-code-ready')

  const tabs = document.createElement('div')
  tabs.className = 'fw-code-tabs'
  tabs.setAttribute('role', 'tablist')
  tabs.setAttribute('aria-label', 'Framework')

  const indicator = document.createElement('span')
  indicator.className = 'fw-code-indicator'
  indicator.setAttribute('aria-hidden', 'true')
  tabs.appendChild(indicator)

  const panel = document.createElement('pre')
  panel.className = 'fw-code-panel'
  panel.setAttribute('role', 'tabpanel')
  const code = document.createElement('code')
  panel.appendChild(code)

  const buttons = new Map<Lang, HTMLButtonElement>()

  const moveIndicator = () => {
    const btn = buttons.get(active)
    if (!btn) return
    indicator.style.width = btn.offsetWidth + 'px'
    indicator.style.transform = 'translateX(' + btn.offsetLeft + 'px)'
  }

  const apply = (next: Lang, persist: boolean) => {
    active = next
    for (const [lang, btn] of buttons) {
      const on = lang === active
      btn.setAttribute('aria-selected', on ? 'true' : 'false')
      btn.tabIndex = on ? 0 : -1
    }
    code.innerHTML = snippets[active]
    panel.setAttribute('aria-labelledby', 'fw-tab-' + active)
    moveIndicator()
    if (persist) {
      try {
        localStorage.setItem(STORAGE_KEY, active)
      } catch {
        /* quota or private mode — the choice still applies for this page */
      }
      window.dispatchEvent(new CustomEvent(SYNC_EVENT, { detail: active }))
    }
  }

  for (const lang of LANGS) {
    const btn = document.createElement('button')
    btn.type = 'button'
    btn.className = 'fw-code-tab'
    btn.id = 'fw-tab-' + lang
    btn.textContent = LABEL[lang]
    btn.addEventListener('click', () => apply(lang, true))
    buttons.set(lang, btn)
    tabs.appendChild(btn)
  }

  tabs.addEventListener('keydown', (e: KeyboardEvent) => {
    const i = LANGS.indexOf(active)
    if (e.key === 'ArrowRight') {
      e.preventDefault()
      const next = LANGS[(i + 1) % LANGS.length]
      if (next) apply(next, true)
      buttons.get(active)?.focus()
    } else if (e.key === 'ArrowLeft') {
      e.preventDefault()
      const prev = LANGS[(i - 1 + LANGS.length) % LANGS.length]
      if (prev) apply(prev, true)
      buttons.get(active)?.focus()
    }
  })

  if (!reduced) {
    indicator.style.transition =
      'transform 200ms cubic-bezier(0.16,1,0.3,1), width 200ms cubic-bezier(0.16,1,0.3,1)'
  }

  el.appendChild(tabs)
  el.appendChild(panel)
  apply(active, false)
  // offsetWidth is 0 until the element is laid out; re-run on the next frame.
  requestAnimationFrame(moveIndicator)
}

export default function FrameworkTabsBootstrap(): null {
  const reduced = useReducedMotion()

  useEffect(() => {
    const blocks = Array.from(document.querySelectorAll<HTMLElement>('[data-fw-snippets]'))
    if (blocks.length === 0) return

    const stored = readStored() ?? 'react'
    const built: Array<{ el: HTMLElement; snippets: Record<Lang, string> }> = []

    for (const el of blocks) {
      const snippets = parseSnippets(el)
      if (!snippets) continue
      build(el, snippets, stored, reduced)
      built.push({ el, snippets })
    }

    const onSync = (e: Event) => {
      const next = (e as CustomEvent<Lang>).detail
      if (!LANGS.includes(next)) return
      for (const { el } of built) {
        const buttons = el.querySelectorAll<HTMLButtonElement>('.fw-code-tab')
        if (!buttons.length) continue
        buttons.forEach((btn, idx) => {
          if (LANGS[idx] === next) btn.click()
        })
      }
    }

    window.addEventListener(SYNC_EVENT, onSync)
    return () => window.removeEventListener(SYNC_EVENT, onSync)
  }, [reduced])

  return null
}
