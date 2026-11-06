// packages/oxide-website/src/islands/code-highlight.tsx
//
// Generic syntax-highlighting island: scans every `<pre><code>` on the page
// and applies rangi highlighting, re-running after DOM mutations (e.g. when
// an adapter tab swaps the active snippet). Not currently mounted by the
// site — framework-tabs.tsx handles its own highlighting inline — but kept
// as a reusable bootstrap for pages that have plain markdown-rendered code.
//
// [wr2xx] Migrated to rangi: the previous highlighter was never a direct
// dep of packages/oxide-website, so its dynamic import failed at runtime
// and tripped TS2307 at tsc time. rangi ships `codeToHtml` + `githubDark`
// (rangi/themes) and is already the highlighter md.server.ts and
// adapter-tabs.tsx use.
import { useEffect } from 'react'
import { codeToHtml } from 'rangi'
import { githubDark } from 'rangi/themes'

const LANGS = ['tsx','ts','jsx','js','bash','sh','css','html','json','rust','vue','svelte','shell','markdown'] as const
const ACTIVE = 'code-hl-active'

// Timestamp until which observer-reported mutations are ignored because they
// were caused by highlight() itself (see the observer below).
let selfWriteUntil = 0

function normalizeLang(raw: string): string {
  const l = raw.toLowerCase()
  if (l === 'solid') return 'tsx'
  return (LANGS as readonly string[]).includes(l) ? l : 'tsx'
}

function highlight(pre: HTMLElement) {
  const code = pre.querySelector('code')
  if (!code) return
  const text = code.textContent ?? ''
  if (!text.trim()) return

  const lang = normalizeLang(code.className.match(/language-([\w-]+)/)?.[1] ?? 'tsx')

  try {
    const html = codeToHtml(text, { lang, theme: githubDark })
    const parsed = new DOMParser().parseFromString(html, 'text/html')
    const shj = parsed.querySelector('[class*="shj-lang-"]')
    if (shj) {
      // Our own write echoes back through the MutationObserver below as a
      // childList mutation; without this window the observer would re-run
      // highlight() on it forever (100% CPU, self-sustaining loop).
      selfWriteUntil = Date.now() + 100
      code.innerHTML = shj.innerHTML
      const bg = (shj as HTMLElement).style.backgroundColor
      if (bg) pre.style.backgroundColor = bg
      pre.classList.add(ACTIVE)
    }
  } catch (e) {
    console.warn('rangi failed', lang, e)
  }
}

function scan() {
  document.querySelectorAll<HTMLElement>(`pre:not(.${ACTIVE})`).forEach(pre => {
    if (pre.querySelector('code')) highlight(pre)
  })
}

export default function CodeHighlightBootstrap() {
  useEffect(() => {
    scan()

    const obs = new MutationObserver((mutations) => {
      // Skip the mutations highlight() itself just caused (see selfWriteUntil).
      if (Date.now() < selfWriteUntil) return
      for (const m of mutations) {
        if (m.type === 'childList' || m.type === 'characterData') {
          let node = m.target as Node | null
          while (node && node.nodeName !== 'PRE') node = node.parentNode
          if (node) {
            (node as HTMLElement).classList.remove(ACTIVE)
            highlight(node as HTMLElement)
          }
        }
      }
    })
    obs.observe(document.body, { childList: true, subtree: true, characterData: true })

    const onSync = () => setTimeout(scan, 10)
    window.addEventListener('oxide:framework-change', onSync)

    return () => {
      obs.disconnect()
      window.removeEventListener('oxide:framework-change', onSync)
    }
  }, [])
  return null
}
