// src/routes/layout.tsx — root layout (§8.2). The adapter shell owns the
// <html>/<body> document; layouts compose the app chrome around pages.
// Importing the stylesheet here is enough: the bundler hashes it through
// Lightning CSS and the injector streams the <link> into every page's head.
import type { ReactNode } from 'react'
import '../styles/global.css'
import '../styles/tailwind.css'
import { site, nav, footerLinks } from '../lib/site.ts'

interface Props {
  children?: ReactNode
}

export default function RootLayout({ children }: Props) {
  return (
    <>
      <header className="site-header">
        <div className="container">
          <a className="logo" href="/">
            oxide<span className="cursor">_</span>
          </a>
          <nav className="site-nav">
            {nav.map((n) => (
              <a key={n.href} href={n.href}>
                {n.label}
              </a>
            ))}
            <a className="gh" href={site.repo} rel="noopener">
              GitHub ↗
            </a>
          </nav>
        </div>
      </header>

      {children}

      <div className="flex gap-2 bg-red-500 px-4 py-2 text-oxide-accent" data-tailwind-probe="active">
        tailwind-probe
      </div>

      <footer className="site-footer">
        <div className="container">
          <span>
            {site.name} {site.version} · {site.license}
          </span>
          {footerLinks.map((l) => (
            <a key={l.href} href={l.href}>
              {l.label}
            </a>
          ))}
          <span className="built">
            this site is rendered by <em>oxide itself</em> — one binary, streaming SSR
          </span>
        </div>
      </footer>
    </>
  )
}
