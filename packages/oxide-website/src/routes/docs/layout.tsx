// src/routes/docs/layout.tsx — docs section layout: sidebar + body. Layouts
// nest (§8.2): the pipeline resolves the layout chain from the router and
// wraps the page — RootLayout(DocsLayout(Page)).
import type { ReactNode } from 'react'
import { nav } from '../../lib/site.ts'

interface Props {
  children?: ReactNode
}

const docLinks = [
  { href: '/docs', label: 'Getting started' },
  { href: '/docs/architecture', label: 'Architecture' },
  { href: '/docs/wire-format-odf', label: 'Wire format (ODF)' },
  { href: '/docs/adapters', label: 'Framework adapters' },
  { href: '/docs/caching', label: 'Caching' },
  { href: '/docs/cli-reference', label: 'CLI reference' },

  { href: '/docs/development', label: 'Local development' },
  { href: '/docs/ci', label: 'CI' },
  { href: '/docs/release', label: 'Release process' },
  { href: '/docs/security', label: 'Security' },
  ]

export default function DocsLayout({ children }: Props) {
  return (
    <div className="docs-shell container">
      <aside className="docs-sidebar">
        <div className="cap">Docs</div>
        {docLinks.map((l) => (
          <a href={l.href} key={l.href}>
            {l.label}
          </a>
        ))}
        <div className="cap" style={{ marginTop: 22 }}>
          Site
        </div>
        {nav.map((n) => (
          <a href={n.href} key={n.href}>
            {n.label}
          </a>
        ))}
        <div className="cap" style={{ marginTop: 22 }}>
          Project
        </div>
        <a href="https://github.com/elcoosp/oxide" rel="noopener">
          GitHub ↗
        </a>
        <a href="/rss">RSS</a>
      </aside>
      <div className="docs-body">{children}</div>
    </div>
  )
}
