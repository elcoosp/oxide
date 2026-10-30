// src/routes/layout.tsx — root layout (§8.2). The adapter shell owns the
// <html>/<body> document; layouts compose the app chrome around pages.
// Importing the stylesheet here is enough: the bundler hashes it through
// Lightning CSS and the injector streams the <link> into every page's head.
import type { ReactNode } from 'react'
import '../styles/tailwind.css'
import { Island } from '@oxide-fw/react'
import { site, nav, footerLinks } from '../lib/site.ts'

interface Props {
  children?: ReactNode
}

export default function RootLayout({ children }: Props) {
  return (
    <>
      <link rel="preconnect" href="https://fonts.googleapis.com" />
      <link rel="preconnect" href="https://fonts.gstatic.com" crossOrigin="anonymous" />
      <link href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;700&display=swap" rel="stylesheet" />


      

      <Island component="RevealBootstrap" chunk="RevealBootstrap" strategy="visible" />
      <Island component="FrameworkTabsBootstrap" chunk="FrameworkTabsBootstrap" strategy="visible" />
      
      
      
{/* ═══ FrameworkAdapters CSS (inlined by wr14) ═══ */}
      

      <div className="scroll-progress" aria-hidden="true" />
      <header className="site-header">
        <div className="container">
          <a className="logo flex items-center gap-2.5" href="/">
            <img src="/logo.png" alt="Oxide" className="h-7 w-7 object-contain" />
            <span className="font-semibold tracking-tight text-lg">oxide<span className="cursor text-[#e45631]">_</span></span>
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

      <footer className="border-t border-zinc-800 bg-[#09090b] pt-16 pb-8">
        <div className="max-w-6xl mx-auto px-6">
          <div className="grid grid-cols-1 md:grid-cols-4 gap-12 mb-16">

            <div className="col-span-1 md:col-span-1">
              <a href="/" className="flex items-center gap-2 mb-4 group">
                <img src="/logo.png" alt="Oxide" className="w-8 h-8 object-contain opacity-80 group-hover:opacity-100 transition-opacity" />
                <span className="font-semibold tracking-tight text-xl text-zinc-50">
                  oxide<span className="text-[#e45631]">_</span>
                </span>
              </a>
              <p className="text-sm text-zinc-500 leading-relaxed max-w-xs">
                A full-stack meta-framework with a Rust core. One engine, every framework.
              </p>
            </div>

            <div>
              <h4 className="font-mono text-xs tracking-widest uppercase text-zinc-300 mb-4 font-semibold">Product</h4>
              <ul className="space-y-3 text-sm text-zinc-500">
                <li><a href="/docs" className="hover:text-[#e45631] transition-colors">Documentation</a></li>
                <li><a href="/docs/getting-started" className="hover:text-[#e45631] transition-colors">Getting Started</a></li>
                <li><a href="/blog" className="hover:text-[#e45631] transition-colors">Blog</a></li>
              </ul>
            </div>

            <div>
              <h4 className="font-mono text-xs tracking-widest uppercase text-zinc-300 mb-4 font-semibold">Community</h4>
              <ul className="space-y-3 text-sm text-zinc-500">
                <li><a href={site.repo} className="hover:text-[#e45631] transition-colors">GitHub</a></li>
                <li><a href="/rss" className="hover:text-[#e45631] transition-colors">RSS Feed</a></li>
                <li><a href={`${site.repo}/blob/main/LICENSE-MIT`} className="hover:text-[#e45631] transition-colors">MIT License</a></li>
                <li><a href={`${site.repo}/blob/main/LICENSE-APACHE`} className="hover:text-[#e45631] transition-colors">Apache-2.0 License</a></li>
              </ul>
            </div>

            <div>
              <h4 className="font-mono text-xs tracking-widest uppercase text-zinc-300 mb-4 font-semibold">Ready to ship?</h4>
              <p className="text-sm text-zinc-500 mb-4">Start building your next app today.</p>
              <a className="btn btn-primary w-full text-center" href="/docs">Get started</a>
            </div>

          </div>

          <div className="border-t border-zinc-800/50 pt-8 flex flex-col md:flex-row items-center justify-between gap-4 text-xs font-mono text-zinc-600">
            <div className="flex items-center gap-4">
              <span>Oxide v0.1</span>
              <span className="w-1 h-1 rounded-full bg-zinc-700"></span>
              <span>MIT / Apache-2.0</span>
            </div>
            <div>
              This site is rendered by <a href="/" className="text-zinc-400 hover:text-[#e45631] transition-colors">oxide itself</a> — one binary, streaming SSR.
            </div>
          </div>
        </div>
      </footer>
    </>
  )
}
