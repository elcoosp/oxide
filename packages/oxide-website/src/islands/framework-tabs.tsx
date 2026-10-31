import { useEffect } from 'react'
import { useReducedMotion } from '../lib/motion.ts'
import { codeToHtml } from 'rangi'
import { githubDark } from 'rangi/themes'

const STORAGE_KEY = 'oxide:framework'
const SYNC_EVENT = 'oxide:framework-change'
const LANGS = ['react', 'vue', 'svelte', 'solid'] as const
type Lang = (typeof LANGS)[number]
const LABEL: Record<Lang, string> = { react: 'React', vue: 'Vue', svelte: 'Svelte', solid: 'Solid' }

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

function build(el: HTMLElement, snippets: Record<Lang, string>, active: Lang, reduced: boolean) {
  el.textContent = ''
  el.className = 'max-w-4xl mx-auto w-full'

  const tabs = document.createElement('div')
  tabs.className = 'flex border-b border-zinc-800 relative w-full justify-start'
  tabs.setAttribute('role', 'tablist')

  const indicator = document.createElement('span')
  indicator.className = 'absolute bottom-[-1px] h-[2px] bg-orange-600 transition-all duration-300'
  tabs.appendChild(indicator)

  const panel = document.createElement('pre')
  panel.className = 'm-0 p-6 bg-zinc-900 border border-zinc-800 border-t-0 rounded-b-xl overflow-x-auto'
  panel.setAttribute('role', 'tabpanel')

  const code = document.createElement('code')
  code.className = 'block font-mono text-sm leading-relaxed text-zinc-50 whitespace-pre min-w-0'
  panel.appendChild(code)

  const buttons = new Map<Lang, HTMLButtonElement>()

  const moveIndicator = () => {
    const btn = buttons.get(active)
    if (!btn) return
    indicator.style.width = btn.offsetWidth + 'px'
    indicator.style.transform = 'translateX(' + btn.offsetLeft + 'px)'
  }

  const apply = async (next: Lang, persist: boolean) => {
    active = next
    for (const [lang, btn] of buttons) {
      const on = lang === active
      btn.setAttribute('aria-selected', on ? 'true' : 'false')
      btn.className = `flex-none! w-max! px-4 py-3 font-mono text-xs tracking-wider transition-colors cursor-pointer border-none bg-transparent ${on ? 'text-zinc-50' : 'text-zinc-400 hover:text-zinc-200'}`
    }

    code.textContent = snippets[active]
    moveIndicator()

    try {
      const lang = next === 'solid' ? 'tsx' : next
      const html = codeToHtml(snippets[active], { lang, theme: githubDark })
      const parsed = new DOMParser().parseFromString(html, 'text/html')
      const shj = parsed.querySelector('[class*="shj-lang-"]')
      if (shj && active === next) {
        code.innerHTML = shj.innerHTML
        const bg = (shj as HTMLElement).style.backgroundColor
        if (bg) panel.style.backgroundColor = bg
      }
    } catch (e) { console.warn('rangi failed', e) }

    if (persist) {
      try { localStorage.setItem(STORAGE_KEY, active) } catch {}
      window.dispatchEvent(new CustomEvent(SYNC_EVENT, { detail: active }))
    }
  }

  for (const lang of LANGS) {
    const btn = document.createElement('button')
    btn.type = 'button'
    btn.id = 'fw-tab-' + lang
    btn.className = 'flex-none! w-max! px-4 py-3 font-mono text-xs tracking-wider text-zinc-400 transition-colors cursor-pointer border-none bg-transparent'
    btn.textContent = LABEL[lang]
    btn.addEventListener('click', () => apply(lang, true))
    buttons.set(lang, btn)
    tabs.appendChild(btn)
  }

  el.appendChild(tabs)
  el.appendChild(panel)
  apply(active, false)
}

export default function FrameworkTabsBootstrap() {
  const reduced = useReducedMotion()
  useEffect(() => {
    const blocks = document.querySelectorAll<HTMLElement>('[data-fw-snippets]')
    blocks.forEach((el) => {
      if (el.classList.contains('fw-code-ready')) return
      const snippets = parseSnippets(el)
      if (snippets) build(el, snippets, readStored() ?? 'react', !!reduced)
    })
  }, [reduced])
  return null
}
