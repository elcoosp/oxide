import { useEffect } from 'react'
import { codeToHtml } from 'rangi'
import { githubDark } from 'rangi/themes'

const STORAGE_KEY = 'oxide:framework'
const SYNC_EVENT = 'oxide:framework-change'

const META = {
  react: { label: 'React', logo: '/frameworks/react.png', note: 'React 19 adapter — streaming SSR + concurrent features.' },
  vue: { label: 'Vue', logo: '/frameworks/vue.png', note: 'Vue 3 adapter — SFC support + Vapor runtime.' },
  svelte: { label: 'Svelte', logo: '/frameworks/svelte.png', note: 'Svelte 5 adapter — runes + server components.' },
  solid: { label: 'Solid', logo: '/frameworks/solid.png', note: 'Solid adapter — fine-grained reactivity.' }
} as const

type Lang = keyof typeof META
const LANGS = ['react', 'vue', 'svelte', 'solid'] as const

function readStored(): Lang | null {
  try {
    const v = localStorage.getItem(STORAGE_KEY)
    return v && (LANGS as readonly string[]).includes(v) ? (v as Lang) : null
  } catch { return null }
}

function parseSnippets(el: HTMLElement): Record<Lang, string> | null {
  const raw = el.getAttribute('data-fw-snippets')
  if (!raw) return null
  try {
    const parsed = JSON.parse(raw) as Partial<Record<Lang, string>>
    if (!parsed || LANGS.some((l) => typeof parsed[l] !== 'string')) return null
    return parsed as Record<Lang, string>
  } catch { return null }
}

function stripShikiStyle(html: string): string {
  return html.replace(/<div\b([^>]*)>/g, (match, attrs) => {
    if (/class="[^"]*shj-/.test(attrs)) {
      return `<div${attrs.replace(/\s+style="[^"]*"/, '')}>`;
    }
    return match;
  });
}

function build(el: HTMLElement, snippets: Record<Lang, string>, active: Lang) {
  el.innerHTML = ''
  el.className = 'max-w-[1080px] mx-auto border border-zinc-800 rounded-[10px] overflow-hidden'

  const tabs = document.createElement('div')
  tabs.className = 'flex flex-wrap gap-1 p-1.5 bg-[#09090b] border-b border-zinc-800'
  tabs.setAttribute('role', 'tablist')

  const panel = document.createElement('div')
  panel.className = 'adapter-shiki bg-zinc-900'

  const note = document.createElement('p')
  note.className = 'm-0 px-5 py-4 bg-[#09090b] text-zinc-400 text-sm font-mono border-t border-zinc-800'

  const buttons = new Map<Lang, HTMLButtonElement>()

  const apply = (next: Lang, persist: boolean) => {
    active = next
    for (const [lang, btn] of buttons) {
      const on = lang === active
      btn.setAttribute('aria-selected', on ? 'true' : 'false')
      btn.className = `inline-flex items-center gap-2 px-3 py-2 font-mono text-[13px] cursor-pointer border border-transparent rounded-[7px] transition-colors ${
        on ? 'bg-zinc-900 text-zinc-50' : 'bg-transparent text-zinc-400 hover:text-zinc-200'
      }`
    }

    note.textContent = META[active].note

    const lang = next === 'solid' ? 'tsx' : next
    const html = codeToHtml(snippets[active], { lang, theme: githubDark, lineNumbers: false })
    panel.innerHTML = stripShikiStyle(html)

    if (persist) {
      try { localStorage.setItem(STORAGE_KEY, active) } catch {}
      window.dispatchEvent(new CustomEvent(SYNC_EVENT, { detail: active }))
    }
  }

  for (const lang of LANGS) {
    const btn = document.createElement('button')
    btn.type = 'button'
    btn.id = 'fw-tab-' + lang
    btn.addEventListener('click', () => apply(lang, true))

    const logoWrap = document.createElement('span')
    logoWrap.className = 'flex items-center justify-center w-[18px] h-[18px] rounded-full bg-zinc-200 overflow-hidden shrink-0'
    const img = document.createElement('img')
    img.src = META[lang].logo
    img.alt = ''
    img.width = 12
    img.height = 12
    img.style.width = '12px'
    img.style.height = '12px'
    img.style.objectFit = 'contain'
    logoWrap.appendChild(img)

    btn.appendChild(logoWrap)
    btn.appendChild(document.createTextNode(META[lang].label))

    buttons.set(lang, btn)
    tabs.appendChild(btn)
  }

  el.appendChild(tabs)
  el.appendChild(panel)
  el.appendChild(note)
  apply(active, false)
}

export default function FrameworkTabsBootstrap() {
  useEffect(() => {
    const process = () => {
      const blocks = document.querySelectorAll<HTMLElement>('[data-fw-snippets]')
      blocks.forEach((el) => {
        if (el.classList.contains('fw-code-ready')) return
        const snippets = parseSnippets(el)
        if (snippets) {
          el.classList.add('fw-code-ready')
          build(el, snippets, readStored() ?? 'react')
        }
      })
    }

    // Initial scan
    process()

    // Watch for client-side navigation (SPA swaps)
    const obs = new MutationObserver(() => {
      process()
    })
    obs.observe(document.body, { childList: true, subtree: true })

    return () => obs.disconnect()
  }, [])
  return null
}
