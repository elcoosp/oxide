import { useEffect } from 'react'

const THEME = 'github-dark-dimmed'
const LANGS = ['tsx','ts','jsx','js','bash','sh','css','html','json','rust','vue','svelte','shell','markdown'] as const

let highlighterPromise: Promise<any> | null = null
function getHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = import('shiki').then(m =>
      m.createHighlighter({ themes: [THEME], langs: [...LANGS] })
    )
  }
  return highlighterPromise
}
function normalizeLang(raw: string): string {
  const l = raw.toLowerCase()
  if (l === 'solid') return 'tsx'
  return (LANGS as readonly string[]).includes(l) ? l : 'tsx'
}

async function highlight(pre: HTMLElement) {
  const code = pre.querySelector('code')
  if (!code) return
  const text = code.textContent ?? ''
  if (!text.trim()) return

  const lang = normalizeLang(code.className.match(/language-([\w-]+)/)?.[1] ?? 'tsx')

  try {
    const h = await getHighlighter()
    const html = h.codeToHtml(text, { lang, theme: THEME })

    // Extract just the inner HTML of the <code> block so we don't replace
    // the <pre> element (which framework-tabs holds a reference to).
    const parsed = new DOMParser().parseFromString(html, 'text/html')
    const newPre = parsed.querySelector('pre')
    const newCode = parsed.querySelector('code')

    if (newCode && newPre) {
      code.innerHTML = newCode.innerHTML
      pre.style.backgroundColor = newPre.getAttribute('style')?.match(/background-color:[^;]+/)?.[0] ?? ''
      pre.style.color = newPre.getAttribute('style')?.match(/color:[^;]+/)?.[0] ?? ''
      pre.classList.add('shiki-active')
    }
  } catch (e) {
    console.warn('shiki failed', lang, e)
  }
}

function scan() {
  document.querySelectorAll<HTMLElement>('pre:not(.shiki-active)').forEach(pre => {
    if (pre.querySelector('code')) highlight(pre)
  })
}

export default function CodeHighlightBootstrap() {
  useEffect(() => {
    // 1. Initial scan
    scan()

    // 2. Watch for framework-tabs swapping code.innerHTML
    const obs = new MutationObserver((mutations) => {
      for (const m of mutations) {
        if (m.type === 'childList' || m.type === 'characterData') {
          // Find the parent <pre> of the changed node
          let node = m.target as Node | null
          while (node && node.nodeName !== 'PRE') node = node.parentNode
          if (node) {
            // Reset shiki-active so it re-highlights
            (node as HTMLElement).classList.remove('shiki-active')
            highlight(node as HTMLElement)
          }
        }
      }
    })

    obs.observe(document.body, { childList: true, subtree: true, characterData: true })

    // 3. framework-tabs fires this event on sync
    const onSync = () => setTimeout(scan, 10)
    window.addEventListener('oxide:framework-change', onSync)

    return () => {
      obs.disconnect()
      window.removeEventListener('oxide:framework-change', onSync)
    }
  }, [])
  return null
}
